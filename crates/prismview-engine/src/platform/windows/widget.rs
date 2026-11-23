//! Janela de widget nativo (Entrega 3): popup sem borda, fora da barra de tarefas, nunca ativa,
//! com conteúdo 100% DirectComposition (`WS_EX_NOREDIRECTIONBITMAP`: sem bitmap GDI por trás,
//! o alpha pré-multiplicado da cadeia é composto direto sobre o desktop).
//!
//! - destravado: `WM_NCHITTEST` → `HTCAPTION` (arrasto nativo do sistema, sem foco);
//!   fim do arrasto (`WM_EXITSIZEMOVE`) → `SIG_WIDGET_MOVED` (o motor relê e persiste a posição);
//! - travado: cliques locais (`WM_LBUTTONUP`) → `SIG_WIDGET_CLICK` (toques 2D / CLICK_MESH);
//! - `click_through`: `WS_EX_TRANSPARENT | WS_EX_LAYERED` (alpha 255): o clique atravessa para o
//!   desktop e volta ao motor pelo toque observado (`DESKTOP_TAP`) testado contra o widget;
//! - `always_on_top`: `HWND_TOPMOST`; senão fica no fundo da pilha (`HWND_BOTTOM`), sobre o
//!   wallpaper e atrás das janelas normais.
//!
//! O wndproc só lê flags do `GWLP_USERDATA` e seta sinais (sem `RefCell`, sem alocação).

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use ::windows::Win32::Graphics::Gdi::ValidateRect;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, GetWindowRect, IsWindow,
    RegisterClassExW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWLP_USERDATA, GWL_EXSTYLE, HMENU, HTCAPTION, HTCLIENT, HWND_BOTTOM, HWND_NOTOPMOST,
    HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WM_DPICHANGED,
    WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_PAINT,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::signals::{self, SIG_REPAINT, SIG_WIDGET_MOVED};
use super::util::{id_from_hwnd, last_os_error, os_error};
use crate::platform::{NativeWindowId, PlatformResult};

const FLAG_LOCKED: isize = 1 << 0;
const FLAG_CLICK_THROUGH: isize = 1 << 1;

#[inline]
fn widget_class() -> PCWSTR {
    w!("PrismViewNativeWidget")
}

/// Estado inicial/novo da janela (px físicos do desktop virtual).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetWindowConfig {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub always_on_top: bool,
    pub locked: bool,
    pub click_through: bool,
}

/// Dono do HWND: destruída no `Drop` (soltar a superfície/presenter antes).
pub struct WidgetWindow {
    hwnd: HWND,
}

pub(crate) fn register_class(hinstance: HINSTANCE) -> PlatformResult<()> {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(widget_wndproc),
        hInstance: hinstance,
        lpszClassName: widget_class(),
        ..Default::default()
    };
    unsafe {
        if RegisterClassExW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(last_os_error("RegisterClassExW(widget)"));
        }
    }
    Ok(())
}

fn ex_style(cfg: &WidgetWindowConfig) -> u32 {
    let mut ex = WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_NOREDIRECTIONBITMAP.0;
    if cfg.always_on_top {
        ex |= WS_EX_TOPMOST.0;
    }
    if cfg.click_through {
        ex |= WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0;
    }
    ex
}

fn flag_bits(cfg: &WidgetWindowConfig) -> isize {
    (if cfg.locked { FLAG_LOCKED } else { 0 }) | (if cfg.click_through { FLAG_CLICK_THROUGH } else { 0 })
}

impl WidgetWindow {
    pub(crate) fn create(hinstance: HINSTANCE, cfg: &WidgetWindowConfig) -> PlatformResult<Self> {
        unsafe {
            let hwnd = CreateWindowExW(
                ::windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(ex_style(cfg)),
                widget_class(),
                w!("PrismView Widget"),
                WS_POPUP,
                cfg.x,
                cfg.y,
                i32::try_from(cfg.width.max(1)).unwrap_or(i32::MAX),
                i32::try_from(cfg.height.max(1)).unwrap_or(i32::MAX),
                HWND::default(),
                HMENU::default(),
                hinstance,
                None,
            )
            .map_err(|e| os_error("CreateWindowExW(widget)", e))?;
            let this = Self { hwnd };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, flag_bits(cfg));
            if cfg.click_through {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
            this.apply_z_order(cfg.always_on_top);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Ok(this)
        }
    }

    pub fn id(&self) -> NativeWindowId {
        id_from_hwnd(self.hwnd)
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_alive(&self) -> bool {
        unsafe { IsWindow(self.hwnd).as_bool() }
    }

    fn apply_z_order(&self, always_on_top: bool) {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        unsafe {
            if always_on_top {
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
            } else {
                let _ = SetWindowPos(self.hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
                let _ = SetWindowPos(self.hwnd, HWND_BOTTOM, 0, 0, 0, 0, flags);
            }
        }
    }

    /// Move/redimensiona (px físicos). Não muda z-order nem ativa.
    pub fn set_bounds(&self, x: i32, y: i32, width: u32, height: u32) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                HWND::default(),
                x,
                y,
                i32::try_from(width.max(1)).unwrap_or(i32::MAX),
                i32::try_from(height.max(1)).unwrap_or(i32::MAX),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    /// Aplica travado/sempre-no-topo/click-through sem recriar a janela.
    pub fn set_flags(&self, always_on_top: bool, locked: bool, click_through: bool) {
        let cfg = WidgetWindowConfig {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            always_on_top,
            locked,
            click_through,
        };
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, flag_bits(&cfg));
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex_style(&cfg) as isize);
            if click_through {
                let _ = SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
            let _ = SetWindowPos(
                self.hwnd,
                HWND::default(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        self.apply_z_order(always_on_top);
    }

    /// Retângulo atual `(x, y, w, h)` em px físicos (após arrasto/DPI).
    pub fn bounds(&self) -> Option<(i32, i32, u32, u32)> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r) }.ok()?;
        let w = u32::try_from((r.right - r.left).max(1)).unwrap_or(1);
        let h = u32::try_from((r.bottom - r.top).max(1)).unwrap_or(1);
        Some((r.left, r.top, w, h))
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for WidgetWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.hwnd).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn widget_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            let flags = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if flags & FLAG_LOCKED == 0 {
                LRESULT(HTCAPTION as isize)
            } else {
                LRESULT(HTCLIENT as isize)
            }
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let _ = ValidateRect(hwnd, None);
            signals::raise(SIG_REPAINT);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            signals::widget_clicked(hwnd, x, y);
            LRESULT(0)
        }
        WM_EXITSIZEMOVE => {
            signals::raise(SIG_WIDGET_MOVED);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // PMv2: adota o retângulo sugerido (mantém o tamanho lógico no monitor novo).
            let suggested = lparam.0 as *const RECT;
            if let Some(r) = suggested.as_ref() {
                let _ = SetWindowPos(
                    hwnd,
                    HWND::default(),
                    r.left,
                    r.top,
                    (r.right - r.left).max(1),
                    (r.bottom - r.top).max(1),
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                );
            }
            signals::raise(SIG_WIDGET_MOVED);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
