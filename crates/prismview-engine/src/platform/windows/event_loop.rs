//! Loop Win32 sob demanda com semântica `ControlFlow` (estilo winit), sem winit.
//!
//! - `ControlFlow::Wait`: a thread dorme em `MsgWaitForMultipleObjectsEx(INFINITE)` até
//!   chegar mensagem, WinEvent, timer de debounce ou `LoopProxy::wake` → 0% de CPU.
//! - `ControlFlow::WaitUntil(t)`: acorda no prazo usando um waitable timer de alta
//!   resolução (Win10 1803+), com fallback para timeout em ms. É o pacing do FPS cap.
//! - `ControlFlow::Poll`: não dorme; usar só com present em vsync.
//! - `ControlFlow::Exit`: encerra (o mesmo que `LoopProxy::request_exit`/`WM_QUIT`).
//!
//! Regras do loop: zero `unwrap`/`expect`, zero alocação por iteração (buffer de eventos
//! pré-alocado e reaproveitado), falhas de API degradam para espera curta, nunca panic.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use ::windows::core::PCWSTR;
use ::windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HWND, LPARAM, WAIT_FAILED, WPARAM};
use ::windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE,
    TIMER_ALL_ACCESS,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, PostMessageW, TranslateMessage,
    MSG, MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
};

use super::signals::{SIG_REPAINT, SIG_TRAY_MENU, SIG_TRAY_OPEN, SIG_WAKE, SIG_WIDGET_CLICK, SIG_WIDGET_MOVED};
use super::window::{WM_PRISM_EXIT, WM_PRISM_WAKE};
use super::Win32DesktopHost;
use crate::platform::DesktopEvent;

/// Capacidade inicial do buffer de eventos (cresce só se um burst passar disso).
const EVENT_CAPACITY: usize = 32;
/// Backstop caso a espera falhe (evita spin a 100% de CPU).
const WAIT_FAILED_BACKOFF: Duration = Duration::from_millis(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFlow {
    /// Dormir até o próximo evento do sistema (cena estática, coberta, pausada).
    Wait,
    /// Dormir até o instante dado ou até um evento, o que vier primeiro.
    WaitUntil(Instant),
    /// Não dormir (render contínuo limitado pelo vsync do present).
    Poll,
    /// Sair do loop.
    Exit,
}

/// Callbacks do loop. Todos rodam na thread do host; `host` pode ser usado livremente.
pub trait LoopHandler {
    /// Evento de desktop (visibilidade, monitores, shell reiniciado).
    fn desktop_event(&mut self, _host: &mut Win32DesktopHost, _event: DesktopEvent) {}

    /// `LoopProxy::wake` chamado de outra thread (IPC, decoder, etc.).
    fn woken(&mut self, _host: &mut Win32DesktopHost) {}

    /// O sistema invalidou uma janela de wallpaper (ex.: deixou de estar coberta);
    /// apresentar ao menos um frame mesmo com a cena estática.
    fn repaint_requested(&mut self, _host: &mut Win32DesktopHost) {}

    /// Clique direito no ícone da bandeja: exibir menu e executar a ação escolhida.
    fn tray_menu_requested(&mut self, _host: &mut Win32DesktopHost) {}

    /// Duplo clique esquerdo no ícone da bandeja: abrir/focar o PrismView Studio.
    fn tray_open_requested(&mut self, _host: &mut Win32DesktopHost) {}

    /// Um widget nativo foi arrastado/redimensionado pelo sistema (fim do arrasto, DPI).
    fn widgets_moved(&mut self, _host: &mut Win32DesktopHost) {}

    /// Clique solto num widget travado: janela + ponto em px do cliente.
    fn widget_clicked(&mut self, _host: &mut Win32DesktopHost, _window: crate::platform::NativeWindowId, _point: (i32, i32)) {}

    /// Fila vazia: renderizar se preciso e decidir como dormir.
    fn about_to_wait(&mut self, host: &mut Win32DesktopHost) -> ControlFlow;

    /// Último callback antes de `run` retornar.
    fn exiting(&mut self, _host: &mut Win32DesktopHost) {}
}

/// Handle `Send + Sync` para acordar/encerrar o loop a partir de outras threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopProxy {
    controller: isize,
}

impl LoopProxy {
    pub(crate) fn new(controller: HWND) -> Self {
        Self { controller: controller.0 as isize }
    }

    fn post(&self, msg: u32) -> bool {
        let hwnd = HWND(self.controller as *mut c_void);
        !hwnd.is_invalid() && unsafe { PostMessageW(hwnd, msg, WPARAM(0), LPARAM(0)) }.is_ok()
    }

    /// Acorda o loop (chama `LoopHandler::woken`). `false` se o host já foi destruído.
    pub fn wake(&self) -> bool {
        self.post(WM_PRISM_WAKE)
    }

