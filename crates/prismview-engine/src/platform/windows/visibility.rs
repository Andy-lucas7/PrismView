//! Avaliação de cobertura do desktop por monitor.
//!
//! Executada apenas quando um WinEvent relevante chega (com debounce), nunca por frame.
//! Um monitor é "coberto" se houver, em qualquer posição do Z-order, uma janela
//! top-level visível, não minimizada e não "cloaked" que:
//! - esteja maximizada nesse monitor (a área de trabalho fica invisível), ou
//! - tenha limites visíveis que contêm o retângulo inteiro do monitor (tela cheia
//!   borderless, vídeo F11, jogo em janela sem bordas), ou
//! - seja a janela de um jogo em tela cheia exclusiva (`QUNS_RUNNING_D3D_FULL_SCREEN`).

use std::ffi::c_void;

use ::windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use ::windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use ::windows::Win32::Graphics::Gdi::{
    MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
};
use ::windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_NOT_PRESENT, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, IsIconic,
    IsWindowVisible, IsZoomed, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};

use super::monitors::Win32Monitor;
use super::util::{class_name, eq_ascii_ci, window_pid, CLASS_BUF_LEN};

/// Classes do próprio shell que nunca "cobrem" o desktop.
const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Present,
    /// Proteção de tela, sessão bloqueada ou troca rápida de usuário.
    Away,
}

struct CoverageScan<'a> {
    monitors: &'a [Win32Monitor],
    covered: &'a mut [bool],
    own_pid: u32,
}

/// Recalcula `covered[i]` para `monitors[i]`. Não aloca.
pub(crate) fn scan(monitors: &[Win32Monitor], covered: &mut [bool], own_pid: u32) -> Presence {
    covered.iter_mut().for_each(|c| *c = false);
    let n = monitors.len().min(covered.len());
    let mut presence = Presence::Present;
    if n == 0 {
        return presence;
    }
    let monitors = &monitors[..n];
    let covered = &mut covered[..n];

    unsafe {
        match SHQueryUserNotificationState() {
            Ok(state) if state == QUNS_NOT_PRESENT => presence = Presence::Away,
            Ok(state) if state == QUNS_RUNNING_D3D_FULL_SCREEN => {
                let fg = GetForegroundWindow();
                mark_monitor(monitors, covered, MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY));
            }
            _ => {}
        }

        if !covered.iter().all(|&c| c) {
            let mut state = CoverageScan { monitors, covered, own_pid };
            let _ = EnumWindows(
                Some(coverage_proc),
                LPARAM(&mut state as *mut CoverageScan as isize),
            );
        }
    }
    presence
}

fn mark_monitor(monitors: &[Win32Monitor], covered: &mut [bool], hmonitor: HMONITOR) {
    if hmonitor.is_invalid() {
        return;
    }
    let raw = hmonitor.0 as isize;
    if let Some(i) = monitors.iter().position(|m| m.hmonitor == raw) {
        if let Some(c) = covered.get_mut(i) {
            *c = true;
        }
    }
}

#[inline]
fn contains(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left
        && outer.top <= inner.top
        && outer.right >= inner.right
        && outer.bottom >= inner.bottom
}

unsafe extern "system" fn coverage_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &mut *(lparam.0 as *mut CoverageScan);
    if !is_cover_candidate(hwnd, state.own_pid) {
        return BOOL(1);
    }

    if IsZoomed(hwnd).as_bool() {
        mark_monitor(state.monitors, state.covered, MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL));
    } else if let Some(bounds) = visible_bounds(hwnd) {
        for (monitor, covered) in state.monitors.iter().zip(state.covered.iter_mut()) {
            if !*covered && contains(&bounds, &monitor.rect()) {
                *covered = true;
            }
        }
    }

    if state.covered.iter().all(|&c| c) {
        BOOL(0) // todos cobertos: parar cedo
    } else {
        BOOL(1)
    }
}

/// Filtros do mais barato ao mais caro.
unsafe fn is_cover_candidate(hwnd: HWND, own_pid: u32) -> bool {
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    // Overlays click-through (FPS counters, Discord/NVIDIA overlay) não escondem o desktop.
    if ex & WS_EX_TRANSPARENT.0 != 0 {
        return false;
    }
    if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_NOACTIVATE.0 != 0 {
        return false;
    }
    if is_cloaked(hwnd) {
        return false; // outra área de trabalho virtual / app UWP suspenso
    }
    if window_pid(hwnd) == own_pid {
        return false;
    }
    let mut buf = [0u16; CLASS_BUF_LEN];
    let class = class_name(hwnd, &mut buf);
    !SHELL_CLASSES.iter().any(|c| eq_ascii_ci(class, c))
}

unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut c_void,
        std::mem::size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
}

/// Limites visíveis (sem a borda invisível de redimensionamento do Win10/11).
unsafe fn visible_bounds(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS,
        &mut r as *mut RECT as *mut c_void,
        std::mem::size_of::<RECT>() as u32,
    )
    .is_ok()
        || GetWindowRect(hwnd, &mut r).is_ok();
    (ok && r.right > r.left && r.bottom > r.top).then_some(r)
}
