//! Modo nativo: `prismview-engine.exe --native`.
//!
//! Sobe o `Win32DesktopHost` (WorkerW), uma superfície wgpu por monitor (`core::gpu`) e o
//! servidor IPC do Studio (named pipe `IPC_PIPE_NAME`). Fase 0: `CLEAR_DESKTOP` pinta cor sólida.
//! Fase 1: `LOAD_SCENE` desenha a `PrismScene` com o compositor 2D (`core::scene2d`) e texto
//! glyphon (`core::text`); vidro/3D entram nas próximas fases.
//!
//! Loop: `ControlFlow::Wait` quando nada mudou (0% CPU); só renderiza sob evento (IPC,
//! repaint do sistema, volta de visibilidade, rebuild) e com pausa por monitor coberto. Cena com
//! relógio acorda só no próximo segundo/minuto (`SceneRenderer::next_wake`), e apenas se algum
//! monitor está visível.

use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use chrono::Local;
use prism_core::fx::EvalContext;
use prism_core::schema::{EngineStatus, IpcRequest, IpcResponse, PrismScene, MANIFEST_VERSION};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle};

use crate::engine_core::compositor::Canvas;
use crate::engine_core::gpu::{FrameStatus, GpuContext, NativeHandles, WindowSurface};
use crate::engine_core::scene2d::SceneRenderer;
use crate::platform::windows::{
    run as run_loop, ControlFlow, HostConfig, LoopHandler, Win32DesktopHost,
};
use crate::platform::{DesktopEvent, DesktopVisibility, NativeWindowId};
use crate::{ipc_server, IpcEnvelope};

/// Cor padrão do `CLEAR_DESKTOP` sem argumento: preto opaco.
const DEFAULT_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// Intervalo entre tentativas quando a swapchain precisou ser refeita.
const RETRY_INTERVAL: Duration = Duration::from_millis(16);
/// Tentativas seguidas de `Retry` antes de refazer janelas/superfícies.
const MAX_RETRY_STREAK: u32 = 60;
/// Espera antes de refazer janelas/superfícies após falha.
const REBUILD_DELAY: Duration = Duration::from_millis(250);
/// Rebuilds automáticos seguidos sem sucesso; depois espera um evento do sistema.
const MAX_REBUILD_ATTEMPTS: u32 = 20;

struct Output {
    monitor_id: String,
    /// Fator de DPI do monitor (px lógicos → físicos).
    scale: f32,
    surface: WindowSurface,
    canvas: Canvas,
}

struct NativeApp {
    ipc_rx: Receiver<IpcEnvelope>,
    gpu: Option<GpuContext>,
    /// Compositor 2D + texto; depende de `gpu` (cai junto com ele).
    renderer: Option<SceneRenderer>,
    /// Cena atual (cópia em CPU para recriar o renderer após device perdido).
    scene: Option<PrismScene>,
    ctx: EvalContext,
    outputs: Vec<Output>,
    clear_color: [f32; 4],
    dirty: bool,
    paused: bool,
    exit: bool,
    frames: u64,
    retry_streak: u32,
    retry_at: Option<Instant>,
    rebuild_at: Option<Instant>,
    rebuild_attempts: u32,
}

/// Ponto de entrada do modo nativo. Retorna o código de saída do processo.
pub fn run() -> i32 {
    // `host` declarado antes de `app`: `app` (superfícies wgpu) cai primeiro e só então
    // o host destrói as janelas.
    let mut host = match Win32DesktopHost::new(HostConfig::default()) {
        Ok(host) => host,
        Err(err) => {
            eprintln!("[prismview-engine] host Win32: {err}");
            return 1;
        }
    };

    let proxy = host.proxy();
    let (tx, rx) = mpsc::channel();
    ipc_server::spawn_named_pipe_server(
        move || {
            let _ = proxy.wake();
        },
        tx,
    );

    let mut app = NativeApp::new(rx);
    app.rebuild_outputs(&mut host);
    run_loop(&mut host, &mut app)
}

fn handles_for(window: NativeWindowId) -> Option<NativeHandles> {
    let hwnd = NonZeroIsize::new(window.0)?;
    Some(NativeHandles {
        window: RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)),
        display: RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
    })
}

fn sanitize_color(color: [f32; 4]) -> [f32; 4] {
    color.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 })
}

impl NativeApp {
    fn new(ipc_rx: Receiver<IpcEnvelope>) -> Self {
        Self {
            ipc_rx,
            gpu: None,
            renderer: None,
            scene: None,
            ctx: EvalContext::default(),
            outputs: Vec::new(),
            clear_color: DEFAULT_CLEAR,
            dirty: false,
            paused: false,
            exit: false,
            frames: 0,
            retry_streak: 0,
            retry_at: None,
            rebuild_at: None,
            rebuild_attempts: 0,
        }
    }

