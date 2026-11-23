//! Estado thread-local compartilhado entre callbacks Win32 (wndproc, WinEvent) e o host.
//!
//! Callbacks só setam bits em `Cell`s e armam timers de debounce: nada de `RefCell`,
//! alocação ou trabalho pesado dentro do callback. Isso torna seguro o reentrância
//! (mensagens "sent" despachadas durante `SetWindowPos`/`SetParent`) e mantém o custo
//! de cada evento do sistema em poucas instruções. O trabalho real acontece em
//! `Win32DesktopHost::process_signals`, chamado pelo loop.

use std::cell::Cell;
use std::ffi::c_void;

use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

// ───────────── Bits de sinal ─────────────

/// Reavaliar cobertura (janela maximizada / tela cheia).
pub(crate) const SIG_VISIBILITY: u32 = 1 << 0;
/// Janela em primeiro plano mudou: re-seguir o processo com o hook de LOCATIONCHANGE.
pub(crate) const SIG_FOREGROUND: u32 = 1 << 1;
/// Reenumerar monitores (resolução, DPI, conectar/desconectar).
pub(crate) const SIG_MONITORS: u32 = 1 << 2;
/// Tentar (re)descobrir o WorkerW e reanexar janelas.
pub(crate) const SIG_SHELL_RETRY: u32 = 1 << 3;
/// Uma janela de wallpaper foi destruída por terceiros (Explorer morreu/recriou WorkerW).
pub(crate) const SIG_WINDOW_LOST: u32 = 1 << 4;
/// Sessão bloqueada/desbloqueada.
pub(crate) const SIG_SESSION: u32 = 1 << 5;
/// `LoopProxy::wake` vindo de outra thread.
pub(crate) const SIG_WAKE: u32 = 1 << 6;
/// `WM_PAINT` numa janela de wallpaper: o renderer deve apresentar um frame.
pub(crate) const SIG_REPAINT: u32 = 1 << 7;
/// Clique direito no ícone da bandeja: abrir menu de contexto fora do wndproc.
pub(crate) const SIG_TRAY_MENU: u32 = 1 << 8;
/// Duplo clique esquerdo no ícone da bandeja: abrir/focar o PrismView Studio.
pub(crate) const SIG_TRAY_OPEN: u32 = 1 << 9;
/// `TaskbarCreated` recebido: restaurar ícone da bandeja se visível.
pub(crate) const SIG_TASKBAR_CREATED: u32 = 1 << 10;

/// Um widget nativo terminou de ser arrastado ou mudou de DPI: reler as posições.
pub(crate) const SIG_WIDGET_MOVED: u32 = 1 << 11;
/// Clique solto dentro de um widget nativo (ponto em `WIDGET_CLICK`).
pub(crate) const SIG_WIDGET_CLICK: u32 = 1 << 12;

/// Bits que o loop entrega direto ao handler (não viram `DesktopEvent`).
pub(crate) const LOOP_SIGNALS: u32 =
    SIG_WAKE | SIG_REPAINT | SIG_TRAY_MENU | SIG_TRAY_OPEN | SIG_WIDGET_MOVED | SIG_WIDGET_CLICK;

// ───────────── Timers (id = índice em DEBOUNCE_MS) ─────────────

pub(crate) const TIMER_VISIBILITY: usize = 1;
pub(crate) const TIMER_MONITORS: usize = 2;
pub(crate) const TIMER_SHELL: usize = 3;
pub(crate) const ALL_TIMERS: [usize; 3] = [TIMER_VISIBILITY, TIMER_MONITORS, TIMER_SHELL];
const TIMER_SLOTS: usize = 4;

thread_local! {
    static PENDING: Cell<u32> = const { Cell::new(0) };
    static ARMED: Cell<u32> = const { Cell::new(0) };
    static CONTROLLER: Cell<isize> = const { Cell::new(0) };
    static DEBOUNCE_MS: Cell<[u32; TIMER_SLOTS]> = const { Cell::new([0; TIMER_SLOTS]) };
    static SESSION_LOCKED: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED_MSG: Cell<u32> = const { Cell::new(0) };
    /// > 0 enquanto o próprio host destrói janelas (WM_DESTROY esperado).
    static EXPECTED_DESTROY: Cell<u32> = const { Cell::new(0) };
    /// Último clique em widget: (hwnd, x, y) em px do cliente (o mais recente vence).
    static WIDGET_CLICK: Cell<(isize, i32, i32)> = const { Cell::new((0, 0, 0)) };
}

/// `WM_LBUTTONUP` num widget travado: guarda o ponto e sinaliza o loop.
pub(crate) fn widget_clicked(hwnd: HWND, x: i32, y: i32) {
    let _ = WIDGET_CLICK.try_with(|c| c.set((hwnd.0 as isize, x, y)));
    raise(SIG_WIDGET_CLICK);
}

/// Retira o clique pendente (hwnd 0 = nenhum).
pub(crate) fn take_widget_click() -> (isize, i32, i32) {
    WIDGET_CLICK.try_with(|c| c.replace((0, 0, 0))).unwrap_or((0, 0, 0))
}

