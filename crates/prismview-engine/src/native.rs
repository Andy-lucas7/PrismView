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
//!
//! Fase 4: `Win32SystemSensors` alimenta o `SignalBus`; a demanda vem da cena (bindings 2D/vidro +
//! controles de mídia tocáveis). Por iteração do loop: `SignalReader::update` → bindings
//! (`SceneRenderer::apply_signals`) → toque no desktop (`DESKTOP_TAP`) testado contra as regiões
//! da saída do monitor → `media_control` (SMTC). Sem demanda, sensores dormem e o loop também.
//!
//! Fase 5: `PowerGovernor` (`core::governor`) decide por iteração entre Normal/Eco/Ultra/Pausado/
//! Congelado (bateria, monitor coberto/desligado, pausa do Studio). Aqui só se aplicam as decisões:
//! teto de FPS (`FrameGate`), pós-processamento (blur do vidro), demanda dos sensores e a
//! hibernação de VRAM (60 s pausado → solta pirâmide de blur; volta sob demanda no 1º quadro).
//!
//! 3D: a primeira camada `Scene3d` da cena alimenta um `Scene3dRenderer` (1 por device). Por
//! iteração: eventos do `AssetLoader` → cursor/sinais → `update` no teto de FPS da cena (e do
//! governador) → `render` num encoder próprio, submetido antes do passe 2D → a `output_view` entra
//! no compositor como textura da camada (`SceneRenderer::set_scene3d_output`, só quando a view
//! muda). Toques no desktop caem no `click` (CLICK_MESH) se nenhum controle 2D os consumiu.
//! Hibernação: `release_targets` + o compositor solta o bind (libera a VRAM de fato).
//!
//! Vídeo: um `MfVideoDecoder` (zero-copy D3D11→D3D12) por arquivo de vídeo da cena (fundo e
//! camadas `Video`); por iteração `advance` → `SceneRenderer::set_video_frame`. Contexto FX:
//! globais da cena, mídia (texto/estado/progresso SMTC) e telemetria vêm do `SignalBus`, e só
//! invalidam a lista quando mudam e a cena depende deles (`$mi()$`, `$sys()$`...). Toques
//! não-mídia (`DesktopAction::Custom`): abrir URL/app (`shell_open`), alternar/definir globais.

use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use prism_core::fx::{EvalContext, SystemTelemetry};
use prism_core::schema::{
    DirtyFlags, EngineStatus, IpcRequest, IpcResponse, PrismScene, TouchActionType, MANIFEST_VERSION,
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle};

use crate::engine_core::assets::{AssetLoader, AssetVault, LoadEvent};
use crate::engine_core::compositor::{Canvas, Scene3dPlacement};
use crate::engine_core::governor::{
    FrameGate, GovernorInputs, GovernorUpdate, HibernateAction, PowerGovernor, PowerPreference,
    PowerState,
};
use crate::engine_core::gpu::{FrameStatus, GpuContext, NativeHandles, WindowSurface};
use crate::engine_core::interaction::{tap_to_local, DesktopAction};
use crate::engine_core::scene2d::SceneRenderer;
use crate::engine_core::scene3d::{Scene3dConfig, Scene3dOptions, Scene3dRenderer, Scene3dShared};
use crate::engine_core::signals::{MediaText, SignalBus, SignalKind, SignalReader, SignalSnapshot};
use crate::platform::windows::{
    run as run_loop, shell_open, ControlFlow, HostConfig, LoopHandler, LoopProxy, MfVideoDecoder,
    SensorsConfig, Win32DesktopHost, Win32SystemSensors,
};
use crate::platform::{
    DesktopEvent, DesktopVisibility, HardwareVideoDecoder, NativeWindowId, SystemSensors,
};
use crate::ipc_server::{self, IpcEnvelope};

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
/// Cadência enquanto molas de bindings ainda se movem.
const BINDING_FRAME: Duration = Duration::from_millis(16);
/// Cena persistida pelo Studio (`LoadSceneFile`); relida ao iniciar sem o Studio.
const ACTIVE_SCENE_FILE: &str = "active_scene.json";
/// Preferência de energia persistida pelo motor (`SetPowerPreference`).
const POWER_PREFERENCE_FILE: &str = "power_preference.txt";
/// Limite defensivo do JSON da cena.
const MAX_SCENE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// `output_generation` ainda não registrada no compositor.
const SCENE3D_UNBOUND: u64 = u64::MAX;

/// Decoder de um arquivo de vídeo da cena.
struct VideoSlot {
    path: PathBuf,
    decoder: MfVideoDecoder,
    paused: bool,
}

