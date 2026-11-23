//! Escrita atômica e leitura com recuperação dos arquivos do usuário (projetos, sessão, pacotes).
//!
//! Escrita: grava em `<arquivo>.<pid>-<n>.tmp`, faz `sync_all` e troca o destino com `rename`
//! (no Windows, `MoveFileExW` + `MOVEFILE_REPLACE_EXISTING`). Um crash no meio deixa só o `.tmp`;
//! o arquivo final continua com a versão anterior inteira.
//! Leitura: JSON inválido vai para quarentena e, se houver `.bak` válido, ele é restaurado.

use serde_json::Value;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// `dir/nome.ext` → `dir/nome.ext<suffix>`.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

pub(crate) fn backup_path(path: &Path) -> PathBuf {
    with_suffix(path, ".bak")
}

/// Caminho temporário único (por processo e chamada) ao lado de `path`.
pub(crate) fn temp_path_for(path: &Path) -> PathBuf {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    with_suffix(path, &format!(".{}-{seq}.tmp", std::process::id()))
}

pub(crate) fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Apaga um temporário que não será usado. Falha só é registrada.
pub(crate) fn discard_temp(tmp: &Path) {
    if let Err(e) = fs::remove_file(tmp) {
        if e.kind() != ErrorKind::NotFound {
            eprintln!("[prismview-studio] remover {}: {e}", tmp.display());
        }
    }
}

/// Troca `path` pelo temporário já gravado. Em erro, o temporário é apagado.
pub(crate) fn commit_temp(tmp: &Path, path: &Path) -> Result<(), String> {
    fs::rename(tmp, path).map_err(|e| {
        discard_temp(tmp);
        format!("{}: {e}", path.display())
    })
}

/// Grava `bytes` em `path` de forma atômica. Com `backup`, a versão anterior vira `<arquivo>.bak`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], backup: bool) -> Result<(), String> {
    let tmp = temp_path_for(path);
    let written = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        discard_temp(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    if backup && path.is_file() {
        if let Err(e) = fs::copy(path, backup_path(path)) {
            eprintln!("[prismview-studio] backup de {}: {e}", path.display());
        }
    }
    commit_temp(&tmp, path)
}

/// Resultado de [`load_json_recovering`].
pub(crate) enum JsonLoad {
    Missing,
    Ok(Value),
    /// O arquivo estava inválido: foi para a quarentena e o `.bak` foi restaurado no lugar.
    Restored { value: Value, error: String },
    /// Inválido e sem `.bak` utilizável; o arquivo foi para a quarentena (se possível).
    Broken { error: String },
    /// Erro de I/O (arquivo bloqueado etc.). Nada foi movido; `backup` é o `.bak` lido, se válido.
    Unreadable { error: String, backup: Option<Value> },
}

fn read_backup(path: &Path) -> Option<(Vec<u8>, Value)> {
    let raw = fs::read(backup_path(path)).ok()?;
    let value = serde_json::from_slice::<Value>(&raw).ok()?;
    Some((raw, value))
}

/// Lê um JSON. Se o conteúdo for inválido, move o arquivo para `quarantine` e tenta o `.bak`.
pub(crate) fn load_json_recovering(path: &Path, quarantine: &Path) -> JsonLoad {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == ErrorKind::NotFound => return JsonLoad::Missing,
        Err(e) => {
            return JsonLoad::Unreadable {
                error: e.to_string(),
                backup: read_backup(path).map(|(_, v)| v),
            }
        }
    };
    let error = match serde_json::from_slice::<Value>(&raw) {
        Ok(value) => return JsonLoad::Ok(value),
        Err(e) => e.to_string(),
    };
    if let Some(parent) = quarantine.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("[prismview-studio] quarentena {}: {e}", parent.display());
        }
    }
    if let Err(e) = fs::rename(path, quarantine) {
        eprintln!(
            "[prismview-studio] mover {} para quarentena: {e}",
            path.display()
        );
    }
    match read_backup(path) {
        Some((raw_bak, value)) => {
            if let Err(e) = write_atomic(path, &raw_bak, false) {
                eprintln!("[prismview-studio] restaurar backup de {}: {e}", path.display());
            }
            JsonLoad::Restored { value, error }
        }
        None => JsonLoad::Broken { error },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prismview-atomic-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_replaces_and_keeps_backup() {
        let dir = scratch("write");
        let p = dir.join("a.json");
        write_atomic(&p, b"{\"v\":1}", true).unwrap();
        write_atomic(&p, b"{\"v\":2}", true).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"{\"v\":2}");
        assert_eq!(fs::read(backup_path(&p)).unwrap(), b"{\"v\":1}");
        let leftovers = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_json_is_quarantined_and_restored_from_backup() {
        let dir = scratch("recover");
        let p = dir.join("s.json");
        fs::write(backup_path(&p), b"{\"ok\":true}").unwrap();
        fs::write(&p, b"{\"ok\":tr").unwrap();
        let q = dir.join("q").join("s.corrupt.json");
        match load_json_recovering(&p, &q) {
            JsonLoad::Restored { value, .. } => assert_eq!(value["ok"], true),
            _ => panic!("esperava Restored"),
        }
        assert!(q.is_file());
        assert_eq!(fs::read(&p).unwrap(), b"{\"ok\":true}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncated_json_without_backup_is_broken() {
        let dir = scratch("broken");
        let p = dir.join("x.json");
        fs::write(&p, b"{").unwrap();
        let q = dir.join("x.corrupt.json");
        assert!(matches!(load_json_recovering(&p, &q), JsonLoad::Broken { .. }));
        assert!(!p.exists());
        assert!(q.is_file());
        let _ = fs::remove_dir_all(&dir);
    }
}
