//! Ações de toque do wallpaper que saem do motor: abrir URL (navegador padrão) ou app/arquivo
//! (`ShellExecuteW`, verbo `open`). Chamado só a partir do loop, em resposta a um toque do
//! usuário sobre uma camada com `TouchAction::OpenUrl/OpenApp`.

use std::ffi::c_void;
use std::path::Path;

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND};
use ::windows::Win32::System::Threading::CreateMutexW;
use ::windows::Win32::UI::Shell::ShellExecuteW;
use ::windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SystemParametersInfoW, SPIF_SENDCHANGE,
    SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER, SW_RESTORE, SW_SHOWNORMAL,
};

use crate::platform::{PlatformError, PlatformResult};

/// Mutex nomeado que garante um único motor por sessão. Mantido vivo até o processo sair
/// (o handle nunca é fechado explicitamente: o SO libera na saída).
pub struct SingleInstance {
    _handle: HANDLE,
}

/// `Ok(Some)` = esta é a única instância; `Ok(None)` = já existe outro motor nesta sessão.
pub fn acquire_single_instance() -> PlatformResult<Option<SingleInstance>> {
    // SAFETY: nome literal terminado em NUL; atributos padrão.
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\PrismViewEngine")) }
        .map_err(|e| PlatformError::Os(format!("CreateMutexW: {e}")))?;
    // SAFETY: lido logo após CreateMutexW (que sempre define o último erro).
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Ok(None);
    }
    Ok(Some(SingleInstance { _handle: handle }))
}

/// Extensões que um toque OPEN_APP pode abrir (programas e atalhos; nada de scripts).
const OPEN_APP_EXTENSIONS: [&str; 3] = ["exe", "lnk", "appref-ms"];

/// Valida o alvo de OPEN_APP: caminho absoluto local, arquivo existente e extensão permitida.
/// Recusa UNC (`\\host\share`), `\\?\`/`\\.\` e caminhos relativos.
fn validate_open_app_target(target: &str) -> PlatformResult<()> {
    let normalized = target.replace('/', "\\");
    if normalized.starts_with("\\\\") {
        return Err(PlatformError::Os(format!("shell_open: caminho de rede/dispositivo recusado ({target})")));
    }
    let path = std::path::Path::new(&normalized);
    if !path.is_absolute() {
        return Err(PlatformError::Os(format!("shell_open: caminho relativo recusado ({target})")));
    }
    let ext_ok = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| OPEN_APP_EXTENSIONS.iter().any(|ok| e.eq_ignore_ascii_case(ok)));
    if !ext_ok {
        return Err(PlatformError::Os(format!("shell_open: tipo de arquivo recusado ({target})")));
    }
    if !path.is_file() {
        return Err(PlatformError::Os(format!("shell_open: arquivo não encontrado ({target})")));
    }
    Ok(())
}

/// Abre ou traz ao primeiro plano o `prismview-studio.exe` (ao lado do executável da engine).
pub fn open_studio() -> PlatformResult<()> {
    // 1) Se o Studio já estiver aberto, apenas restaura e foca a janela existente.
    if let Ok(hwnd) = unsafe { FindWindowW(PCWSTR::null(), w!("PrismView")) } {
        if !hwnd.is_invalid() {
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return Ok(());
        }
    }
    // 2) Caso contrário, lança `prismview-studio.exe` irmão do `current_exe`.
    let exe = std::env::current_exe()
        .map_err(|e| PlatformError::Os(format!("current_exe: {e}")))?;
    let studio = exe
        .parent()
        .map(|d| d.join("prismview-studio.exe"))
        .filter(|p| p.is_file())
        .ok_or_else(|| PlatformError::Os("prismview-studio.exe não encontrado".into()))?;
    std::process::Command::new(&studio)
        .spawn()
        .map_err(|e| PlatformError::Os(format!("abrir PrismView Studio: {e}")))?;
    Ok(())
}

/// Abre `target` com o manipulador padrão do Windows. URLs só `http(s)://`/`mailto:`; demais
/// valores são tratados como caminho de app/arquivo.
pub fn shell_open(target: &str, is_url: bool) -> PlatformResult<()> {
    let target = target.trim();
    if target.is_empty() {
        return Err(PlatformError::Os("shell_open: alvo vazio".into()));
    }
    if is_url {
        let lower = target.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")) {
            return Err(PlatformError::Os(format!("shell_open: URL recusada ({target})")));
        }
    } else {
        validate_open_app_target(target)?;
    }
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` termina em NUL e vive até o fim da chamada; demais ponteiros são nulos.
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Valores ≤ 32 são códigos de erro (contrato do ShellExecute).
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(PlatformError::Os(format!("ShellExecuteW falhou ({})", result.0 as isize)))
    }
}

/// Aplica `path` (PNG/JPG/BMP absoluto) como papel de parede real do Windows
/// (`SPI_SETDESKWALLPAPER`), atualizando Mica/Acrylic, cor de destaque e fallback de área de trabalho.
pub fn set_windows_desktop_wallpaper(path: &Path) -> PlatformResult<()> {
    if !path.is_file() {
        return Err(PlatformError::Os(format!(
            "set_windows_desktop_wallpaper: arquivo ausente ({})",
            path.display()
        )));
    }
    let normalized = path.to_string_lossy().replace('/', "\\");
    let wide: Vec<u16> = normalized.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` termina em NUL e vive até o fim da chamada; `SPI_SETDESKWALLPAPER` lê
    // `pvParam` como PCWSTR.
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as *mut c_void),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    }
    .map_err(|e| PlatformError::Os(format!("SPI_SETDESKWALLPAPER: {e}")))
}

#[cfg(test)]
mod tests {
    use super::validate_open_app_target;

    #[test]
    fn open_app_rejects_unc_relative_and_scripts() {
        assert!(validate_open_app_target(r"\\evil\s\a.exe").is_err());
        assert!(validate_open_app_target("//evil/s/a.exe").is_err());
        assert!(validate_open_app_target(r"\\?\C:\Windows\notepad.exe").is_err());
        assert!(validate_open_app_target("notepad.exe").is_err());
        assert!(validate_open_app_target(r"C:\Windows\System32\cmd.bat").is_err());
        assert!(validate_open_app_target(r"C:\x\run.ps1").is_err());
    }

    #[test]
    fn open_app_accepts_existing_local_exe() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        assert!(validate_open_app_target(&format!(r"{windir}\notepad.exe")).is_ok());
    }
}