struct Output {
    monitor_id: String,
    /// Canto superior esquerdo do monitor no desktop virtual (px físicos), para mapear toques.
    origin: (i32, i32),
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
    /// Sensores (áudio/entrada/mídia/telemetria) + leitura por quadro do bus.
    sensors: Option<Win32SystemSensors>,
    signal_reader: SignalReader,
    snapshot: SignalSnapshot,
    last_signals_at: Option<Instant>,
    /// Fase 5: governador de energia + preferência escolhida no Studio.
    governor: PowerGovernor,
    preference: PowerPreference,
    /// Demanda da cena atual (sem `POWER`, que o governador sempre pede).
    scene_demand: SignalKind,
    /// Acorda o loop a partir de threads (loader de modelos, HDRI).
    proxy: Option<LoopProxy>,
    /// Cena 3D: cache de assets (sobrevive a device perdido), recursos por device e renderer.
    vault: Arc<AssetVault>,
    loader: Option<AssetLoader>,
    scene3d_shared: Option<Arc<Scene3dShared>>,
    scene3d: Option<Scene3dRenderer>,
    /// Scratch dos eventos do loader (reaproveitado).
    load_events: Vec<LoadEvent>,
    /// `output_generation` já ligada ao compositor (`SCENE3D_UNBOUND` = nenhuma).
    scene3d_bound_gen: u64,
    scene3d_last_update: Option<Instant>,
    /// Próximo quadro 3D permitido (teto da cena × governador).
    scene3d_next_at: Option<Instant>,
    /// Vídeos da cena (fundo + camadas).
    videos: Vec<VideoSlot>,
    /// Texto de mídia (scratch reaproveitado) e versões já aplicadas ao `ctx`.
    media_text: MediaText,
    media_version_seen: u64,
    /// RAM total (GB) amostrada 1× no início (`rm(ram_total_gb)`, `rm(ram_gb)` = total × %).
    ram_total_gb: f32,
}

