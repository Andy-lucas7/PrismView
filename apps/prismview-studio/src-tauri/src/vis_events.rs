//! Visibilidade do desktop por eventos (PLANO_CORRECAO 4.5): em vez de recalcular a cobertura
//! a cada 500 ms, `SetWinEventHook` (foreground, minimizar, mostrar/ocultar, cloak, fim de
//! mover/redimensionar e mudança de posição só do processo em foreground) marca a visibilidade
//! como suja e acorda a thread de fundo. Mesmo padrão de `prismview-engine/.../win_events.rs`.
//!
//! Os hooks são out-of-context: os callbacks rodam na thread que os instalou (precisa de laço de
//! mensagens — a do `spawn_desktop_recovery_listener`). Callbacks só marcam um bit e acordam.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, EVENT_OBJECT_CLOAKED,
    EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW, EVENT_OBJECT_UNCLOAKED,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART,
    EVENT_SYSTEM_MOVESIZEEND, GA_ROOT, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
};

static DIRTY: AtomicBool = AtomicBool::new(true);
static WAKER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

const GLOBAL_RANGES: [(u32, u32); 5] = [
    (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
    (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
    (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
    (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
];

thread_local! {
    /// Hook de LOCATIONCHANGE do processo em foreground: (handle, pid).
    static LOCATION: Cell<(isize, u32)> = const { Cell::new((0, 0)) };
}

/// Quem acorda a thread de fundo (registrar uma vez, antes de instalar os hooks).
pub fn set_waker(waker: Box<dyn Fn() + Send + Sync>) {
    if WAKER.set(waker).is_err() {
        eprintln!("[prismview-studio] vis_events: waker já registrado");
    }
}

/// `true` (e limpa) se algo pode ter mudado a cobertura do desktop desde a última consulta.
pub fn take_dirty() -> bool {
    DIRTY.swap(false, Ordering::AcqRel)
}

/// Força um recálculo (ex.: monitores mudaram, pausa manual).
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Release);
    if let Some(wake) = WAKER.get() {
        wake();
    }
}

/// Dono dos hooks; criar e soltar na mesma thread (com laço de mensagens).
pub struct VisibilityHooks {
    global: Vec<HWINEVENTHOOK>,
}

impl VisibilityHooks {
    pub fn install() -> Self {
        let mut global = Vec::with_capacity(GLOBAL_RANGES.len());
        for &(min, max) in &GLOBAL_RANGES {
            // SAFETY: callback `extern "system"` estático; out-of-context, sem DLL.
            let hook = unsafe {
                SetWinEventHook(min, max, HMODULE::default(), Some(event_proc), 0, 0, WINEVENT_OUTOFCONTEXT)
            };
            if hook.is_invalid() {
                eprintln!("[prismview-studio] SetWinEventHook({min:#x}) falhou");
            } else {
                global.push(hook);
            }
        }
        // SAFETY: leitura da janela em foreground (pode ser nula).
        follow_process_of(unsafe { GetForegroundWindow() });
        Self { global }
    }
}

impl Drop for VisibilityHooks {
    fn drop(&mut self) {
        for hook in self.global.drain(..) {
            // SAFETY: hooks instalados por esta thread.
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
        unhook_location();
    }
}

fn unhook_location() {
    LOCATION.with(|cell| {
        let (hook, _) = cell.get();
        if hook != 0 {
            // SAFETY: hook instalado por esta thread em `follow_process_of`.
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(hook as *mut _));
            }
        }
        cell.set((0, 0));
    });
}

/// LOCATIONCHANGE só do processo em foreground (maximizar/arrastar a janela ativa). Global ele
/// dispararia a cada movimento do cursor.
fn follow_process_of(hwnd: HWND) {
    let mut pid = 0u32;
    if !hwnd.is_invalid() {
        // SAFETY: `pid` vive durante a chamada.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
    }
    if LOCATION.with(|cell| cell.get().1) == pid {
        return;
    }
    unhook_location();
    if pid == 0 {
        return;
    }
    // SAFETY: como em `install`.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            HMODULE::default(),
            Some(event_proc),
            pid,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if !hook.is_invalid() {
        LOCATION.with(|cell| cell.set((hook.0 as isize, pid)));
    }
}

unsafe extern "system" fn event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Só janelas de topo (ignora cursor, caret, controles filhos).
    if id_object != OBJID_WINDOW.0 || hwnd.is_invalid() || GetAncestor(hwnd, GA_ROOT) != hwnd {
        return;
    }
    if event == EVENT_SYSTEM_FOREGROUND {
        follow_process_of(hwnd);
    } else if event == EVENT_OBJECT_LOCATIONCHANGE && GetForegroundWindow() != hwnd {
        return;
    }
    mark_dirty();
}