    /// Solta superfícies (antes das janelas) e, se `with_gpu`, o device.
    fn release_gpu_resources(&mut self, with_gpu: bool) {
        // Ordem: superfícies/canvases → renderer → device.
        self.outputs.clear();
        if with_gpu {
            self.renderer = None;
            self.gpu = None;
        }
    }

    /// Pastas de fontes do projeto (`%APPDATA%\PrismView\fonts`).
    fn font_dirs() -> Vec<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|appdata| vec![PathBuf::from(appdata).join("PrismView").join("fonts")])
            .unwrap_or_default()
    }

    /// Cria o `SceneRenderer` (se ainda não existe) com o formato da swapchain.
    fn ensure_renderer(&mut self, format: wgpu::TextureFormat) {
        if self.renderer.is_some() {
            return;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let mut renderer = SceneRenderer::new(gpu, format);
        for dir in Self::font_dirs() {
            if dir.is_dir() {
                renderer.register_font_dir(&dir);
            }
        }
        if let Some(scene) = self.scene.clone() {
            self.ctx.now = Local::now();
            renderer.set_scene(scene, &self.ctx);
        }
        self.renderer = Some(renderer);
    }

    /// (Re)cria janela + superfície wgpu para cada monitor atual. Raro (início, shell
    /// reiniciado, monitores mudaram, device perdido): pode alocar.
    fn rebuild_outputs(&mut self, host: &mut Win32DesktopHost) {
        self.release_gpu_resources(false);
        self.rebuild_at = None;

        let monitors: Vec<(String, u32, u32, f32)> = host
            .monitors()
            .iter()
            .map(|m| (m.info.id.clone(), m.info.width, m.info.height, m.info.scale_factor))
            .collect();

        let mut pending: Vec<(String, f32, WindowSurface)> = Vec::with_capacity(monitors.len());
        for (monitor_id, width, height, scale) in monitors {
            let Ok(window) = host.create_wallpaper_window(&monitor_id) else {
                continue;
            };
            let Some(handles) = handles_for(window) else {
                continue;
            };

            if self.gpu.is_none() {
                match GpuContext::new(handles, width, height) {
                    Ok((gpu, surface)) => {
                        self.gpu = Some(gpu);
                        pending.push((monitor_id, scale, surface));
                    }
                    Err(err) => eprintln!("[prismview-engine] gpu: {err}"),
                }
                continue;
            }
            let Some(gpu) = self.gpu.as_ref() else {
                continue;
            };
            match gpu.create_surface(handles, width, height) {
                Ok(surface) => pending.push((monitor_id, scale, surface)),
                Err(err) => eprintln!("[prismview-engine] superfície {monitor_id}: {err}"),
            }
        }

        if let Some(format) = pending.first().map(|(_, _, s)| s.format()) {
            self.ensure_renderer(format);
        }
        if let (Some(gpu), Some(renderer)) = (self.gpu.as_ref(), self.renderer.as_ref()) {
            for (monitor_id, scale, surface) in pending {
                let canvas = renderer.create_canvas(gpu);
                self.outputs.push(Output {
                    monitor_id,
                    scale,
                    surface,
                    canvas,
                });
            }
        }

        if self.outputs.is_empty() {
            self.schedule_rebuild();
        } else {
            self.rebuild_attempts = 0;
        }
        self.dirty = true;
        self.retry_at = None;
        self.retry_streak = 0;
    }

    fn schedule_rebuild(&mut self) {
        if self.rebuild_attempts < MAX_REBUILD_ATTEMPTS {
            self.rebuild_attempts += 1;
            self.rebuild_at = Some(Instant::now() + REBUILD_DELAY);
        }
    }

    /// Um passe de clear em todos os monitores visíveis. Sem alocação.
    fn render(&mut self, host: &Win32DesktopHost) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let color = self.clear_color;
        let mut retry = false;
        let mut rebuild = false;
        let mut device_lost = false;
        let mut renderer = self.renderer.as_mut().filter(|r| r.has_scene());

        for output in self.outputs.iter_mut() {
            if host.monitor_visibility(&output.monitor_id) != DesktopVisibility::Visible {
                continue; // coberto/bloqueado: volta no VisibilityChanged
            }
            let status = match renderer.as_deref_mut() {
                Some(r) => r.render(
                    gpu,
                    &mut output.surface,
                    &mut output.canvas,
                    &output.monitor_id,
                    output.scale,
                    &self.ctx,
                    color,
                ),
                None => output.surface.clear(gpu, color),
            };
            match status {
                FrameStatus::Presented => self.frames += 1,
                FrameStatus::Retry => retry = true,
                FrameStatus::Unavailable => rebuild = true,
                FrameStatus::DeviceLost => device_lost = true,
            }
        }

        if device_lost {
            self.release_gpu_resources(true);
            self.rebuild_attempts = 0;
            self.schedule_rebuild();
        } else if rebuild {
            self.schedule_rebuild();
        } else if retry {
            self.retry_streak += 1;
            if self.retry_streak > MAX_RETRY_STREAK {
                self.retry_streak = 0;
                self.schedule_rebuild();
            } else {
                self.dirty = true;
                self.retry_at = Some(Instant::now() + RETRY_INTERVAL);
            }
        } else {
            self.retry_streak = 0;
        }
    }

    fn handle_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        match request {
            IpcRequest::Ping => IpcResponse::Pong {
                protocol_version: MANIFEST_VERSION,
            },
            IpcRequest::GetStatus => IpcResponse::Status {
                engine: EngineStatus {
                    running: true,
                    paused: self.paused,
                    pause_reason: self.paused.then(|| "ipc".to_string()),
                    active_wallpaper_id: self.scene.as_ref().map(|s| s.id.clone()),
                    pinned_widget_ids: Vec::new(),
                    active_monitor_count: u32::try_from(self.outputs.len()).unwrap_or(u32::MAX),
                    rendered_frames: self.frames,
                    gpu_adapter: self
                        .gpu
                        .as_ref()
                        .map_or_else(|| "Unavailable".to_string(), |g| g.adapter_name().to_string()),
                },
            },
            IpcRequest::LoadScene { scene, .. } => {
                if self.gpu.is_none() || self.outputs.is_empty() {
                    return IpcResponse::Error {
                        message: "gpu_unavailable".to_string(),
                    };
                }
                let scene = *scene;
                self.ctx.now = Local::now();
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_scene(scene.clone(), &self.ctx);
                }
                self.scene = Some(scene);
                self.dirty = true;
                self.retry_at = None;
                IpcResponse::Ok
            }
            IpcRequest::ClearDesktop { color } => {
                if self.gpu.is_none() || self.outputs.is_empty() {
                    return IpcResponse::Error {
                        message: "gpu_unavailable".to_string(),
                    };
                }
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.clear_scene();
                }
                self.scene = None;
                self.clear_color = color.map_or(DEFAULT_CLEAR, sanitize_color);
                self.dirty = true;
                self.retry_at = None;
                IpcResponse::Ok
            }
            IpcRequest::SetPaused { paused } => {
                self.paused = paused;
                if !paused {
                    self.dirty = true;
                }
                IpcResponse::Ok
            }
            IpcRequest::SetStudioPreviewMode { .. } => IpcResponse::Ok,
            IpcRequest::Shutdown => {
                self.exit = true;
                IpcResponse::Ok
            }
            _ => IpcResponse::Error {
                message: "native_mode_unsupported_command".to_string(),
            },
        }
    }
}

