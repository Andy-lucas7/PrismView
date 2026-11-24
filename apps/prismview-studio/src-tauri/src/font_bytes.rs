//! Bytes de uma fonte para o texto alongado do Estúdio (`vstem` em WebAssembly).
//!
//! O WebView não expõe os contornos das fontes do sistema; este comando resolve `família + peso`
//! entre as fontes instaladas no Windows e as embutidas do Estúdio (`%APPDATA%\PrismView\fonts`) e
//! devolve o arquivo. Só abre arquivos que o banco de fontes conhece (nunca um caminho vindo do
//! JS), então não vira leitura arbitrária de disco. Fontes do PROJETO (`fontPath`) o JS lê pelo
//! protocolo de assets.
//!
//! Resposta binária: `[u32 LE: índice da face na coleção][bytes do arquivo]`.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use fontdb::{Database, Family, Query, Source, Stretch, Style, Weight};

/// Teto do arquivo devolvido (fontes CJK completas passam de 20 MB; acima disto o JS usa o raster).
const MAX_FONT_BYTES: u64 = 48 * 1024 * 1024;

static DB: OnceLock<Mutex<Database>> = OnceLock::new();

fn database() -> &'static Mutex<Database> {
    DB.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        db.load_fonts_dir(crate::prismview_root_dir().join("fonts"));
        Mutex::new(db)
    })
}

/// Nome de família sem aspas/vírgulas (o JS passa o primeiro item da pilha CSS).
fn clean_family(raw: &str) -> String {
    raw.split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_string()
}

struct Found {
    path: Option<PathBuf>,
    data: Option<Vec<u8>>,
    index: u32,
}

fn find(family: &str, weight: u16) -> Result<Found, String> {
    let name = clean_family(family);
    if name.is_empty() || name.len() > 128 {
        return Err("família inválida".into());
    }
    let db = database().lock().map_err(|_| "banco de fontes indisponível".to_string())?;
    let families = [Family::Name(&name)];
    let query = Query {
        families: &families,
        weight: Weight(weight.clamp(100, 900)),
        stretch: Stretch::Normal,
        style: Style::Normal,
    };
    let id = db.query(&query).ok_or_else(|| format!("fonte não encontrada: {name}"))?;
    let face = db.face(id).ok_or_else(|| "face sem dados".to_string())?;
    // `query` cai em outra família quando a pedida não existe? Não: só devolve faces da família.
    let (path, data) = match &face.source {
        Source::File(p) => (Some(p.clone()), None),
        Source::Binary(bytes) => (None, Some(bytes.as_ref().as_ref().to_vec())),
    };
    Ok(Found { path, data, index: face.index })
}

fn load(family: &str, weight: u16) -> Result<Vec<u8>, String> {
    let found = find(family, weight)?;
    let bytes = match (found.path, found.data) {
        (Some(path), _) => {
            let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
            if len > MAX_FONT_BYTES {
                return Err("fonte grande demais".into());
            }
            std::fs::read(&path).map_err(|e| e.to_string())?
        }
        (None, Some(data)) => data,
        (None, None) => return Err("face sem dados".into()),
    };
    if bytes.len() as u64 > MAX_FONT_BYTES {
        return Err("fonte grande demais".into());
    }
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&found.index.to_le_bytes());
    out.extend_from_slice(&bytes);
    Ok(out)
}

#[tauri::command]
pub async fn read_font_bytes(family: String, weight: u16) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || load(&family, weight))
        .await
        .map_err(|e| e.to_string())?
        .map(tauri::ipc::Response::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_is_cleaned_from_css_stack() {
        assert_eq!(clean_family("\"JetBrains Mono\", \"Outfit\", sans-serif"), "JetBrains Mono");
        assert_eq!(clean_family("  'Segoe UI'  "), "Segoe UI");
        assert_eq!(clean_family(""), "");
    }

    #[test]
    fn rejects_empty_and_huge_names() {
        assert!(find("", 400).is_err());
        assert!(find(&"x".repeat(200), 400).is_err());
    }

    /// No Windows o Segoe UI é fonte do sistema: o comando devolve índice + arquivo sfnt.
    #[test]
    #[cfg(target_os = "windows")]
    fn resolves_a_system_font_to_sfnt_bytes() {
        let out = load("Segoe UI", 400).expect("Segoe UI instalada no Windows");
        assert!(out.len() > 4096);
        let magic = &out[4..8];
        assert!(
            magic == [0, 1, 0, 0] || magic == *b"OTTO" || magic == *b"ttcf" || magic == *b"true",
            "cabeçalho sfnt inesperado: {magic:?}"
        );
    }

    #[test]
    fn unknown_family_is_an_error_not_a_fallback() {
        assert!(find("Fonte Que Nao Existe 9f3a", 400).is_err());
    }
}