#[inline]
fn signal_for_timer(timer: usize) -> u32 {
    match timer {
        TIMER_VISIBILITY => SIG_VISIBILITY,
        TIMER_MONITORS => SIG_MONITORS,
        TIMER_SHELL => SIG_SHELL_RETRY,
        _ => 0,
    }
}

#[inline]
fn controller_hwnd() -> HWND {
    HWND(CONTROLLER.try_with(Cell::get).unwrap_or(0) as *mut c_void)
}

/// Já existe um host ativo nesta thread?
pub(crate) fn is_installed() -> bool {
    CONTROLLER.try_with(Cell::get).unwrap_or(0) != 0
}

pub(crate) fn install(
    controller: HWND,
    visibility_debounce_ms: u32,
    monitors_debounce_ms: u32,
    shell_debounce_ms: u32,
    taskbar_created_msg: u32,
) {
    let mut debounce = [0u32; TIMER_SLOTS];
    debounce[TIMER_VISIBILITY] = visibility_debounce_ms.max(1);
    debounce[TIMER_MONITORS] = monitors_debounce_ms.max(1);
    debounce[TIMER_SHELL] = shell_debounce_ms.max(1);
    let _ = CONTROLLER.try_with(|c| c.set(controller.0 as isize));
    let _ = DEBOUNCE_MS.try_with(|c| c.set(debounce));
    let _ = TASKBAR_CREATED_MSG.try_with(|c| c.set(taskbar_created_msg));
    let _ = PENDING.try_with(|c| c.set(0));
    let _ = ARMED.try_with(|c| c.set(0));
    let _ = SESSION_LOCKED.try_with(|c| c.set(false));
}

pub(crate) fn reset() {
    let _ = CONTROLLER.try_with(|c| c.set(0));
    let _ = PENDING.try_with(|c| c.set(0));
    let _ = ARMED.try_with(|c| c.set(0));
    let _ = TASKBAR_CREATED_MSG.try_with(|c| c.set(0));
}

#[inline]
pub(crate) fn raise(bits: u32) {
    let _ = PENDING.try_with(|c| c.set(c.get() | bits));
}

/// Retira todos os sinais pendentes.
#[inline]
pub(crate) fn take() -> u32 {
    PENDING.try_with(|c| c.replace(0)).unwrap_or(0)
}

/// Arma o timer de debounce com o intervalo configurado (no-op se já armado).
#[inline]
pub(crate) fn schedule(timer: usize) {
    let ms = DEBOUNCE_MS
        .try_with(Cell::get)
        .ok()
        .and_then(|d| d.get(timer).copied())
        .unwrap_or(1);
    schedule_ms(timer, ms);
}

/// Arma `timer` para disparar em `ms`. Eventos repetidos dentro da janela são absorvidos
/// (throttle com disparo final): o handler sempre lê o estado *atual* do sistema.
pub(crate) fn schedule_ms(timer: usize, ms: u32) {
    let bit = 1u32 << (timer as u32 & 31);
    let armed = ARMED.try_with(Cell::get).unwrap_or(0);
    if armed & bit != 0 {
        return;
    }
    let controller = controller_hwnd();
    let ok = !controller.is_invalid() && unsafe { SetTimer(controller, timer, ms.max(1), None) } != 0;
    if ok {
        let _ = ARMED.try_with(|c| c.set(c.get() | bit));
    } else {
        // Sem timer: entrega imediatamente (perde só o debounce).
        raise(signal_for_timer(timer));
    }
}

/// Chamado em `WM_TIMER` do controlador. Retorna `true` se o timer era nosso.
pub(crate) fn timer_fired(timer: usize) -> bool {
    let sig = signal_for_timer(timer);
    if sig == 0 {
        return false;
    }
    let controller = controller_hwnd();
    if !controller.is_invalid() {
        let _ = unsafe { KillTimer(controller, timer) };
    }
    let bit = 1u32 << (timer as u32 & 31);
    let _ = ARMED.try_with(|c| c.set(c.get() & !bit));
    raise(sig);
    true
}

pub(crate) fn set_session_locked(locked: bool) {
    let _ = SESSION_LOCKED.try_with(|c| c.set(locked));
    raise(SIG_SESSION);
}

pub(crate) fn session_locked() -> bool {
    SESSION_LOCKED.try_with(Cell::get).unwrap_or(false)
}

pub(crate) fn taskbar_created_msg() -> u32 {
    TASKBAR_CREATED_MSG.try_with(Cell::get).unwrap_or(0)
}

/// `WM_DESTROY` de uma janela de wallpaper.
pub(crate) fn surface_destroyed() {
    if EXPECTED_DESTROY.try_with(Cell::get).unwrap_or(0) == 0 {
        raise(SIG_WINDOW_LOST);
        schedule(TIMER_SHELL);
    }
}

/// Marca destruições feitas pelo próprio host enquanto o guard existir.
pub(crate) struct ExpectedDestroy(());

impl ExpectedDestroy {
    pub(crate) fn new() -> Self {
        let _ = EXPECTED_DESTROY.try_with(|c| c.set(c.get().saturating_add(1)));
        Self(())
    }
}

impl Drop for ExpectedDestroy {
    fn drop(&mut self) {
        let _ = EXPECTED_DESTROY.try_with(|c| c.set(c.get().saturating_sub(1)));
    }
}