    /// Encerra o loop de forma ordenada (`exiting` é chamado).
    pub fn request_exit(&self) -> bool {
        self.post(WM_PRISM_EXIT)
    }
}

/// Waitable timer de alta resolução para `WaitUntil`.
struct DeadlineTimer(HANDLE);

impl DeadlineTimer {
    fn new() -> Option<Self> {
        unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))
            .ok()
            .filter(|h| !h.is_invalid())
            .map(Self)
        }
    }

    /// Arma o timer para `remaining` a partir de agora (relativo, em unidades de 100 ns).
    fn arm(&self, remaining: Duration) -> bool {
        let ticks = i64::try_from(remaining.as_nanos() / 100).unwrap_or(i64::MAX).max(1);
        let due = -ticks;
        unsafe { SetWaitableTimer(self.0, &due, 0, None, None, BOOL(0)) }.is_ok()
    }
}

impl Drop for DeadlineTimer {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct Waiter {
    timer: Option<DeadlineTimer>,
}

impl Waiter {
    fn new() -> Self {
        Self { timer: DeadlineTimer::new() }
    }

    fn wait(&self, deadline: Option<Instant>) {
        let result = unsafe {
            match deadline {
                None => MsgWaitForMultipleObjectsEx(None, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE),
                Some(deadline) => {
                    let now = Instant::now();
                    if deadline <= now {
                        return;
                    }
                    let remaining = deadline - now;
                    // Backstop em ms (arredondado para cima) mesmo com o timer armado.
                    let backstop_ms = u32::try_from(remaining.as_micros().div_ceil(1000))
                        .unwrap_or(INFINITE - 1)
                        .saturating_add(2)
                        .min(INFINITE - 1);
                    match &self.timer {
                        Some(timer) if timer.arm(remaining) => {
                            let handles = [timer.0];
                            MsgWaitForMultipleObjectsEx(
                                Some(&handles[..]),
                                backstop_ms,
                                QS_ALLINPUT,
                                MWMO_INPUTAVAILABLE,
                            )
                        }
                        _ => MsgWaitForMultipleObjectsEx(
                            None,
                            backstop_ms.saturating_sub(2).max(1),
                            QS_ALLINPUT,
                            MWMO_INPUTAVAILABLE,
                        ),
                    }
                }
            }
        };
        if result.0 == WAIT_FAILED.0 {
            std::thread::sleep(WAIT_FAILED_BACKOFF);
        }
    }
}

/// Bombeia a fila sem bloquear. `Some(code)` se chegou `WM_QUIT`.
fn pump_messages(msg: &mut MSG) -> Option<i32> {
    unsafe {
        while PeekMessageW(msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return Some(msg.wParam.0 as i32);
            }
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }
    None
}

/// Executa o loop até `ControlFlow::Exit`, `LoopProxy::request_exit` ou `WM_QUIT`.
/// Retorna o código de saída. Deve rodar na thread que criou o `host`.
pub fn run<H: LoopHandler>(host: &mut Win32DesktopHost, handler: &mut H) -> i32 {
    let waiter = Waiter::new();
    let mut events: Vec<DesktopEvent> = Vec::with_capacity(EVENT_CAPACITY);
    let mut msg = MSG::default();

    loop {
        // 1) Mensagens Win32: WinEvents, timers de debounce, broadcasts, proxy.
        if let Some(code) = pump_messages(&mut msg) {
            handler.exiting(host);
            return code;
        }

        // 2) Sinais → eventos (trabalho pesado fora dos callbacks).
        let loop_signals = host.process_signals();
        host.drain_events_into(&mut events);
        for event in events.drain(..) {
            handler.desktop_event(host, event);
        }
        if loop_signals & SIG_WAKE != 0 {
            handler.woken(host);
        }
        if loop_signals & SIG_REPAINT != 0 {
            handler.repaint_requested(host);
        }
        if loop_signals & SIG_TRAY_OPEN != 0 {
            handler.tray_open_requested(host);
        }
        if loop_signals & SIG_TRAY_MENU != 0 {
            handler.tray_menu_requested(host);
        }
        if loop_signals & SIG_WIDGET_MOVED != 0 {
            handler.widgets_moved(host);
        }
        if loop_signals & SIG_WIDGET_CLICK != 0 {
            if let Some((window, x, y)) = host.take_widget_click() {
                handler.widget_clicked(host, window, (x, y));
            }
        }

        // 3) Render / decisão de sono.
        match handler.about_to_wait(host) {
            ControlFlow::Exit => {
                handler.exiting(host);
                return 0;
            }
            ControlFlow::Poll => {}
            ControlFlow::Wait => waiter.wait(None),
            ControlFlow::WaitUntil(deadline) => waiter.wait(Some(deadline)),
        }
    }
}
