//! Janelas Win32 do host: superfície de wallpaper (uma por monitor) e controlador oculto.
//!
//! - Superfície: criada como popup oculta e depois reparentada como `WS_CHILD` da camada
//!   do Explorer (ver `workerw`). Não pinta via GDI (o wgpu apresenta direto), é
//!   transparente a cliques e nunca ativa.
//! - Controlador: top-level oculta (não message-only: precisa receber broadcasts como
//!   `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE` e `TaskbarCreated`), dona dos timers de
//!   debounce e alvo do `LoopProxy`.

use std::ffi::c_void;

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    WPARAM,
};
use ::windows::Win32::Graphics::Gdi::{MapWindowPoints, ValidateRect};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetAncestor, GetWindowLongPtrW, IsWindow, PostQuitMessage,
    RegisterClassExW, SetLayeredWindowAttributes, SetParent, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, SystemParametersInfoW, GA_PARENT, GWL_EXSTYLE, GWL_STYLE, HMENU, HTTRANSPARENT,
    HWND_TOP, LWA_ALPHA, MA_NOACTIVATE, SPI_GETDESKWALLPAPER, SPI_SETDESKWALLPAPER,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_HIDE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WM_APP, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_DPICHANGED_AFTERPARENT, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST,
    WM_PAINT, WM_SETTINGCHANGE, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSEXW, WS_CAPTION,
    WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, WTS_SESSION_LOCK,
    WTS_SESSION_UNLOCK,
};

use super::monitors::Win32Monitor;
use super::signals::{self, SIG_REPAINT, SIG_WAKE, TIMER_MONITORS, TIMER_SHELL};
use super::util::{last_os_error, os_error};
use super::workerw::{DesktopLayer, ShellLayout};
use crate::platform::PlatformResult;

#[inline]
fn surface_class() -> PCWSTR {
    w!("PrismViewWallpaperSurface")
}

#[inline]
fn controller_class() -> PCWSTR {
    w!("PrismViewDesktopController")
}

/// Acorda o loop (postado pelo `LoopProxy` de qualquer thread).
pub(crate) const WM_PRISM_WAKE: u32 = WM_APP + 0x40;
/// Pede o fim do loop (`PostQuitMessage` na thread do host).
pub(crate) const WM_PRISM_EXIT: u32 = WM_APP + 0x41;

const MAX_PATH_W: usize = 260;

pub(crate) fn register_classes(hinstance: HINSTANCE) -> PlatformResult<()> {
    register_class(hinstance, surface_class(), surface_wndproc)?;
    register_class(hinstance, controller_class(), controller_wndproc)
}

fn register_class(
    hinstance: HINSTANCE,
    class: PCWSTR,
    proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
) -> PlatformResult<()> {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(proc_),
        hInstance: hinstance,
        lpszClassName: class,
        // Sem hbrBackground/hCursor: nada de GDI apagando o fundo por cima do swapchain.
        ..Default::default()
    };
    unsafe {
        if RegisterClassExW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(last_os_error("RegisterClassExW"));
        }
    }
    Ok(())
}

pub(crate) fn create_controller(hinstance: HINSTANCE) -> PlatformResult<HWND> {
    unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            controller_class(),
            w!("PrismView Desktop Controller"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .map_err(|e| os_error("CreateWindowExW(controller)", e))
    }
}

/// Cria a superfície oculta (ainda top-level). `attach` a coloca na camada do desktop.
pub(crate) fn create_surface(hinstance: HINSTANCE) -> PlatformResult<HWND> {
    unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            surface_class(),
            w!("PrismView Wallpaper"),
            WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .map_err(|e| os_error("CreateWindowExW(wallpaper)", e))
    }
}

/// Reparenta `hwnd` na camada do desktop e o posiciona sobre `monitor` (pixels físicos).
/// Idempotente: pode ser chamado de novo após mudança de monitor/DPI ou reinício do shell.
pub(crate) fn attach(
    hwnd: HWND,
    layer: &DesktopLayer,
    monitor: &Win32Monitor,
    layered: bool,
) -> PlatformResult<()> {
    unsafe {
        if !IsWindow(hwnd).as_bool() || !IsWindow(layer.parent).as_bool() {
            return Err(crate::platform::PlatformError::Os(
                "janela de wallpaper ou camada do desktop inválida".into(),
            ));
        }

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let style = (style & !(WS_POPUP.0 | WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0))
            | WS_CHILD.0
            | WS_CLIPSIBLINGS.0
            | WS_CLIPCHILDREN.0;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);

        let mut ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        ex = (ex & !WS_EX_APPWINDOW.0) | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;
        if layered {
            ex |= WS_EX_LAYERED.0;
        } else {
            ex &= !WS_EX_LAYERED.0;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        if layered {
            // 24H2: o Progman não tem redirection bitmap; filho layered opaco é composto
            // corretamente pelo DWM (mesma abordagem usada por outros motores de wallpaper).
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)
                .map_err(|e| os_error("SetLayeredWindowAttributes", e))?;
        }

        if GetAncestor(hwnd, GA_PARENT) != layer.parent {
            // O retorno de SetParent (pai anterior) pode ser nulo mesmo em sucesso:
            // valida pelo pai efetivo.
            let _ = SetParent(hwnd, layer.parent);
            if GetAncestor(hwnd, GA_PARENT) != layer.parent {
                return Err(last_os_error("SetParent(camada do desktop)"));
            }
        }

        place(hwnd, layer, monitor)
    }
}

