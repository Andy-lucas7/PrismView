//! Ícone de bandeja (System Tray) Win32 nativo e levíssimo do `prismview-engine`.
//!
//! Usa a janela controladora oculta já existente em `Win32DesktopHost` (`Shell_NotifyIconW` +
//! `CreatePopupMenu` + `TrackPopupMenu`): zero WebView, zero thread extra, ~0 KB de RAM.
//!
//! - Visível por padrão enquanto o motor estiver em execução (configurável em tempo real via
//!   IPC `SetTrayVisible` e persistido em `%APPDATA%\PrismView\native\tray_enabled.txt`).
//! - Restaura automaticamente após reinício do Explorer (`TaskbarCreated`).
//! - Cleanup garantido (`NIM_DELETE` + `DestroyIcon`) no `Drop` e no `exiting()` do motor.

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use ::windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, DestroyIcon, DestroyMenu,
    GetCursorPos, GetSystemMetrics, LoadIconW, PostMessageW, SetForegroundWindow, TrackPopupMenu,
    HICON, IDI_APPLICATION, LR_DEFAULTCOLOR, MF_SEPARATOR, MF_STRING, SM_CXSMICON, SM_CYSMICON,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, WM_NULL,
};

use super::window::WM_PRISM_TRAY;

/// Ícone oficial do PrismView embutido direto no executável (sem leitura de disco).
const PRISMVIEW_ICO: &[u8] =
    include_bytes!("../../../../../apps/prismview-studio/src-tauri/icons/icon.ico");

const TRAY_UID: u32 = 1;
const CMD_OPEN_STUDIO: usize = 1001;
const CMD_TOGGLE_PAUSE: usize = 1002;
const CMD_REMOVE_WALLPAPER: usize = 1003;
const CMD_QUIT_ENGINE: usize = 1004;

/// Ação escolhida pelo usuário no menu de contexto da bandeja do motor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenStudio,
    TogglePause,
    RemoveWallpaper,
    QuitEngine,
}

/// Gerencia o ícone de bandeja Win32 vinculado à janela controladora do host.
pub(crate) struct TrayIcon {
    hwnd: HWND,
    icon: HICON,
    owns_icon: bool,
    visible: bool,
    installed: bool,
    paused: bool,
}

impl TrayIcon {
    pub(crate) fn new(hwnd: HWND) -> Self {
        let (icon, owns_icon) = load_tray_icon();
        Self {
            hwnd,
            icon,
            owns_icon,
            visible: false,
            installed: false,
            paused: false,
        }
    }

    /// Mostra ou oculta o ícone na bandeja em tempo real (idempotente).
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if visible {
            if !self.installed {
                self.add_icon();
            }
        } else if self.installed {
            self.remove_icon();
        }
    }

    /// Atualiza o tooltip conforme o estado de pausa do wallpaper.
    pub(crate) fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if self.installed {
            let nid = self.build_nid();
            let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
        }
    }

    /// Reinstala o ícone após `TaskbarCreated` (reinício do Explorer) se deveria estar visível.
    pub(crate) fn restore_if_visible(&mut self) {
        if self.visible {
            self.installed = false;
            self.add_icon();
        }
    }

    /// Remove explicitamente o ícone da bandeja (chamado no `exiting()` e no `Drop`).
    pub(crate) fn hide(&mut self) {
        self.visible = false;
        if self.installed {
            self.remove_icon();
        }
    }

    /// Abre o menu nativo na posição do cursor e devolve a ação clicada (se houver).
    pub(crate) fn show_context_menu(&self, paused: bool) -> Option<TrayAction> {
        if self.hwnd.is_invalid() {
            return None;
        }
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return None;
            };
            let pause_label = if paused {
                w!("Retomar Wallpaper")
            } else {
                w!("Pausar Wallpaper")
            };
            let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN_STUDIO, w!("Abrir PrismView Studio"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_TOGGLE_PAUSE, pause_label);
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_REMOVE_WALLPAPER, w!("Remover Wallpaper"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT_ENGINE, w!("Desligar Engine / Sair"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Regra Win32: trazer a janela dona ao primeiro plano para o menu fechar ao clicar fora.
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_NONOTIFY | TPM_RETURNCMD,
                pt.x,
                pt.y,
                0,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(self.hwnd, WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);

            match cmd.0 as usize {
                CMD_OPEN_STUDIO => Some(TrayAction::OpenStudio),
                CMD_TOGGLE_PAUSE => Some(TrayAction::TogglePause),
                CMD_REMOVE_WALLPAPER => Some(TrayAction::RemoveWallpaper),
                CMD_QUIT_ENGINE => Some(TrayAction::QuitEngine),
                _ => None,
            }
        }
    }

    fn add_icon(&mut self) {
        if self.hwnd.is_invalid() {
            return;
        }
        let nid = self.build_nid();
        if unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool() {
            self.installed = true;
        }
    }

    fn remove_icon(&mut self) {
        if self.hwnd.is_invalid() {
            self.installed = false;
            return;
        }
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_UID,
            ..Default::default()
        };
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
        self.installed = false;
    }

    fn build_nid(&self) -> NOTIFYICONDATAW {
        let mut tip = [0u16; 128];
        let text = if self.paused {
            "PrismView Engine — Pausado"
        } else {
            "PrismView Engine — Wallpaper ativo"
        };
        for (dst, ch) in tip.iter_mut().take(127).zip(text.encode_utf16()) {
            *dst = ch;
        }
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_UID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_PRISM_TRAY,
            hIcon: self.icon,
            szTip: tip,
            ..Default::default()
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.hide();
        if self.owns_icon && !self.icon.is_invalid() {
            let _ = unsafe { DestroyIcon(self.icon) };
            self.icon = HICON::default();
        }
    }
}