impl LoopHandler for NativeApp {
    fn desktop_event(&mut self, host: &mut Win32DesktopHost, event: DesktopEvent) {
        match event {
            DesktopEvent::ShellRestarted | DesktopEvent::MonitorsChanged => {
                self.rebuild_attempts = 0;
                self.rebuild_outputs(host);
            }
            DesktopEvent::VisibilityChanged(_) => self.dirty = true,
        }
    }

    fn woken(&mut self, _host: &mut Win32DesktopHost) {
        while let Ok(envelope) = self.ipc_rx.try_recv() {
            let response = self.handle_ipc(envelope.req);
            let _ = envelope.reply.send(response);
        }
    }

    fn repaint_requested(&mut self, _host: &mut Win32DesktopHost) {
        self.dirty = true;
    }

    fn about_to_wait(&mut self, host: &mut Win32DesktopHost) -> ControlFlow {
        if self.exit {
            return ControlFlow::Exit;
        }

        let now = Instant::now();
        if self.rebuild_at.is_some_and(|at| now >= at) {
            self.rebuild_outputs(host);
        }

        // Relógio da cena: só invalida (e acorda) quando `$df(..)$` muda de fato.
        self.ctx.now = Local::now();
        if let Some(renderer) = self.renderer.as_mut() {
            if renderer.advance_clock(&self.ctx.now) {
                self.dirty = true;
            }
        }

        let due = self.retry_at.map_or(true, |at| now >= at);
        if self.dirty && !self.paused && due {
            self.dirty = false;
            self.retry_at = None;
            self.render(host);
        }

        // Próximo tique do relógio, só se há monitor visível e não está pausado.
        let clock_at = if self.paused {
            None
        } else {
            let any_visible = self
                .outputs
                .iter()
                .any(|o| host.monitor_visibility(&o.monitor_id) == DesktopVisibility::Visible);
            if any_visible {
                self.renderer
                    .as_ref()
                    .and_then(|r| r.next_wake(&self.ctx.now))
                    .map(|d| Instant::now() + d)
            } else {
                None
            }
        };
        let next = [self.retry_at, self.rebuild_at, clock_at]
            .into_iter()
            .flatten()
            .min();
        match next {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        }
    }

    fn exiting(&mut self, _host: &mut Win32DesktopHost) {
        // Superfícies e device antes de o host destruir as janelas.
        self.release_gpu_resources(true);
    }
}
