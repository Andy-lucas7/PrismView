//! Backend Windows do `DesktopHost` (Fase 0 do PLANO_NATIVO).
//!
//! - `workerw`: descoberta segura da camada de wallpaper (Win10 / Win11 / 24H2+).
//! - `monitors`: monitores em pixels físicos com DPI efetivo por monitor (PMv2).
//! - `window`: superfícies de wallpaper (1 por monitor) + janela controladora oculta.
//! - `win_events`: `SetWinEventHook` → detecção de maximizado/tela cheia sem polling.
//! - `visibility`: varredura de cobertura por monitor (só sob evento, com debounce).
//! - `event_loop`: loop Win32 com `ControlFlow::{Wait, WaitUntil, Poll, Exit}`.
//! - `video`: `MfVideoDecoder` (Media Foundation HW → D3D11 → textura compartilhada → wgpu/DX12).
//! - `sensors`: `Win32SystemSensors` (WASAPI loopback, Raw Input, SMTC, telemetria) → `SignalBus`.
//!
//! Modelo de threads: o host, suas janelas, hooks e o loop vivem numa única thread
//! (o tipo é `!Send` por conter `HWND`). Outras threads falam com ele via `LoopProxy`.
//!
//! Uso típico:
//! ```ignore
//! let mut host = Win32DesktopHost::new(HostConfig::default())?;
//! for m in host.list_monitors()? {
//!     let window = host.create_wallpaper_window(&m.id)?; // superfície wgpu por monitor
//! }
//! event_loop::run(&mut host, &mut renderer);
//! ```

mod composition;
mod desktop_hit;
pub mod event_loop;
pub mod fonts;
pub mod monitors;
pub mod sensors;
mod shell;
mod signals;
pub mod tray;
mod util;
pub mod video;
mod visibility;
pub mod widget;
mod win_events;
mod window;
pub mod workerw;

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, POINT};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use ::windows::Win32::System::Threading::GetCurrentProcessId;
use ::windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetCursorPos, GetForegroundWindow, IsWindow, KillTimer, RegisterWindowMessageW,
};

pub use composition::DCompPresenter;
pub use event_loop::{run, ControlFlow, LoopHandler, LoopProxy};
pub use fonts::resolve_font_files;
pub use monitors::Win32Monitor;
pub use sensors::{SensorsConfig, Win32SystemSensors};
pub use shell::{acquire_single_instance, open_studio, shell_open};
pub use tray::{parse_tray_enabled, TrayAction};
pub use video::{MfVideoDecoder, VIDEO_TEXTURE_FORMAT};
pub use widget::{WidgetWindow, WidgetWindowConfig};
pub use workerw::{DesktopLayer, ShellLayout};

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn SetProcessWorkingSetSize(hprocess: *mut std::ffi::c_void, min: usize, max: usize) -> i32;
}

/// Solicita ao gerenciador de memória do Windows devolver páginas frias (compilação de shaders,
/// decodificação inicial de PNGs, hibernação) sem impactar o caminho quente de renderização.
pub fn trim_working_set() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

use self::monitors::{enumerate, ensure_per_monitor_dpi_awareness, same_layout};
use self::signals::{
    ExpectedDestroy, ALL_TIMERS, LOOP_SIGNALS, SIG_FOREGROUND, SIG_MONITORS, SIG_SESSION,
    SIG_SHELL_RETRY, SIG_TASKBAR_CREATED, SIG_VISIBILITY, SIG_WINDOW_LOST, TIMER_SHELL,
    TIMER_VISIBILITY,
};
use self::tray::TrayIcon;
use self::util::{hwnd_from_id, id_from_hwnd, os_error, window_pid};
use self::visibility::Presence;
use self::win_events::WinEventHooks;
use crate::platform::{
    DesktopEvent, DesktopHost, DesktopVisibility, MonitorInfo, NativeWindowId, PlatformError,
    PlatformResult,
};

