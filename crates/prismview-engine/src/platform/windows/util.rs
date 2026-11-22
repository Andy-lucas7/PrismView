//! Utilitários Win32 sem alocação: nomes de classe, conversões de handle e erros.

use std::ffi::c_void;

use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowThreadProcessId};

use crate::platform::{NativeWindowId, PlatformError};

pub(crate) const CLASS_BUF_LEN: usize = 64;

#[inline]
pub(crate) fn hwnd_from_id(id: NativeWindowId) -> HWND {
    HWND(id.0 as *mut c_void)
}

#[inline]
pub(crate) fn id_from_hwnd(hwnd: HWND) -> NativeWindowId {
    NativeWindowId(hwnd.0 as isize)
}

/// `Ok(hwnd)` não nulo → `Some`. Substitui `unwrap_or_default()` + `is_invalid()`.
#[inline]
pub(crate) fn valid_hwnd(result: ::windows::core::Result<HWND>) -> Option<HWND> {
    result.ok().filter(|h| !h.is_invalid())
}

/// Lê o nome da classe em `buf` (stack) e devolve o trecho preenchido.
pub(crate) unsafe fn class_name(hwnd: HWND, buf: &mut [u16; CLASS_BUF_LEN]) -> &[u16] {
    let len = GetClassNameW(hwnd, buf);
    let len = usize::try_from(len).unwrap_or(0).min(CLASS_BUF_LEN);
    &buf[..len]
}

/// Comparação UTF-16 × ASCII sem diferenciar maiúsculas (classes Win32 são case-insensitive).
pub(crate) fn eq_ascii_ci(wide: &[u16], ascii: &str) -> bool {
    wide.len() == ascii.len()
        && wide
            .iter()
            .zip(ascii.bytes())
            .all(|(&w, b)| w < 0x80 && (w as u8).eq_ignore_ascii_case(&b))
}

pub(crate) unsafe fn class_is(hwnd: HWND, ascii: &str) -> bool {
    let mut buf = [0u16; CLASS_BUF_LEN];
    eq_ascii_ci(class_name(hwnd, &mut buf), ascii)
}

/// PID dono da janela (0 se inválida).
pub(crate) unsafe fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
    pid
}

pub(crate) fn os_error(context: &str, err: ::windows::core::Error) -> PlatformError {
    PlatformError::Os(format!("{context}: {err}"))
}

/// Erro a partir de `GetLastError()`.
pub(crate) fn last_os_error(context: &str) -> PlatformError {
    os_error(context, ::windows::core::Error::from_win32())
}
