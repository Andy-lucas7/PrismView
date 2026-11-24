//! Marcador da sessão do Windows para decidir quando mostrar a intro: ela aparece no primeiro uso
//! depois de ligar o PC / entrar na sessão, e não ao reabrir o app na mesma sessão.
//!
//! Usa a hora de logon da sessão (`WTSINFOW::LogonTime`), que muda a cada boot e a cada logon,
//! inclusive com Inicialização Rápida (onde o uptime do kernel NÃO reinicia).

use windows::Win32::System::RemoteDesktop::{
    WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfo, WTSINFOW, WTS_CURRENT_SERVER_HANDLE,
    WTS_CURRENT_SESSION,
};
use windows::Win32::System::SystemInformation::GetTickCount64;

fn logon_marker() -> Option<String> {
    let mut buf = windows::core::PWSTR::null();
    let mut len = 0u32;
    // SAFETY: parâmetros válidos; o buffer devolvido é liberado com `WTSFreeMemory`.
    unsafe {
        WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSSessionInfo, &mut buf, &mut len)
            .ok()?;
        if buf.is_null() || (len as usize) < std::mem::size_of::<WTSINFOW>() {
            if !buf.is_null() {
                WTSFreeMemory(buf.0 as *mut _);
            }
            return None;
        }
        let info = &*(buf.0 as *const WTSINFOW);
        let logon = info.LogonTime;
        WTSFreeMemory(buf.0 as *mut _);
        (logon > 0).then(|| format!("logon:{logon}"))
    }
}

/// Reserva: instante do boot arredondado a 10 min (ruído do relógio não muda o marcador).
fn boot_marker() -> String {
    // SAFETY: sem argumentos.
    let up_ms = unsafe { GetTickCount64() };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let boot = now_ms.saturating_sub(up_ms);
    format!("boot:{}", boot / 600_000)
}

#[tauri::command]
pub(crate) fn get_session_marker() -> String {
    logon_marker().unwrap_or_else(boot_marker)
}