/// Decodifica `tray_enabled.txt`: `"0"` / `"false"` / `"off"` = falso; ausente ou outro = verdadeiro.
pub fn parse_tray_enabled(raw: &str) -> bool {
    let trimmed = raw.trim();
    !(trimmed == "0"
        || trimmed.eq_ignore_ascii_case("false")
        || trimmed.eq_ignore_ascii_case("off"))
}

/// Seleciona a entrada do `.ico` mais próxima de `target_px` (16/32 px na bandeja).
fn pick_ico_entry(ico: &[u8], target_px: u32) -> Option<&[u8]> {
    if ico.len() < 6 {
        return None;
    }
    let reserved = u16::from_le_bytes([ico[0], ico[1]]);
    let ico_type = u16::from_le_bytes([ico[2], ico[3]]);
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if reserved != 0 || ico_type != 1 || count == 0 {
        return None;
    }
    let mut best: Option<(u32, &[u8])> = None;
    for i in 0..count {
        let off = 6 + i * 16;
        if off + 16 > ico.len() {
            break;
        }
        let w = if ico[off] == 0 { 256 } else { u32::from(ico[off]) };
        let size = u32::from_le_bytes([ico[off + 8], ico[off + 9], ico[off + 10], ico[off + 11]]) as usize;
        let data_off =
            u32::from_le_bytes([ico[off + 12], ico[off + 13], ico[off + 14], ico[off + 15]]) as usize;
        let Some(slice) = ico.get(data_off..data_off.saturating_add(size)) else {
            continue;
        };
        let score = w.abs_diff(target_px);
        if best.is_none_or(|(bs, _)| score < bs) {
            best = Some((score, slice));
        }
    }
    best.map(|(_, slice)| slice)
}

fn load_tray_icon() -> (HICON, bool) {
    unsafe {
        let cx = GetSystemMetrics(SM_CXSMICON).max(16);
        let cy = GetSystemMetrics(SM_CYSMICON).max(16);
        if let Some(entry) = pick_ico_entry(PRISMVIEW_ICO, cx as u32) {
            if let Ok(hicon) = CreateIconFromResourceEx(entry, true, 0x0003_0000, cx, cy, LR_DEFAULTCOLOR) {
                if !hicon.is_invalid() {
                    return (hicon, true);
                }
            }
        }
        let fallback = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        (fallback, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_enabled_defaults_true_and_parses_false_tokens() {
        assert!(parse_tray_enabled("1"));
        assert!(parse_tray_enabled("true"));
        assert!(parse_tray_enabled(""));
        assert!(!parse_tray_enabled("0"));
        assert!(!parse_tray_enabled("false"));
        assert!(!parse_tray_enabled("OFF\n"));
    }

    #[test]
    fn embedded_icon_has_valid_entry() {
        let slice = pick_ico_entry(PRISMVIEW_ICO, 16);
        assert!(slice.is_some_and(|s| !s.is_empty()));
    }
}