/// Máximo de tentativas de reanexar após o Explorer sumir (≈ 30 s com o intervalo abaixo).
const SHELL_MAX_RETRIES: u32 = 60;
const SHELL_RETRY_INTERVAL_MS: u32 = 500;
/// Se os hooks falharem por completo, reavalia a cobertura nesse intervalo (fallback).
const FALLBACK_VISIBILITY_POLL_MS: u32 = 1000;
const EVENT_QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostConfig {
    /// Timeout de cada `SendMessageTimeoutW` ao Progman.
    pub shell_message_timeout_ms: u32,
    /// Janela de debounce dos WinEvents antes de reavaliar cobertura.
    pub visibility_debounce_ms: u32,
    /// Debounce de `WM_DISPLAYCHANGE`/`WM_DPICHANGED`/`WM_SETTINGCHANGE`.
    pub monitors_debounce_ms: u32,
    /// Espera após `TaskbarCreated`/janela perdida antes de redescobrir o WorkerW.
    pub shell_restart_delay_ms: u32,
    /// Win11 24H2+: superfície `WS_EX_LAYERED` (alpha 255) sob o Progman.
    pub layered_on_raised_desktop: bool,
    /// Repintar o wallpaper do Windows ao soltar a última superfície.
    pub refresh_wallpaper_on_detach: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            shell_message_timeout_ms: 1000,
            visibility_debounce_ms: 80,
            monitors_debounce_ms: 250,
            shell_restart_delay_ms: 400,
            layered_on_raised_desktop: true,
            refresh_wallpaper_on_detach: true,
        }
    }
}

#[derive(Debug)]
struct Attachment {
    hwnd: HWND,
    monitor_id: String,
    /// Criada pelo host (recriada se o Explorer a destruir) ou fornecida por terceiros.
    owned: bool,
}

pub struct Win32DesktopHost {
    config: HostConfig,
    hinstance: HINSTANCE,
    controller: HWND,
    own_pid: u32,
    wts_registered: bool,
    layer: Option<DesktopLayer>,
    shell_retries: u32,
    monitors: Vec<Win32Monitor>,
    monitors_scratch: Vec<Win32Monitor>,
    /// `covered[i]` ↔ `monitors[i]`.
    covered: Vec<bool>,
    covered_mask: u64,
    presence: Presence,
    session_locked: bool,
    aggregate: DesktopVisibility,
    attachments: Vec<Attachment>,
    pending: Vec<DesktopEvent>,
    hooks: Option<WinEventHooks>,
    tray: TrayIcon,
}

impl Win32DesktopHost {
    /// Cria o host na thread atual (que deve rodar o loop). Um host por thread.
    pub fn new(config: HostConfig) -> PlatformResult<Self> {
        if signals::is_installed() {
            return Err(PlatformError::Os("já existe um Win32DesktopHost nesta thread".into()));
        }
        ensure_per_monitor_dpi_awareness();

        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|e| os_error("GetModuleHandleW", e))?
            .into();
        window::register_classes(hinstance)?;
        widget::register_class(hinstance)?;
        let controller = window::create_controller(hinstance)?;

        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        signals::install(
            controller,
            config.visibility_debounce_ms,
            config.monitors_debounce_ms,
            config.shell_restart_delay_ms,
            taskbar_created,
        );
        let wts_registered =
            unsafe { WTSRegisterSessionNotification(controller, NOTIFY_FOR_THIS_SESSION) }.is_ok();

        // A partir daqui o Drop limpa controlador, sinais e hooks em qualquer erro.
        let mut host = Self {
            config,
            hinstance,
            controller,
            own_pid: unsafe { GetCurrentProcessId() },
            wts_registered,
            layer: None,
            shell_retries: 0,
            monitors: Vec::new(),
            monitors_scratch: Vec::new(),
            covered: Vec::new(),
            covered_mask: 0,
            presence: Presence::Present,
            session_locked: false,
            aggregate: DesktopVisibility::Visible,
            attachments: Vec::new(),
            pending: Vec::with_capacity(EVENT_QUEUE_CAPACITY),
            hooks: None,
            tray: TrayIcon::new(controller),
        };

        enumerate(&mut host.monitors)?;
        host.monitors_scratch.reserve(host.monitors.len());
        host.covered.resize(host.monitors.len(), false);

