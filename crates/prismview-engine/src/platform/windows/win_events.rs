//! `SetWinEventHook` para detectar janelas maximizadas / tela cheia sem polling.
//!
//! Estratégia de custo mínimo:
//! - Hooks globais só para eventos raros: foreground, fim de move/size, minimizar,
//!   show/hide e cloak/uncloak (troca de área de trabalho virtual).
//! - `EVENT_OBJECT_LOCATIONCHANGE` (muito frequente) é ouvido **apenas no processo em
//!   primeiro plano**, re-registrado a cada troca de foreground. É ele que pega
//!   maximizar por duplo clique/Win+↑ e o F11 de navegadores/players.
//! - Todos `WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS`: nenhuma DLL injetada, os
//!   callbacks chegam pela fila de mensagens desta thread (acordam o `MsgWait` do loop).
//! - O callback filtra (só `OBJID_WINDOW` de janelas top-level não-tool) e apenas arma
//!   um timer de debounce; a varredura de cobertura roda no máximo uma vez por janela.

use ::windows::Win32::Foundation::{HMODULE, HWND};
use ::windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetWindowLongPtrW, CHILDID_SELF, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART,
    EVENT_SYSTEM_MOVESIZEEND, GA_ROOT, GWL_EXSTYLE, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WS_EX_TOOLWINDOW,
};

use super::signals::{self, SIG_FOREGROUND, TIMER_VISIBILITY};
use crate::platform::{PlatformError, PlatformResult};

const HOOK_FLAGS: u32 = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;

const GLOBAL_HOOKS: usize = 5;
const GLOBAL_RANGES: [(u32, u32); GLOBAL_HOOKS] = [
    (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
    (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
    (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
    (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
];

/// Dono dos hooks. Deve ser criado e destruído na mesma thread do loop (o tipo é `!Send`).
pub(crate) struct WinEventHooks {
    global: [HWINEVENTHOOK; GLOBAL_HOOKS],
    location: HWINEVENTHOOK,
    location_pid: u32,
}

impl WinEventHooks {
    pub(crate) fn install() -> PlatformResult<Self> {
        let mut global = [HWINEVENTHOOK::default(); GLOBAL_HOOKS];
        let mut installed = 0usize;
        for (slot, &(min, max)) in global.iter_mut().zip(GLOBAL_RANGES.iter()) {
            let hook = unsafe {
                SetWinEventHook(min, max, HMODULE::default(), Some(win_event_proc), 0, 0, HOOK_FLAGS)
            };
            if !hook.is_invalid() {
                *slot = hook;
                installed += 1;
            }
        }
        if installed == 0 {
            return Err(PlatformError::Os("SetWinEventHook falhou para todos os eventos".into()));
        }
        Ok(Self {
            global,
            location: HWINEVENTHOOK::default(),
            location_pid: 0,
        })
    }

    /// Passa a ouvir `EVENT_OBJECT_LOCATIONCHANGE` só do processo `pid` (foreground).
    pub(crate) fn follow_process(&mut self, pid: u32, own_pid: u32) {
        if pid == self.location_pid {
            return;
        }
        self.unhook_location();
        self.location_pid = pid;
        if pid == 0 || pid == own_pid {
            return;
        }
        self.location = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                HMODULE::default(),
                Some(win_event_proc),
                pid,
                0,
                HOOK_FLAGS,
            )
        };
    }

    fn unhook_location(&mut self) {
        if !self.location.is_invalid() {
            let _ = unsafe { UnhookWinEvent(self.location) };
        }
        self.location = HWINEVENTHOOK::default();
        self.location_pid = 0;
    }
}

impl Drop for WinEventHooks {
    fn drop(&mut self) {
        self.unhook_location();
        for hook in self.global.iter_mut() {
            if !hook.is_invalid() {
                let _ = unsafe { UnhookWinEvent(*hook) };
            }
            *hook = HWINEVENTHOOK::default();
        }
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time_ms: u32,
) {
    // Só a própria janela (descarta cursor, caret, scrollbars, itens de UIA...).
    if hwnd.is_invalid() || id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }
    match event {
        EVENT_SYSTEM_FOREGROUND => signals::raise(SIG_FOREGROUND),
        EVENT_OBJECT_LOCATIONCHANGE | EVENT_OBJECT_SHOW | EVENT_OBJECT_HIDE => {
            // Só top-level não-tool: ignora tooltips, menus, popups de IME etc.
            if GetAncestor(hwnd, GA_ROOT) != hwnd {
                return;
            }
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return;
            }
        }
        _ => {}
    }
    signals::schedule(TIMER_VISIBILITY);
}
