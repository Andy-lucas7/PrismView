//! Ações de toque do wallpaper que saem do motor: abrir URL (navegador padrão) ou app/arquivo
//! (`ShellExecuteW`, verbo `open`). Chamado só a partir do loop, em resposta a um toque do
//! usuário sobre uma camada com `TouchAction::OpenUrl/OpenApp`.

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::UI::Shell::ShellExecuteW;
use ::windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::platform::{PlatformError, PlatformResult};

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