        // Explorer ausente não é fatal: tenta de novo quando ele voltar.
        match workerw::discover(config.shell_message_timeout_ms) {
            Ok(layer) => host.layer = Some(layer),
            Err(_) => signals::schedule_ms(TIMER_SHELL, SHELL_RETRY_INTERVAL_MS),
        }

        host.hooks = WinEventHooks::install().ok();
        if host.hooks.is_none() {
            signals::schedule_ms(TIMER_VISIBILITY, FALLBACK_VISIBILITY_POLL_MS);
        }
        host.follow_foreground();
        host.evaluate_visibility(false);
        Ok(host)
    }

    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Proxy `Send + Sync` para acordar/encerrar o loop de outra thread.
    pub fn proxy(&self) -> LoopProxy {
        LoopProxy::new(self.controller)
    }

    /// Mostra ou oculta o ícone da engine na bandeja do sistema em tempo real.
    pub fn set_tray_visible(&mut self, visible: bool) {
        self.tray.set_visible(visible);
    }

    /// Atualiza o tooltip do ícone da bandeja conforme o estado de pausa.
    pub fn set_tray_paused(&mut self, paused: bool) {
        self.tray.set_paused(paused);
    }

    /// Exibe o menu de contexto nativo da bandeja e devolve a ação escolhida.
    pub fn show_tray_menu(&self, paused: bool) -> Option<TrayAction> {
        self.tray.show_context_menu(paused)
    }

    /// Monitores atuais (sem alocar).
    pub fn monitors(&self) -> &[Win32Monitor] {
        &self.monitors
    }

    pub fn monitor(&self, monitor_id: &str) -> Option<&Win32Monitor> {
        self.monitors.iter().find(|m| m.info.id == monitor_id)
    }

    /// Camada do Explorer em uso (`None` enquanto o shell estiver indisponível).
    pub fn desktop_layer(&self) -> Option<&DesktopLayer> {
        self.layer.as_ref()
    }

    /// Visibilidade de um monitor específico (pausar só a superfície coberta).
    pub fn monitor_visibility(&self, monitor_id: &str) -> DesktopVisibility {
        if self.session_locked || self.presence == Presence::Away {
            return DesktopVisibility::Locked;
        }
        match self.monitors.iter().position(|m| m.info.id == monitor_id) {
            Some(i) if self.covered.get(i).copied().unwrap_or(false) => DesktopVisibility::Covered,
            _ => DesktopVisibility::Visible,
        }
    }

    /// Visibilidade do monitor onde `window` está anexada.
    pub fn window_visibility(&self, window: NativeWindowId) -> DesktopVisibility {
        let hwnd = hwnd_from_id(window);
        match self.attachments.iter().find(|a| a.hwnd == hwnd) {
            Some(a) => self.monitor_visibility(&a.monitor_id),
            None => self.aggregate,
        }
    }

    /// Superfícies anexadas `(janela, monitor)`. Após `DesktopEvent::ShellRestarted` as
    /// janelas próprias podem ter sido recriadas: refazer as superfícies wgpu a partir daqui.
    pub fn wallpaper_windows(&self) -> impl Iterator<Item = (NativeWindowId, &str)> + '_ {
        self.attachments
            .iter()
            .map(|a| (id_from_hwnd(a.hwnd), a.monitor_id.as_str()))
    }

    /// Cria (ou reaproveita) a superfície de wallpaper do monitor e a anexa atrás dos ícones.
    /// A janela pertence ao host: é destruída em `detach_wallpaper` ou no `Drop`.
    pub fn create_wallpaper_window(&mut self, monitor_id: &str) -> PlatformResult<NativeWindowId> {
        if let Some(existing) = self
            .attachments
            .iter()
            .find(|a| a.owned && a.monitor_id == monitor_id && unsafe { IsWindow(a.hwnd) }.as_bool())
        {
            return Ok(id_from_hwnd(existing.hwnd));
        }

        let layer = self.ensure_layer()?;
        let index = self.monitor_index(monitor_id)?;
        let layered = self.layered_for(&layer);
        let hwnd = window::create_surface(self.hinstance)?;
        if let Err(err) = window::attach(hwnd, &layer, &self.monitors[index], layered) {
            let _guard = ExpectedDestroy::new();
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(err);
        }
        self.attachments.push(Attachment {
            hwnd,
            monitor_id: monitor_id.to_owned(),
            owned: true,
        });
        Ok(id_from_hwnd(hwnd))
    }

    /// Cria uma janela de widget nativo (independente dos monitores de wallpaper). O chamador é
    /// dono dela: soltar a superfície/presenter antes de soltar a janela.
    pub fn create_widget_window(&self, cfg: &WidgetWindowConfig) -> PlatformResult<WidgetWindow> {
        WidgetWindow::create(self.hinstance, cfg)
    }

    /// Presenter DirectComposition (alpha pré-multiplicado) para a janela de widget.
    pub fn create_widget_presenter(
        &self,
        gpu: &crate::engine_core::gpu::GpuContext,
        window: &WidgetWindow,
        width: u32,
        height: u32,
    ) -> PlatformResult<DCompPresenter> {
        DCompPresenter::new(gpu, window.hwnd(), width, height)
    }

    /// Último clique local em widget: `(janela, x, y)` em px do cliente.
    pub fn take_widget_click(&self) -> Option<(NativeWindowId, i32, i32)> {
        let (hwnd, x, y) = signals::take_widget_click();
        (hwnd != 0).then_some((NativeWindowId(hwnd), x, y))
    }

    /// Solta todas as superfícies de wallpaper (o Windows volta a pintar o próprio fundo).
    /// Superfícies wgpu delas devem ter caído antes.
    pub fn detach_all_wallpapers(&mut self) {
        let attachments = std::mem::take(&mut self.attachments);
        let had = !attachments.is_empty();
        for attachment in &attachments {
            self.release_attachment(attachment);
        }
        if had && self.config.refresh_wallpaper_on_detach {
            window::refresh_system_wallpaper();
        }
    }

    /// Move os eventos pendentes para `out` sem alocar (troca de buffers).
    pub(crate) fn drain_events_into(&mut self, out: &mut Vec<DesktopEvent>) {
        out.clear();
        std::mem::swap(&mut self.pending, out);
    }

    /// Converte sinais dos callbacks em estado + eventos. Retorna os bits que o loop
    /// entrega direto ao handler (`SIG_WAKE`, `SIG_REPAINT`).
    pub(crate) fn process_signals(&mut self) -> u32 {
        let sig = signals::take();
        if sig == 0 {
            return 0;
        }

        if sig & SIG_SESSION != 0 {
            self.session_locked = signals::session_locked();
        }
        if sig & SIG_FOREGROUND != 0 {
            self.follow_foreground();
        }
        if sig & SIG_WINDOW_LOST != 0 {
            // Explorer destruiu nossas superfícies: a camada antiga não serve mais.
            // A reanexação vem pelo TIMER_SHELL (já armado pelo wndproc).
            self.layer = None;
        }

        let mut reevaluate = sig & (SIG_VISIBILITY | SIG_FOREGROUND | SIG_SESSION) != 0;
        if sig & SIG_MONITORS != 0 && self.refresh_monitors() {
            reevaluate = true;
        }
        if sig & SIG_TASKBAR_CREATED != 0 {
            self.tray.restore_if_visible();
        }
        if sig & SIG_SHELL_RETRY != 0 {
            self.reattach_after_shell_change();
            reevaluate = true;
        }
        if reevaluate {
            self.evaluate_visibility(true);
        }
        if self.hooks.is_none() {
            signals::schedule_ms(TIMER_VISIBILITY, FALLBACK_VISIBILITY_POLL_MS);
        }
        sig & LOOP_SIGNALS
    }

    // ───────────── internos ─────────────

    fn monitor_index(&self, monitor_id: &str) -> PlatformResult<usize> {
        self.monitors
            .iter()
            .position(|m| m.info.id == monitor_id)
            .ok_or_else(|| PlatformError::Os(format!("monitor desconhecido: {monitor_id}")))
    }

    fn layered_for(&self, layer: &DesktopLayer) -> bool {
        layer.layout == ShellLayout::Raised && self.config.layered_on_raised_desktop
    }

    fn ensure_layer(&mut self) -> PlatformResult<DesktopLayer> {
        if let Some(layer) = self.layer {
            if layer.is_alive() {
                return Ok(layer);
            }
        }
        let layer = workerw::discover(self.config.shell_message_timeout_ms)?;
        self.layer = Some(layer);
        Ok(layer)
    }

    fn follow_foreground(&mut self) {
        let own_pid = self.own_pid;
        let Some(hooks) = self.hooks.as_mut() else {
            return;
        };
        let pid = unsafe {
            let fg = GetForegroundWindow();
            if fg.is_invalid() {
                0
            } else {
                window_pid(fg)
            }
        };
        hooks.follow_process(pid, own_pid);
    }

    /// Reenumera monitores; se o layout mudou, reposiciona superfícies e emite
    /// `MonitorsChanged`. Retorna se mudou.
    fn refresh_monitors(&mut self) -> bool {
        if enumerate(&mut self.monitors_scratch).is_err() {
            return false;
        }
        let changed = !same_layout(&self.monitors, &self.monitors_scratch);
        // Sempre adota a lista nova: HMONITOR pode mudar sem o layout mudar.
        std::mem::swap(&mut self.monitors, &mut self.monitors_scratch);
        self.covered.resize(self.monitors.len(), false);
        if changed {
            // O Explorer às vezes recria o WorkerW em troca de resolução/monitor.
            if !self.layer.is_some_and(|l| l.is_alive()) {
                self.layer = workerw::discover(self.config.shell_message_timeout_ms).ok();
            }
            self.reposition_all();
            self.pending.push(DesktopEvent::MonitorsChanged);
        }
        changed
    }

    fn reposition_all(&mut self) {
        let Some(layer) = self.layer else {
            return;
        };
        let layered = self.layered_for(&layer);
        for a in &self.attachments {
            match self.monitors.iter().find(|m| m.info.id == a.monitor_id) {
                Some(m) => {
                    let _ = window::attach(a.hwnd, &layer, m, layered);
                }
                // Monitor desconectado: esconde; volta ao reconectar.
                None => window::hide(a.hwnd),
            }
        }
    }

    /// Explorer reiniciou ou destruiu nossas janelas: redescobre a camada, recria as
    /// superfícies próprias perdidas e reanexa tudo. Emite `ShellRestarted` se algo mudou.
    fn reattach_after_shell_change(&mut self) {
        let layer = match workerw::discover(self.config.shell_message_timeout_ms) {
            Ok(layer) => layer,
            Err(_) => {
                if self.shell_retries < SHELL_MAX_RETRIES {
                    self.shell_retries += 1;
                    signals::schedule_ms(TIMER_SHELL, SHELL_RETRY_INTERVAL_MS);
                }
                return;
            }
        };
        self.shell_retries = 0;
        let mut changed = self.layer != Some(layer);
        self.layer = Some(layer);
        let layered = self.layered_for(&layer);

        for a in self.attachments.iter_mut() {
            if !unsafe { IsWindow(a.hwnd) }.as_bool() {
                if !a.owned {
                    changed = true;
                    continue; // janela de terceiros morreu: removida abaixo
                }
                match window::create_surface(self.hinstance) {
                    Ok(hwnd) => {
                        a.hwnd = hwnd;
                        changed = true;
                    }
                    Err(_) => continue,
                }
            }
            match self.monitors.iter().find(|m| m.info.id == a.monitor_id) {
                Some(m) => {
                    let _ = window::attach(a.hwnd, &layer, m, layered);
                }
                None => window::hide(a.hwnd),
            }
        }
        self.attachments.retain(|a| unsafe { IsWindow(a.hwnd) }.as_bool());

        if changed {
            self.pending.push(DesktopEvent::ShellRestarted);
        }
    }

    fn evaluate_visibility(&mut self, emit: bool) {
        self.covered.resize(self.monitors.len(), false);
        self.presence = visibility::scan(&self.monitors, &mut self.covered, self.own_pid);

        let mask = self
            .covered
            .iter()
            .take(64)
            .enumerate()
            .fold(0u64, |acc, (i, &c)| if c { acc | (1u64 << i) } else { acc });
        let all_covered = !self.covered.is_empty() && self.covered.iter().all(|&c| c);
        let aggregate = if self.session_locked || self.presence == Presence::Away {
            DesktopVisibility::Locked
        } else if all_covered {
            DesktopVisibility::Covered
        } else {
            DesktopVisibility::Visible
        };

        // Emite também quando só a cobertura por monitor muda (renderer consulta
        // `monitor_visibility` para pausar superfícies individuais).
        let changed = aggregate != self.aggregate || mask != self.covered_mask;
        self.aggregate = aggregate;
        self.covered_mask = mask;
        if emit && changed {
            self.pending.push(DesktopEvent::VisibilityChanged(aggregate));
        }
    }

    fn release_attachment(&self, attachment: &Attachment) {
        unsafe {
            if !IsWindow(attachment.hwnd).as_bool() {
                return;
            }
            if attachment.owned {
                let _guard = ExpectedDestroy::new();
                let _ = DestroyWindow(attachment.hwnd);
            } else {
                window::detach(attachment.hwnd);
            }
        }
    }
}

