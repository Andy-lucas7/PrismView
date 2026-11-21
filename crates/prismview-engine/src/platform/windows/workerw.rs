//! Descoberta segura da camada de wallpaper do Explorer (Progman → WorkerW).
//!
//! Layouts suportados:
//! - [`ShellLayout::Legacy`] (Windows 10 e Windows 11 até 23H2): a mensagem `0x052C`
//!   faz o Progman criar um WorkerW top-level com o `SHELLDLL_DefView` (ícones) e um
//!   segundo WorkerW vazio logo atrás dele. O wallpaper vira filho desse WorkerW vazio.
//! - [`ShellLayout::Raised`] (Windows 11 24H2+): `SHELLDLL_DefView` e WorkerW continuam
//!   filhos do Progman (que ganha `WS_EX_NOREDIRECTIONBITMAP`). O wallpaper vira filho do
//!   Progman, entre o DefView (acima) e o WorkerW (abaixo).
//!
//! "Seguro" significa:
//! - `SendMessageTimeoutW` com `SMTO_ABORTIFHUNG`: Explorer travado não congela o motor;
//! - toda janela candidata precisa pertencer ao mesmo processo do Progman (rejeita
//!   WorkerW/DefView "falsos" criados por outros apps);
//! - nenhuma janela é usada sem `IsWindow`, e nenhuma falha derruba o processo (`Result`).

use std::time::Duration;

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetWindowLongPtrW, IsWindow, SendMessageTimeoutW,
    GWL_EXSTYLE, SMTO_ABORTIFHUNG, SMTO_NORMAL, WS_EX_NOREDIRECTIONBITMAP,
};

use super::util::{last_os_error, valid_hwnd, window_pid};
use crate::platform::{PlatformError, PlatformResult};

