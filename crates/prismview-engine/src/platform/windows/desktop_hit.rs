//! Classifica um ponto da tela para o hit-test de toques no wallpaper (Fase 4).
//!
//! O wallpaper fica atrás dos ícones (WorkerW/Progman) e é `HTTRANSPARENT`: o clique vai para a
//! `SysListView32` do Explorer, nunca para nós. Por isso o toque é **observado** (Raw Input) e só
//! vale quando cai no fundo do desktop:
//! - janela sob o ponto é a lista de ícones do desktop (`SysListView32` → `SHELLDLL_DefView` →
//!   `Progman`/`WorkerW`), o próprio `SHELLDLL_DefView`/`Progman`/`WorkerW` ou a nossa superfície;
//! - e não há ícone sob o cursor: `LVM_GETHOTITEM` (o desktop faz hot-tracking no hover). A
//!   mensagem não leva ponteiros → nada é escrito na memória do Explorer; `SMTO_ABORTIFHUNG` +
//!   timeout curto: Explorer travado ⇒ "Outro" (não dispara).
//!
//! Nenhuma chamada aqui ativa janela ou muda foco: só leitura de estado.

use ::windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, SendMessageTimeoutW, WindowFromPoint, GA_PARENT, GA_ROOT, SMTO_ABORTIFHUNG,
};

use super::util::class_is;

/// `LVM_FIRST + 61`.
const LVM_GETHOTITEM: u32 = 0x1000 + 61;
const HOT_ITEM_TIMEOUT_MS: u32 = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopPoint {
    /// Fundo do desktop (área livre sobre o wallpaper).
    Background,
    /// Sobre um ícone do desktop.
    Icon,
    /// Outra janela, barra de tarefas, Explorer sem resposta…
    Other,
}

unsafe fn is_desktop_host(hwnd: HWND) -> bool {
    !hwnd.is_invalid() && (class_is(hwnd, "Progman") || class_is(hwnd, "WorkerW"))
}

/// Classifica `(x, y)` em pixels físicos do desktop virtual. Chamar fora do loop de render
/// (pode esperar até `HOT_ITEM_TIMEOUT_MS` pelo Explorer).
pub(crate) fn classify_point(x: i32, y: i32) -> DesktopPoint {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.is_invalid() {
            return DesktopPoint::Other;
        }

        if class_is(hwnd, "SysListView32") {
            let defview = GetAncestor(hwnd, GA_PARENT);
            if defview.is_invalid() || !class_is(defview, "SHELLDLL_DefView") {
                return DesktopPoint::Other;
            }
            if !is_desktop_host(GetAncestor(defview, GA_PARENT)) {
                return DesktopPoint::Other;
            }
            let mut hot: usize = usize::MAX;
            let ok = SendMessageTimeoutW(
                hwnd,
                LVM_GETHOTITEM,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                HOT_ITEM_TIMEOUT_MS,
                Some(&mut hot as *mut usize),
            );
            if ok.0 == 0 {
                return DesktopPoint::Other;
            }
            // Retorno é um `int`: -1 = nenhum item quente.
            return if (hot as u32 as i32) >= 0 {
                DesktopPoint::Icon
            } else {
                DesktopPoint::Background
            };
        }

        let background_class = class_is(hwnd, "SHELLDLL_DefView")
            || class_is(hwnd, "Progman")
            || class_is(hwnd, "WorkerW")
            || class_is(hwnd, "PrismViewWallpaperSurface");
        if background_class && is_desktop_host(GetAncestor(hwnd, GA_ROOT)) {
            DesktopPoint::Background
        } else {
            DesktopPoint::Other
        }
    }
}