impl DesktopHost for Win32DesktopHost {
    fn list_monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        Ok(self.monitors.iter().map(|m| m.info.clone()).collect())
    }

    /// Anexa uma janela externa (ex.: criada por outro módulo) como wallpaper do monitor.
    fn attach_wallpaper(&mut self, window: NativeWindowId, monitor_id: &str) -> PlatformResult<()> {
        let hwnd = hwnd_from_id(window);
        if hwnd.is_invalid() || !unsafe { IsWindow(hwnd) }.as_bool() {
            return Err(PlatformError::Os("janela inválida".into()));
        }
        let layer = self.ensure_layer()?;
        let index = self.monitor_index(monitor_id)?;
        let layered = self.layered_for(&layer);
        window::attach(hwnd, &layer, &self.monitors[index], layered)?;

        match self.attachments.iter_mut().find(|a| a.hwnd == hwnd) {
            Some(a) => {
                a.monitor_id.clear();
                a.monitor_id.push_str(monitor_id);
            }
            None => self.attachments.push(Attachment {
                hwnd,
                monitor_id: monitor_id.to_owned(),
                owned: false,
            }),
        }
        Ok(())
    }

    fn detach_wallpaper(&mut self, window: NativeWindowId) -> PlatformResult<()> {
        let hwnd = hwnd_from_id(window);
        let index = self
            .attachments
            .iter()
            .position(|a| a.hwnd == hwnd)
            .ok_or_else(|| PlatformError::Os("janela não anexada".into()))?;
        let attachment = self.attachments.swap_remove(index);
        self.release_attachment(&attachment);
        if self.attachments.is_empty() && self.config.refresh_wallpaper_on_detach {
            window::refresh_system_wallpaper();
        }
        Ok(())
    }

    fn visibility(&self) -> PlatformResult<DesktopVisibility> {
        Ok(self.aggregate)
    }

    fn cursor_position(&self) -> PlatformResult<(i32, i32)> {
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt) }.map_err(|e| os_error("GetCursorPos", e))?;
        Ok((pt.x, pt.y))
    }

    /// Para loops próprios (fora de `event_loop::run`): processa sinais e drena eventos.
    /// As mensagens Win32 precisam estar sendo bombeadas nesta thread.
    fn poll_events(&mut self) -> Vec<DesktopEvent> {
        let loop_bits = self.process_signals();
        if loop_bits != 0 {
            signals::raise(loop_bits); // preserva wake/repaint para quem usa `run`
        }
        std::mem::take(&mut self.pending)
    }
}

impl Drop for Win32DesktopHost {
    fn drop(&mut self) {
        self.tray.hide();
        let had_attachments = !self.attachments.is_empty();
        let attachments = std::mem::take(&mut self.attachments);
        for attachment in &attachments {
            self.release_attachment(attachment);
        }
        self.hooks = None; // UnhookWinEvent na mesma thread
        unsafe {
            for timer in ALL_TIMERS {
                let _ = KillTimer(self.controller, timer);
            }
            if self.wts_registered {
                let _ = WTSUnRegisterSessionNotification(self.controller);
            }
            let _ = DestroyWindow(self.controller);
        }
        signals::reset();
        if had_attachments && self.config.refresh_wallpaper_on_detach {
            window::refresh_system_wallpaper();
        }
    }
}