/// Mensagem não documentada que pede ao Progman para criar o WorkerW de wallpaper.
const WM_SPAWN_WORKER: u32 = 0x052C;
/// O Explorer cria o WorkerW de forma assíncrona; algumas tentativas curtas bastam.
const DISCOVERY_ATTEMPTS: u32 = 6;
const DISCOVERY_RETRY_SLEEP: Duration = Duration::from_millis(40);
/// Limite de WorkerW top-level percorridos (defesa contra listas anômalas).
const MAX_WORKER_SCAN: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellLayout {
    /// Win10 / Win11 ≤ 23H2: wallpaper filho do WorkerW top-level atrás dos ícones.
    Legacy,
    /// Win11 24H2+ ("raised desktop"): wallpaper filho do Progman, abaixo do DefView.
    Raised,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopLayer {
    pub layout: ShellLayout,
    pub progman: HWND,
    /// Janela que recebe o wallpaper como filho (WorkerW vazio ou Progman).
    pub parent: HWND,
    /// `SHELLDLL_DefView` (ícones). Nulo se não encontrado.
    pub defview: HWND,
    /// WorkerW de fundo. Em `Raised` fica abaixo do wallpaper. Nulo se ausente.
    pub worker_w: HWND,
    /// PID do Explorer dono do Progman.
    pub shell_pid: u32,
}

impl DesktopLayer {
    /// A camada ainda é válida (Explorer não reiniciou e as janelas existem)?
    pub fn is_alive(&self) -> bool {
        unsafe {
            IsWindow(self.progman).as_bool()
                && IsWindow(self.parent).as_bool()
                && window_pid(self.parent) == self.shell_pid
        }
    }
}

/// Localiza (criando se preciso) a camada de wallpaper. Pode bloquear até
/// `DISCOVERY_ATTEMPTS × (2 × timeout_ms + 40 ms)` no pior caso (Explorer lento):
/// chamar só na inicialização / reinício do shell, nunca por frame.
pub fn discover(timeout_ms: u32) -> PlatformResult<DesktopLayer> {
    unsafe {
        let progman = valid_hwnd(FindWindowW(w!("Progman"), PCWSTR::null()))
            .ok_or_else(|| PlatformError::Os("Progman não encontrado (Explorer em execução?)".into()))?;
        let shell_pid = window_pid(progman);
        if shell_pid == 0 {
            return Err(last_os_error("GetWindowThreadProcessId(Progman)"));
        }

        for attempt in 0..DISCOVERY_ATTEMPTS {
            request_worker(progman, timeout_ms);
            if let Some(layer) = probe(progman, shell_pid) {
                return Ok(layer);
            }
            if attempt + 1 < DISCOVERY_ATTEMPTS {
                std::thread::sleep(DISCOVERY_RETRY_SLEEP);
            }
        }

        // Fallback: Explorer não criou o WorkerW (0x052C ignorado / shell alternativo).
        // Filho direto do Progman atrás do DefView funciona em ambos os layouts, só
        // perde a garantia de não ser pintado por cima pelo próprio Progman no Legacy.
        let defview = find_child(progman, w!("SHELLDLL_DefView"), shell_pid).unwrap_or(HWND::default());
        let worker_w = find_child(progman, w!("WorkerW"), shell_pid).unwrap_or(HWND::default());
        Ok(DesktopLayer {
            layout: ShellLayout::Raised,
            progman,
            parent: progman,
            defview,
            worker_w,
            shell_pid,
        })
    }
}

unsafe fn request_worker(progman: HWND, timeout_ms: u32) {
    let flags = SMTO_NORMAL | SMTO_ABORTIFHUNG;
    let timeout = timeout_ms.max(50);
    let mut result = 0usize;
    // Variante clássica (wParam 0xD, lParam 1) e a "zerada" exigida por alguns builds.
    let _ = SendMessageTimeoutW(
        progman,
        WM_SPAWN_WORKER,
        WPARAM(0x0D),
        LPARAM(0x01),
        flags,
        timeout,
        Some(&mut result as *mut usize),
    );
    let _ = SendMessageTimeoutW(
        progman,
        WM_SPAWN_WORKER,
        WPARAM(0),
        LPARAM(0),
        flags,
        timeout,
        Some(&mut result as *mut usize),
    );
}

/// Filho direto de `parent` com a classe dada e pertencente ao Explorer.
unsafe fn find_child(parent: HWND, class: PCWSTR, shell_pid: u32) -> Option<HWND> {
    valid_hwnd(FindWindowExW(parent, HWND::default(), class, PCWSTR::null()))
        .filter(|h| window_pid(*h) == shell_pid)
}

unsafe fn probe(progman: HWND, shell_pid: u32) -> Option<DesktopLayer> {
    // 24H2+: DefView permanece filho do Progman mesmo após 0x052C.
    if let Some(defview) = find_child(progman, w!("SHELLDLL_DefView"), shell_pid) {
        let ex_style = GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32;
        let no_redirection = ex_style & WS_EX_NOREDIRECTIONBITMAP.0 != 0;
        let worker = find_child(progman, w!("WorkerW"), shell_pid);
        if no_redirection || worker.is_some() {
            return Some(DesktopLayer {
                layout: ShellLayout::Raised,
                progman,
                parent: progman,
                defview,
                worker_w: worker.unwrap_or(HWND::default()),
                shell_pid,
            });
        }
        // Legacy em que o Explorer ainda não moveu o DefView: tentar novamente.
        return None;
    }

    // Legacy: top-level que hospeda o DefView; o alvo é o WorkerW top-level seguinte.
    let mut search = LegacySearch {
        shell_pid,
        host: HWND::default(),
        defview: HWND::default(),
    };
    let _ = EnumWindows(
        Some(find_defview_host),
        LPARAM(&mut search as *mut LegacySearch as isize),
    );
    if search.host.is_invalid() {
        return None;
    }

    let mut candidate = valid_hwnd(FindWindowExW(HWND::default(), search.host, w!("WorkerW"), PCWSTR::null()));
    let mut scanned = 0u32;
    while let Some(worker) = candidate {
        if scanned >= MAX_WORKER_SCAN {
            break;
        }
        scanned += 1;
        if window_pid(worker) == shell_pid
            && find_child(worker, w!("SHELLDLL_DefView"), shell_pid).is_none()
        {
            return Some(DesktopLayer {
                layout: ShellLayout::Legacy,
                progman,
                parent: worker,
                defview: search.defview,
                worker_w: worker,
                shell_pid,
            });
        }
        candidate = valid_hwnd(FindWindowExW(HWND::default(), worker, w!("WorkerW"), PCWSTR::null()));
    }
    None
}

struct LegacySearch {
    shell_pid: u32,
    host: HWND,
    defview: HWND,
}

unsafe extern "system" fn find_defview_host(top: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut LegacySearch);
    if let Some(defview) = find_child(top, w!("SHELLDLL_DefView"), search.shell_pid) {
        search.host = top;
        search.defview = defview;
        return BOOL(0); // parar enumeração
    }
    BOOL(1)
}