/// Reposiciona sobre o monitor e corrige o Z-order dentro do pai.
pub(crate) unsafe fn place(hwnd: HWND, layer: &DesktopLayer, monitor: &Win32Monitor) -> PlatformResult<()> {
    let r = monitor.rect();
    // Tela → coordenadas cliente do pai (WorkerW/Progman cobrem o desktop virtual,
    // cuja origem pode ser negativa com monitores à esquerda/acima do primário).
    let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
    MapWindowPoints(HWND::default(), layer.parent, &mut pts);
    let (x, y) = (pts[0].x, pts[0].y);
    let (w, h) = ((pts[1].x - pts[0].x).max(1), (pts[1].y - pts[0].y).max(1));

    let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED | SWP_NOOWNERZORDER;
    match layer.layout {
        ShellLayout::Legacy => {
            SetWindowPos(hwnd, HWND_TOP, x, y, w, h, flags)
                .map_err(|e| os_error("SetWindowPos(wallpaper)", e))?;
        }
        ShellLayout::Raised => {
            // Logo abaixo do DefView (ícones por cima do wallpaper)...
            let after = if !layer.defview.is_invalid() && IsWindow(layer.defview).as_bool() {
                layer.defview
            } else {
                HWND_TOP
            };
            SetWindowPos(hwnd, after, x, y, w, h, flags)
                .map_err(|e| os_error("SetWindowPos(wallpaper)", e))?;
            // ...e o WorkerW (wallpaper estático do Windows) abaixo do nosso.
            if !layer.worker_w.is_invalid() && IsWindow(layer.worker_w).as_bool() {
                let _ = SetWindowPos(
                    layer.worker_w,
                    hwnd,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn hide(hwnd: HWND) {
    unsafe {
        if IsWindow(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// Devolve uma janela de terceiros ao estado top-level oculto.
pub(crate) fn detach(hwnd: HWND) {
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return;
        }
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = SetParent(hwnd, HWND::default());
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !WS_CHILD.0) | WS_POPUP.0) as isize);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_LAYERED.0) as isize);
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Força o Explorer a repintar o wallpaper do sistema (senão o WorkerW pode manter o
/// último frame do motor). Reaplica o mesmo caminho sem `SPIF_UPDATEINIFILE`: nada é
/// persistido. Cor sólida/slideshow sem caminho: não mexe (setar "" removeria o wallpaper).
pub(crate) fn refresh_system_wallpaper() {
    let mut path = [0u16; MAX_PATH_W];
    let none = SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);
    unsafe {
        let got = SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            MAX_PATH_W as u32,
            Some(path.as_mut_ptr() as *mut c_void),
            none,
        );
        if got.is_ok() && path[0] != 0 {
            let _ = SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                Some(path.as_mut_ptr() as *mut c_void),
                none,
            );
        }
    }
}

// ───────────── Window procedures (só setam sinais; nunca bloqueiam) ─────────────

unsafe extern "system" fn surface_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            // Valida sem GDI (senão WM_PAINT se repete indefinidamente) e pede um frame.
            let _ = ValidateRect(hwnd, None);
            signals::raise(SIG_REPAINT);
            LRESULT(0)
        }
        WM_DPICHANGED_AFTERPARENT | WM_DISPLAYCHANGE => {
            signals::schedule(TIMER_MONITORS);
            LRESULT(0)
        }
        WM_DESTROY => {
            signals::surface_destroyed();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn controller_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            if signals::timer_fired(wparam.0) {
                return LRESULT(0);
            }
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            signals::schedule(TIMER_MONITORS);
            return LRESULT(0);
        }
        WM_WTSSESSION_CHANGE => {
            match wparam.0 as u32 {
                WTS_SESSION_LOCK => signals::set_session_locked(true),
                WTS_SESSION_UNLOCK => signals::set_session_locked(false),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_PRISM_WAKE => {
            signals::raise(SIG_WAKE);
            return LRESULT(0);
        }
        WM_PRISM_EXIT => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {
            let taskbar_created = signals::taskbar_created_msg();
            if taskbar_created != 0 && msg == taskbar_created {
                // Explorer reiniciou: espera assentar e redescobre o WorkerW.
                signals::schedule(TIMER_SHELL);
                return LRESULT(0);
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
