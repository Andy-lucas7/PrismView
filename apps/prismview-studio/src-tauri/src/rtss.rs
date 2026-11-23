//! Overlay do MSI Afterburner / RivaTuner Statistics Server (RTSS) no wallpaper.
//!
//! O RTSS se injeta em todo processo que apresenta frames DirectX — inclusive o motor nativo,
//! que ele trata como um jogo e cobre com o OSD. A única exclusão que ele respeita é o perfil
//! por executável (`<RTSS>\Profiles\<exe>.cfg` com `EnableHooking=0`, o mesmo que "Application
//! detection level: None" na interface dele). A pasta só aceita escrita de administrador, então
//! o perfil é gravado por um PowerShell elevado (o Windows pede confirmação uma vez).

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

use super::{wide_null, RegCloseKey, RegOpenKeyExW, RegQueryValueExW};
use super::{ERROR_SUCCESS, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ};

const ENGINE_EXE: &str = "prismview-engine.exe";
/// Perfil do RTSS: sem hook (nada de OSD, limitador ou captura) para o motor.
const PROFILE_BODY: &str = "[Hooking]\r\nEnableHooking=0\r\n[OSD]\r\nEnableOSD=0\r\n";
/// `CREATE_NO_WINDOW`: o PowerShell intermediário não abre console.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Serialize)]
pub struct RtssStatusDto {
    /// RTSS instalado (MSI Afterburner instala junto).
    pub installed: bool,
    /// O motor já está excluído no perfil do RTSS.
    pub excluded: bool,
}

fn read_reg_string(hive: isize, subkey: &str, value: &str) -> Option<String> {
    let subkey = wide_null(subkey);
    let value = wide_null(value);
    let mut hkey: isize = 0;
    // SAFETY: buffers terminados em NUL; a chave é fechada abaixo.
    if unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_READ, &mut hkey) } != ERROR_SUCCESS {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut cb = (buf.len() * 2) as u32;
    let mut kind = 0u32;
    // SAFETY: `buf` tem `cb` bytes.
    let status = unsafe {
        RegQueryValueExW(hkey, value.as_ptr(), std::ptr::null_mut(), &mut kind, buf.as_mut_ptr().cast(), &mut cb)
    };
    // SAFETY: chave aberta acima.
    unsafe {
        RegCloseKey(hkey);
    }
    if status != ERROR_SUCCESS || kind != REG_SZ {
        return None;
    }
    let len = (cb as usize / 2).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..len]);
    let text = text.trim_end_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn install_dir() -> Option<PathBuf> {
    [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Unwinder\RTSS"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Unwinder\RTSS"),
        (HKEY_CURRENT_USER, r"Software\Unwinder\RTSS"),
    ]
    .into_iter()
    .filter_map(|(hive, key)| read_reg_string(hive, key, "InstallDir"))
    .map(PathBuf::from)
    .find(|dir| dir.join("Profiles").is_dir())
}

fn profile_path() -> Option<PathBuf> {
    install_dir().map(|dir| dir.join("Profiles").join(format!("{ENGINE_EXE}.cfg")))
}

fn profile_excludes_engine(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|raw| raw.lines().any(|l| l.trim().eq_ignore_ascii_case("EnableHooking=0")))
        .unwrap_or(false)
}

pub fn status() -> RtssStatusDto {
    match profile_path() {
        Some(path) => RtssStatusDto {
            installed: true,
            excluded: profile_excludes_engine(&path),
        },
        None => RtssStatusDto {
            installed: false,
            excluded: false,
        },
    }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Grava o perfil de exclusão com um PowerShell elevado (UAC) e espera terminar.
pub fn exclude_engine() -> Result<RtssStatusDto, String> {
    use std::os::windows::process::CommandExt;
    let path = profile_path().ok_or("RivaTuner Statistics Server não encontrado")?;
    if profile_excludes_engine(&path) {
        return Ok(status());
    }
    let literal = path.to_string_lossy().replace('\'', "''");
    let body = PROFILE_BODY.replace("\r\n", "`r`n");
    let inner = format!("Set-Content -LiteralPath '{literal}' -Encoding ASCII -NoNewline -Value \"{body}\"");
    let utf16: Vec<u8> = inner.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let outer = format!(
        "$p = Start-Process -FilePath powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-EncodedCommand','{}'; exit $p.ExitCode",
        base64(&utf16)
    );
    let result = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &outer])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if !profile_excludes_engine(&path) {
        let detail = String::from_utf8_lossy(&result.stderr);
        eprintln!("[prismview-studio] perfil RTSS não gravado: {}", detail.trim());
        return Err("Permissão negada ou cancelada: o perfil do RivaTuner não foi gravado.".to_string());
    }
    Ok(status())
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