/// Ponto de entrada do modo nativo. Retorna o código de saída do processo.
pub fn run() -> i32 {
    // Login do Windows (`--native --login`, registrado pelo Studio): sem cena aplicada, sai na hora
    // em vez de cobrir o wallpaper do sistema com a cor de clear.
    let at_login = std::env::args().skip(1).any(|a| a == "--login");
    let has_scene = NativeApp::state_dir().is_some_and(|dir| dir.join(ACTIVE_SCENE_FILE).is_file());
    if at_login && !has_scene {
        return 0;
    }

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
    app.proxy = Some(host.proxy());
    let bus = Arc::new(SignalBus::new());
    match Win32SystemSensors::new(bus, SensorsConfig::default(), Some(host.proxy())) {
        Ok(sensors) => app.sensors = Some(sensors),
        Err(err) => eprintln!("[prismview-engine] sensores: {err}"),
    }
    app.restore_persisted_state();
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
            sensors: None,
            signal_reader: SignalReader::new(),
            snapshot: SignalSnapshot::default(),
            last_signals_at: None,
            governor: PowerGovernor::new(),
            preference: PowerPreference::Auto,
            scene_demand: SignalKind::NONE,
            proxy: None,
            vault: AssetVault::new(),
            loader: None,
            scene3d_shared: None,
            scene3d: None,
            load_events: Vec::with_capacity(8),
            scene3d_bound_gen: SCENE3D_UNBOUND,
            scene3d_last_update: None,
            scene3d_next_at: None,
            videos: Vec::new(),
            media_text: MediaText::default(),
            media_version_seen: u64::MAX,
            ram_total_gb: SystemTelemetry::sample_live().ram_total_gb,
        }
    }

    // ── Cena 3D ──

    /// (Re)lê o `Scene3DConfig` da primeira camada 3D da cena atual e aplica ao renderer 3D
    /// (criando-o sob demanda). Cena sem 3D = renderer e saída descartados. Raro: pode alocar.
    fn refresh_scene3d(&mut self) {
        let config = self
            .renderer
            .as_ref()
            .and_then(SceneRenderer::scene3d_config)
            .and_then(|raw| match serde_json::from_value::<Scene3dConfig>(raw.clone()) {
                Ok(cfg) => Some(cfg),
                Err(err) => {
                    eprintln!("[prismview-engine] scene3d inválida: {err}");
                    None
                }
            });
        let Some(config) = config else {
            self.drop_scene3d();
            return;
        };
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let shared = match self.scene3d_shared.as_ref() {
            Some(shared) => Arc::clone(shared),
            None => {
                let shared = Scene3dShared::new(gpu);
                self.scene3d_shared = Some(Arc::clone(&shared));
                shared
            }
        };
        let proxy = self.proxy;
        if self.loader.is_none() {
            self.loader = Some(AssetLoader::new(Arc::clone(&self.vault), shared.loader_config(), move || {
                if let Some(proxy) = proxy {
                    let _ = proxy.wake();
                }
            }));
        }
        if self.scene3d.is_none() {
            let mut scene3d = Scene3dRenderer::new(shared, gpu, Scene3dOptions::default());
            scene3d.set_render_scale_cap(self.governor.caps().render_scale_cap);
            if let Some(proxy) = proxy {
                scene3d.set_waker(Some(Arc::new(move || {
                    let _ = proxy.wake();
                })));
            }
            self.scene3d = Some(scene3d);
            self.scene3d_bound_gen = SCENE3D_UNBOUND;
        }
        if let (Some(scene3d), Some(loader)) = (self.scene3d.as_mut(), self.loader.as_mut()) {
            scene3d.set_config(gpu, config, loader);
        }
        self.scene3d_last_update = None;
        self.scene3d_next_at = None;
        self.dirty = true;
    }

    /// Descarta o renderer 3D (cena sem 3D) e o bind da saída no compositor.
    fn drop_scene3d(&mut self) {
        if self.scene3d.take().is_some() {
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.clear_scene3d_output();
            }
        }
        self.scene3d_bound_gen = SCENE3D_UNBOUND;
        self.scene3d_last_update = None;
        self.scene3d_next_at = None;
    }

    /// Caixa da camada 3D na primeira saída visível que a desenhou.
    fn scene3d_placement(&self, host: &Win32DesktopHost) -> Option<Scene3dPlacement> {
        self.outputs.iter().find_map(|o| {
            let placement = SceneRenderer::scene3d_placement(&o.canvas)?;
            (host.monitor_visibility(&o.monitor_id) == DesktopVisibility::Visible).then_some(placement)
        })
    }

    /// Ponto do desktop virtual (px físicos) → 0..1 da camada 3D na saída que o contém.
    fn scene3d_local(outputs: &[Output], point: (i32, i32)) -> Option<[f32; 2]> {
        outputs.iter().find_map(|o| {
            let local = tap_to_local(point, o.origin, o.surface.size())?;
            SceneRenderer::scene3d_placement(&o.canvas)?.to_local(local)
        })
    }

    /// Eventos do loader, `update` e `render` do 3D no teto de FPS. Devolve quando acordar para
    /// o próximo quadro 3D (`None` = só por evento). Sem alocação além do encoder do quadro.
    fn step_scene3d(&mut self, host: &Win32DesktopHost, now: Instant) -> Option<Instant> {
        let placement = self.scene3d_placement(host);
        let gpu = self.gpu.as_ref()?;
        let scene3d = self.scene3d.as_mut()?;

        if let Some(loader) = self.loader.as_mut() {
            loader.poll(&mut self.load_events);
            for event in &self.load_events {
                scene3d.handle_load_event(gpu, event);
            }
            self.load_events.clear();
        }

        // Pausado/congelado: tempo da cena parado; nada é desenhado.
        if !self.governor.animations_allowed() {
            self.scene3d_last_update = None;
            return None;
        }
        // Sem caixa ainda (lista 2D não construída ou monitores cobertos): espera o próximo
        // rebuild/visibilidade (o loop volta via `needs_frame` em `about_to_wait`).
        let placement = placement?;
        let (w, h) = placement.pixel_size();
        scene3d.set_viewport(w, h);

        let cadence = scene3d
            .frame_interval()
            .unwrap_or(Duration::ZERO)
            .max(self.governor.frame_interval());
        if let Some(at) = self.scene3d_next_at {
            if now < at {
                return Some(at);
            }
        }

        let dt = self
            .scene3d_last_update
            .map_or(1.0 / 60.0, |at| now.saturating_duration_since(at).as_secs_f32());
        self.scene3d_last_update = Some(now);

        if scene3d.update(gpu, dt) {
            let mut encoder = gpu.device().create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismview_scene3d"),
            });
            if scene3d.render(gpu, &mut encoder) {
                gpu.queue().submit(Some(encoder.finish()));
                let generation = scene3d.output_generation();
                if generation != self.scene3d_bound_gen {
                    if let (Some(renderer), Some(texture), Some(size)) =
                        (self.renderer.as_mut(), scene3d.output_texture(), scene3d.output_size())
                    {
                        // View própria no compositor: também é o backdrop do vidro (3D full-bleed).
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        renderer.set_scene3d_output(gpu, view, size);
                        self.scene3d_bound_gen = generation;
                    }
                }
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.scene3d_frame_rendered();
                }
                // Conteúdo da textura mudou: o passe 2D precisa ser regravado.
                self.dirty = true;
            }
        }

        if scene3d.needs_frame() {
            let at = now + cadence.max(Duration::from_millis(1));
            self.scene3d_next_at = Some(at);
            Some(at)
        } else {
            self.scene3d_next_at = None;
            scene3d.idle_wake().map(|d| now + d)
        }
    }

    /// `%APPDATA%\PrismView\native`: estado que o motor lê sozinho (Studio fechado).
    fn state_dir() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("PrismView").join("native"))
    }

    /// Lê um `PrismScene` JSON do disco (tamanho limitado).
    fn read_scene_file(path: &std::path::Path) -> Result<PrismScene, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("scene_file: {e}"))?;
        if meta.len() > MAX_SCENE_FILE_BYTES {
            return Err("scene_file_too_large".to_string());
        }
        let raw = std::fs::read_to_string(path).map_err(|e| format!("scene_file: {e}"))?;
        serde_json::from_str::<PrismScene>(&raw).map_err(|e| format!("scene_invalid: {e}"))
    }

    /// Ao iniciar: preferência de energia e última cena aplicada pelo Studio. O renderer só é
    /// criado em `rebuild_outputs`, que lê `self.scene` (`ensure_renderer`).
    fn restore_persisted_state(&mut self) {
        let Some(dir) = Self::state_dir() else {
            return;
        };
        if let Ok(raw) = std::fs::read_to_string(dir.join(POWER_PREFERENCE_FILE)) {
            if let Some(preference) = PowerPreference::parse(&raw) {
                self.preference = preference;
            }
        }
        let scene_path = dir.join(ACTIVE_SCENE_FILE);
        if scene_path.is_file() {
            match Self::read_scene_file(&scene_path) {
                Ok(scene) => self.scene = Some(scene),
                Err(err) => eprintln!("[prismview-engine] cena persistida: {err}"),
            }
        }
    }

    /// Troca a cena ativa (IPC `LoadScene`/`LoadSceneFile`). Recarga imediata: o `SceneRenderer`
    /// é reaproveitado (device, pipelines, fontes); só a cena e os recursos dela são trocados.
    fn load_scene(&mut self, scene: PrismScene) -> IpcResponse {
        if self.gpu.is_none() || self.outputs.is_empty() {
            return IpcResponse::Error {
                message: "gpu_unavailable".to_string(),
            };
        }
        self.ctx.now = Local::now();
        self.ctx.globals = scene.globals_map();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_scene(scene.clone(), &self.ctx);
        }
        self.scene = Some(scene);
        self.refresh_scene3d();
        self.refresh_videos();
        self.dirty = true;
        self.retry_at = None;
        self.governor.invalidate_frozen();
        IpcResponse::Ok
    }

    /// Fatos para o governador: visibilidade por monitor, pausa do Studio e bateria (bus).
    fn governor_inputs(&self, host: &Win32DesktopHost) -> GovernorInputs {
        let mut any_visible = false;
        let mut all_locked = !self.outputs.is_empty();
        for output in &self.outputs {
            match host.monitor_visibility(&output.monitor_id) {
                DesktopVisibility::Visible => {
                    any_visible = true;
                    all_locked = false;
                }
                DesktopVisibility::Covered => all_locked = false,
                DesktopVisibility::Locked => {}
            }
        }
        let power = self
            .sensors
            .as_ref()
            .map(|s| s.bus().power())
            .unwrap_or_default();
        GovernorInputs {
            preference: self.preference,
            user_paused: self.paused,
            any_visible,
            display_off: all_locked,
            on_battery: power.on_battery,
            battery_percent: power.battery_percent,
            battery_saver: power.battery_saver,
        }
    }

    /// Aplica a decisão do governador: pós-processamento, hibernação de VRAM, redesenho.
    fn apply_power(&mut self, update: GovernorUpdate) {
        if update.caps_changed {
            let caps = self.governor.caps();
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_post_processing(caps.post_processing);
            }
            if let Some(scene3d) = self.scene3d.as_mut() {
                scene3d.set_render_scale_cap(caps.render_scale_cap);
            }
            self.scene3d_next_at = None;
        }
        if update.hibernate == HibernateAction::Release {
            // Mantém cena, canvases, texto e modelos; só os alvos pesados caem.
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.release_glass_gpu();
                // O bind group seguraria a textura de saída do 3D viva: solta antes.
                renderer.clear_scene3d_output();
            }
            if let Some(scene3d) = self.scene3d.as_mut() {
                scene3d.release_targets();
            }
            self.scene3d_bound_gen = SCENE3D_UNBOUND;
        }
        if update.needs_redraw() {
            self.dirty = true;
        }
    }

    /// Declara ao bus o que a cena consome (produtores fora da demanda dormem) e o que acorda o
    /// loop. `set_demand` só notifica os sensores quando muda: barato a cada iteração.
    fn sync_signal_demand(&mut self) {
        let Some(bus) = self.sensors.as_ref().map(|s| Arc::clone(s.bus())) else {
            return;
        };
        // Pausado/congelado: nada da cena é consumido → produtores dormem. O governador só
        // precisa de `POWER` (amostragem lenta) para reagir a bateria/carregador.
        let scene_demand = if self.governor.animations_allowed() {
            let demand_2d = self
                .renderer
                .as_ref()
                .filter(|r| r.has_scene())
                .map_or(SignalKind::NONE, SceneRenderer::signal_demand);
            let demand_3d = self.scene3d.as_ref().map_or(SignalKind::NONE, |s| {
                let cursor = if s.wants_cursor() { SignalKind::MOUSE } else { SignalKind::NONE };
                s.signal_demand() | cursor
            });
            demand_2d | demand_3d
        } else {
            SignalKind::NONE
        };
        self.scene_demand = scene_demand;
        let demand = scene_demand | SignalKind::POWER;
        bus.set_demand(demand);
        bus.set_wake_mask(demand);
    }

    /// Lê o bus, avança os bindings 2D/vidro e trata toques no desktop. Sem alocação.
    fn process_signals(&mut self, now: Instant) {
        let Some(bus) = self.sensors.as_ref().map(|s| Arc::clone(s.bus())) else {
            return;
        };
        if self.scene_demand.is_empty() {
            self.last_signals_at = None;
            return;
        }
        let dt = self
            .last_signals_at
            .map_or(1.0 / 60.0, |at| now.saturating_duration_since(at).as_secs_f32());
        self.last_signals_at = Some(now);
        self.signal_reader.update(&bus, now, &mut self.snapshot);
        self.snapshot.cursor_norm = Self::cursor_norm(&self.outputs, self.snapshot.cursor);
        self.sync_fx_context(&bus);

        if let Some(renderer) = self.renderer.as_mut() {
            if renderer.apply_signals(&self.snapshot, dt) {
                self.dirty = true;
            }
        }

        // 3D: snapshot do quadro + cursor normalizado sobre a camada (órbita/CURSOR_LOOK/hover).
        let pointer = if self.scene_demand.contains(SignalKind::MOUSE) {
            Self::scene3d_local(&self.outputs, self.snapshot.cursor)
        } else {
            None
        };
        if let Some(scene3d) = self.scene3d.as_mut() {
            scene3d.set_signals(&self.snapshot);
            scene3d.set_pointer(pointer);
        }

        // Toque no fundo do desktop → região tocável da saída daquele monitor → SMTC; sem
        // controle 2D no ponto, vai para o 3D (CLICK_MESH).
        if let Some(tap) = self.snapshot.desktop_tap {
            let mut consumed = false;
            let mut custom = None;
            for output in &self.outputs {
                let Some(local) = tap_to_local(tap, output.origin, output.surface.size()) else {
                    continue;
                };
                if let Some(action) = SceneRenderer::hit_test(&output.canvas, local) {
                    consumed = true;
                    match (action, action.media_command()) {
                        (_, Some(command)) => {
                            if let Some(sensors) = self.sensors.as_mut() {
                                if let Err(err) = sensors.media_control(command) {
                                    eprintln!("[prismview-engine] mídia: {err}");
                                }
                            }
                        }
                        (DesktopAction::Custom(index), None) => custom = Some(index),
                        _ => {}
                    }
                }
                break;
            }
            if let Some(index) = custom {
                self.run_touch_action(index);
            }
            if !consumed {
                if let Some(local) = Self::scene3d_local(&self.outputs, tap) {
                    if let Some(scene3d) = self.scene3d.as_mut() {
                        if scene3d.click(local) {
                            self.scene3d_next_at = None;
                        }
                    }
                }
            }
        }
    }

    // ── Vídeo ──

    /// (Re)abre um decoder por vídeo da cena atual. Raro (troca de cena / device novo): aloca.
    fn refresh_videos(&mut self) {
        let wanted = self
            .renderer
            .as_ref()
            .map(SceneRenderer::video_sources)
            .unwrap_or_default();
        let same = wanted.len() == self.videos.len()
            && wanted.iter().all(|(p, _)| self.videos.iter().any(|v| v.path == *p));
        if same {
            return;
        }
        self.close_videos();
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        for (path, _background) in wanted {
            if !path.is_file() {
                eprintln!("[prismview-engine] vídeo ausente: {}", path.display());
                continue;
            }
            let mut decoder = MfVideoDecoder::new();
            if let Err(err) = decoder.bind_gpu(gpu.device()) {
                eprintln!("[prismview-engine] vídeo (gpu): {err}");
                continue;
            }
            decoder.set_looping(true);
            match decoder.open(&path) {
                Ok(_) => self.videos.push(VideoSlot {
                    path,
                    decoder,
                    paused: false,
                }),
                Err(err) => eprintln!("[prismview-engine] vídeo {}: {err}", path.display()),
            }
        }
        self.dirty = true;
    }

    fn close_videos(&mut self) {
        for slot in &mut self.videos {
            slot.decoder.close();
        }
        self.videos.clear();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_videos();
        }
    }

    /// Avança os decoders e registra frames novos no compositor. Pausado/congelado = decoders
    /// pausados (sem decodificar). Devolve quando acordar para o próximo frame.
    fn step_videos(&mut self, now: Instant) -> Option<Instant> {
        if self.videos.is_empty() {
            return None;
        }
        let paused = !self.governor.animations_allowed();
        let gpu = self.gpu.as_ref()?;
        let mut next: Option<Duration> = None;
        for slot in &mut self.videos {
            if slot.paused != paused {
                slot.decoder.set_paused(paused);
                slot.paused = paused;
            }
            if paused {
                continue;
            }
            if let Some(err) = slot.decoder.last_error() {
                eprintln!("[prismview-engine] vídeo {}: {err}", slot.path.display());
            }
            let advanced = slot.decoder.advance();
            if advanced {
                if let (Some(renderer), Some(frame)) =
                    (self.renderer.as_mut(), slot.decoder.current_gpu_frame())
                {
                    if renderer.set_video_frame(gpu, &slot.path, &frame) {
                        self.dirty = true;
                    }
                }
            }
            if let Some(d) = slot.decoder.time_until_next_frame() {
                next = Some(next.map_or(d, |n| n.min(d)));
            }
        }
        next.map(|d| now + d.max(Duration::from_millis(1)).max(self.governor.frame_interval()))
    }

    // ── Contexto FX (mídia, telemetria) ──

    /// Cursor (-1..=1) sobre o monitor que o contém; fora de todos = centro.
    fn cursor_norm(outputs: &[Output], cursor: (i32, i32)) -> [f32; 2] {
        for o in outputs {
            let (w, h) = o.surface.size();
            if let Some(local) = tap_to_local(cursor, o.origin, (w, h)) {
                let nx = local[0] / w.max(1) as f32 * 2.0 - 1.0;
                let ny = local[1] / h.max(1) as f32 * 2.0 - 1.0;
                return [nx.clamp(-1.0, 1.0), ny.clamp(-1.0, 1.0)];
            }
        }
        [0.0, 0.0]
    }

    /// Copia mídia/telemetria do bus para o `EvalContext` e invalida a lista só se a cena
    /// depende do que mudou. Sem alocação no caminho comum (texto só quando a versão muda).
    fn sync_fx_context(&mut self, bus: &SignalBus) {
        let snap = &self.snapshot;
        let mut changed = DirtyFlags::NONE;

        // Telemetria (porcentagens inteiras: evita refazer a lista por ruído decimal).
        let cpu = if snap.cpu_percent.is_finite() { snap.cpu_percent } else { 0.0 };
        let ram = if snap.ram_percent.is_finite() { snap.ram_percent } else { 0.0 };
        let bat = if snap.battery_percent.is_finite() {
            snap.battery_percent.round().clamp(0.0, 100.0) as u8
        } else {
            100
        };
        let sys = &mut self.ctx.system;
        if sys.cpu_percent.round() != cpu.round()
            || sys.ram_percent.round() != ram.round()
            || sys.battery_percent != bat
            || sys.battery_charging == snap.on_battery
        {
            sys.cpu_percent = cpu;
            sys.ram_percent = ram;
            sys.battery_percent = bat;
            sys.battery_charging = !snap.on_battery;
            sys.ram_total_gb = self.ram_total_gb;
            sys.ram_used_gb = self.ram_total_gb * (ram / 100.0);
            changed.insert(DirtyFlags::SYSTEM_METRICS);
        }

        // Mídia: escalares por quadro (posição extrapolada pelo leitor) + texto por versão.
        let media = &mut self.ctx.media;
        let mut refresh_deps = media.playing != snap.media_playing;
        let pos = f64::from(snap.media_position_s.max(0.0));
        let dur = f64::from(snap.media_duration_s.max(0.0));
        if media.playing != snap.media_playing
            || (media.duration_secs - dur).abs() > 0.5
            || (media.position_secs - pos).abs() >= 1.0
        {
            media.playing = snap.media_playing;
            media.duration_secs = dur;
            media.position_secs = pos;
            media.updated_at_ms = self.ctx.now.timestamp_millis() as f64;
            changed.insert(DirtyFlags::MEDIA);
        }
        let version = snap.media_version.wrapping_add(snap.cover_version.rotate_left(32));
        if version != self.media_version_seen && bus.read_media_text(&mut self.media_text) {
            self.media_version_seen = version;
            let media = &mut self.ctx.media;
            if media.title != self.media_text.title
                || media.artist != self.media_text.artist
                || media.album != self.media_text.album
                || media.cover_path != self.media_text.cover_path
            {
                media.title.clone_from(&self.media_text.title);
                media.artist.clone_from(&self.media_text.artist);
                media.album.clone_from(&self.media_text.album);
                media.cover_path.clone_from(&self.media_text.cover_path);
                changed.insert(DirtyFlags::MEDIA);
                refresh_deps = true;
            }
        }

        if changed.is_empty() {
            return;
        }
        if let Some(renderer) = self.renderer.as_mut() {
            if refresh_deps {
                // `$mi(pos)$` depende do segundo só enquanto toca; `if(mi(...))` pode mudar ramos.
                renderer.refresh_deps(&self.ctx);
            }
            if renderer.mark_dirty(changed) {
                self.dirty = true;
            }
        }
    }

    /// Executa uma ação de toque não-mídia da cena.
    fn run_touch_action(&mut self, index: u32) {
        let Some(action) = self
            .renderer
            .as_ref()
            .and_then(|r| r.touch_action(index))
            .cloned()
        else {
            return;
        };
        let key = action.global_name.trim().to_lowercase();
        let new_value = match action.action_type {
            TouchActionType::OpenUrl => {
                if let Err(err) = shell_open(&action.url, true) {
                    eprintln!("[prismview-engine] abrir URL: {err}");
                }
                None
            }
            TouchActionType::OpenApp => {
                if let Err(err) = shell_open(&action.target_exec, false) {
                    eprintln!("[prismview-engine] abrir app: {err}");
                }
                None
            }
            TouchActionType::ToggleGlobal if !key.is_empty() => {
                let cur = self.ctx.globals.get(&key).map(|v| v.trim().to_ascii_lowercase());
                let on = matches!(cur.as_deref(), Some(v) if !v.is_empty() && v != "0" && v != "false");
                Some(if on { "0" } else { "1" }.to_string())
            }
            TouchActionType::SetGlobal if !key.is_empty() => Some(action.global_value.clone()),
            TouchActionType::NextGlobalList if !key.is_empty() => {
                let options = self
                    .scene
                    .as_ref()
                    .and_then(|s| s.globals.iter().find(|g| g.name.to_lowercase() == key))
                    .map(|g| g.options.clone())
                    .unwrap_or_default();
                if options.is_empty() {
                    None
                } else {
                    let cur = self.ctx.globals.get(&key).cloned().unwrap_or_default();
                    let at = options.iter().position(|o| *o == cur).map_or(0, |i| (i + 1) % options.len());
                    Some(options[at].clone())
                }
            }
            _ => None,
        };
        let Some(value) = new_value else {
            return;
        };
        self.ctx.globals.insert(key.clone(), value.clone());
        if let Some(scene) = self.scene.as_mut() {
            scene.set_global(&key, value);
        }
        if let Some(renderer) = self.renderer.as_mut() {
            if renderer.mark_dirty(DirtyFlags::GLOBALS) {
                self.dirty = true;
            }
        }
    }

    /// Solta superfícies (antes das janelas) e, se `with_gpu`, o device.
    fn release_gpu_resources(&mut self, with_gpu: bool) {
        // Ordem: superfícies/canvases → renderer → device.
        self.outputs.clear();
        if with_gpu {
            // Decoders importam texturas no device: caem antes dele.
            self.close_videos();
            // 3D antes do device: renderer → recursos compartilhados → loader (config do device).
            self.scene3d = None;
            self.scene3d_shared = None;
            self.loader = None;
            self.scene3d_bound_gen = SCENE3D_UNBOUND;
            self.scene3d_last_update = None;
            self.scene3d_next_at = None;
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
        renderer.set_post_processing(self.governor.caps().post_processing);
        for dir in Self::font_dirs() {
            if dir.is_dir() {
                renderer.register_font_dir(&dir);
            }
        }
        if let Some(scene) = self.scene.clone() {
            self.ctx.now = Local::now();
            self.ctx.globals = scene.globals_map();
            renderer.set_scene(scene, &self.ctx);
        }
        self.renderer = Some(renderer);
        self.refresh_scene3d();
        self.refresh_videos();
    }

    /// (Re)cria janela + superfície wgpu para cada monitor atual. Raro (início, shell
    /// reiniciado, monitores mudaram, device perdido): pode alocar.
    fn rebuild_outputs(&mut self, host: &mut Win32DesktopHost) {
        self.release_gpu_resources(false);
        self.rebuild_at = None;

        let monitors: Vec<(String, u32, u32, f32, (i32, i32))> = host
            .monitors()
            .iter()
            .map(|m| {
                let i = &m.info;
                (i.id.clone(), i.width, i.height, i.scale_factor, (i.x, i.y))
            })
            .collect();

        let mut pending: Vec<(String, f32, (i32, i32), WindowSurface)> = Vec::with_capacity(monitors.len());
        for (monitor_id, width, height, scale, origin) in monitors {
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
                        pending.push((monitor_id, scale, origin, surface));
                    }
                    Err(err) => eprintln!("[prismview-engine] gpu: {err}"),
                }
                continue;
            }
            let Some(gpu) = self.gpu.as_ref() else {
                continue;
            };
            match gpu.create_surface(handles, width, height) {
                Ok(surface) => pending.push((monitor_id, scale, origin, surface)),
                Err(err) => eprintln!("[prismview-engine] superfície {monitor_id}: {err}"),
            }
        }

        if let Some(format) = pending.first().map(|(_, _, _, s)| s.format()) {
            self.ensure_renderer(format);
        }
        if let (Some(gpu), Some(renderer)) = (self.gpu.as_ref(), self.renderer.as_ref()) {
            for (monitor_id, scale, origin, surface) in pending {
                let canvas = renderer.create_canvas(gpu);
                self.outputs.push(Output {
                    monitor_id,
                    origin,
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
        self.governor.invalidate_frozen();
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
    /// `true` se ao menos um quadro foi apresentado.
    fn render(&mut self, host: &Win32DesktopHost) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };
        let mut presented = false;
        let color = self.clear_color;
        let mut retry = false;
        let mut rebuild = false;
        let mut device_lost = false;
        let mut renderer = self.renderer.as_mut().filter(|r| r.has_scene());
        // Cena vinda do Estúdio: escala = `containerScale` (espaço lógico → px da saída).
        let logical = self.scene.as_ref().and_then(|s| s.logical_size);

        for output in self.outputs.iter_mut() {
            if host.monitor_visibility(&output.monitor_id) != DesktopVisibility::Visible {
                continue; // coberto/bloqueado: volta no VisibilityChanged
            }
            let ui_scale = match logical {
                Some([lw, lh]) if lw > 0.0 && lh > 0.0 => {
                    let (w, h) = output.surface.size();
                    (w as f32 / lw).min(h as f32 / lh)
                }
                _ => output.scale,
            };
            let status = match renderer.as_deref_mut() {
                Some(r) => r.render(
                    gpu,
                    &mut output.surface,
                    &mut output.canvas,
                    &output.monitor_id,
                    ui_scale,
                    &self.ctx,
                    color,
                ),
                None => output.surface.clear(gpu, color),
            };
            match status {
                FrameStatus::Presented => {
                    self.frames += 1;
                    presented = true;
                }
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
        presented
    }

    fn handle_ipc(&mut self, request: IpcRequest) -> IpcResponse {
        match request {
            IpcRequest::Ping => IpcResponse::Pong {
                protocol_version: MANIFEST_VERSION,
            },
            IpcRequest::GetStatus => IpcResponse::Status {
                engine: EngineStatus {
                    running: true,
                    paused: self.governor.state() == PowerState::Paused,
                    pause_reason: self
                        .governor
                        .pause_reason()
                        .map(|reason| reason.as_str().to_string()),
                    active_wallpaper_id: self.scene.as_ref().map(|s| s.id.clone()),
                    pinned_widget_ids: Vec::new(),
                    active_monitor_count: u32::try_from(self.outputs.len()).unwrap_or(u32::MAX),
                    rendered_frames: self.frames,
                    gpu_adapter: self
                        .gpu
                        .as_ref()
                        .map_or_else(|| "Unavailable".to_string(), |g| g.adapter_name().to_string()),
                    power_state: self.governor.state().as_str().to_string(),
                },
            },
            IpcRequest::LoadScene { scene, .. } => self.load_scene(*scene),
            IpcRequest::LoadSceneFile { path } => match Self::read_scene_file(std::path::Path::new(&path)) {
                Ok(scene) => self.load_scene(scene),
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::ClearDesktop { color } => {
                if self.gpu.is_none() || self.outputs.is_empty() {
                    return IpcResponse::Error {
                        message: "gpu_unavailable".to_string(),
                    };
                }
                self.drop_scene3d();
                self.close_videos();
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.clear_scene();
                }
                self.scene = None;
                self.clear_color = color.map_or(DEFAULT_CLEAR, sanitize_color);
                self.dirty = true;
                self.governor.invalidate_frozen();
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
            IpcRequest::SetPowerPreference { preference } => match PowerPreference::parse(&preference) {
                Some(parsed) => {
                    self.preference = parsed;
                    if let Some(dir) = Self::state_dir() {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = std::fs::write(dir.join(POWER_PREFERENCE_FILE), preference.trim());
                    }
                    IpcResponse::Ok
                }
                None => IpcResponse::Error {
                    message: format!("invalid_power_preference: {preference}"),
                },
            },
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
            DesktopEvent::VisibilityChanged(_) => {
                self.dirty = true;
                self.governor.invalidate_frozen();
            }
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
        self.governor.invalidate_frozen();
    }

    fn about_to_wait(&mut self, host: &mut Win32DesktopHost) -> ControlFlow {
        if self.exit {
            return ControlFlow::Exit;
        }

        let now = Instant::now();
        if self.rebuild_at.is_some_and(|at| now >= at) {
            self.rebuild_outputs(host);
        }

        // Governador (Fase 5): decide estado/caps antes de qualquer trabalho.
        let inputs = self.governor_inputs(host);
        let update = self.governor.update(now, &inputs);
        self.apply_power(update);

        // Sinais (Fase 4): demanda da cena atual → bus; bindings + toques no desktop.
        self.sync_signal_demand();
        self.process_signals(now);

        // Relógio da cena: só invalida (e acorda) quando `$df(..)$` muda de fato.
        // Pausado/congelado: o relógio fica parado (retoma na volta, sem acumular trabalho).
        let animate = self.governor.animations_allowed();
        self.ctx.now = Local::now();
        if animate {
            if let Some(renderer) = self.renderer.as_mut() {
                if renderer.advance_clock(&self.ctx.now) {
                    self.dirty = true;
                }
            }
        }

        // 3D: carrega/anima/renderiza a textura antes do passe 2D que a compõe.
        let scene3d_at = self.step_scene3d(host, now);
        // Vídeo: troca o frame exibido (zero-copy) antes do passe 2D.
        let video_at = self.step_videos(now);

        // Teto de FPS / pausa / quadro congelado.
        let mut throttle_at = None;
        let due = self.retry_at.map_or(true, |at| now >= at);
        if self.dirty && due {
            match self.governor.frame_gate(now) {
                FrameGate::Now => {
                    self.dirty = false;
                    self.retry_at = None;
                    if self.render(host) {
                        self.governor.frame_presented(Instant::now());
                    }
                }
                FrameGate::At(at) => throttle_at = Some(at),
                // Pausado/congelado: `dirty` fica pendente e é atendido na retomada/invalidação.
                FrameGate::Blocked => {}
            }
        }

        // Próximo tique do relógio, só com animação permitida e algum monitor visível.
        let clock_at = if animate {
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
        } else {
            None
        };
        // Molas dos bindings ainda se movendo: mais um quadro na cadência do governador
        // (parado = dorme até o próximo sinal).
        let binding_at = (animate
            && self.renderer.as_ref().is_some_and(SceneRenderer::bindings_animating))
        .then(|| Instant::now() + BINDING_FRAME.max(self.governor.frame_interval()));
        let governor_at = self.governor.next_deadline(now);
        // Caixa 3D recém-construída por este render e 3D ainda sem quadro: volta já.
        let scene3d_at = scene3d_at.or_else(|| {
            let pending = animate
                && self.scene3d.as_ref().is_some_and(Scene3dRenderer::needs_frame)
                && self.scene3d_placement(host).is_some();
            pending.then(Instant::now)
        });
        let next = [
            self.retry_at,
            self.rebuild_at,
            clock_at,
            binding_at,
            throttle_at,
            governor_at,
            scene3d_at,
            video_at,
        ]
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
