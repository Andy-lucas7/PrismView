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
    DirtyFlags, EngineIssue, EngineStatus, IpcRequest, IpcResponse, PrismScene,
    ENGINE_ISSUES_MAX, MANIFEST_VERSION,
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
    acquire_single_instance, log_file, open_studio, parse_tray_enabled, resolve_font_files,
    run as run_loop, set_windows_desktop_wallpaper, trim_working_set, wallpaper_presenter,
    ControlFlow, HostConfig, LoopHandler, LoopProxy, MfVideoDecoder, SensorsConfig, TrayAction,
    Win32DesktopHost, Win32SystemSensors,
};
use crate::platform::{
    DesktopEvent, DesktopHost, DesktopVisibility, HardwareVideoDecoder, NativeWindowId,
    SystemSensors,
};
use crate::ipc_server::{self, IpcEnvelope};
use crate::widgets::{
    apply_touch_action, combined_demand, load_active_widgets, local_point, monitor_for, parse_formula_number,
    save_active_widgets, widget_scene_path, Bounds, Scene3dDeps, WidgetRuntime, WIDGET_SCENES_DIR,
};
use prism_core::schema::NativeWidgetSpec;

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
/// Cena persistida pelo Studio (`LoadSceneFile`); relida ao iniciar sem o Studio.
const ACTIVE_SCENE_FILE: &str = "active_scene.json";
/// Preferência de energia persistida pelo motor (`SetPowerPreference`).
const POWER_PREFERENCE_FILE: &str = "power_preference.txt";
/// Preferência de exibição da engine na bandeja do sistema (`SetTrayVisible`).
const TRAY_ENABLED_FILE: &str = "tray_enabled.txt";
/// Sincronizar o papel de parede real do Windows (imagem limpa sem HUD) ao aplicar cena.
const SYNC_WINDOWS_WALLPAPER_FILE: &str = "sync_windows_wallpaper.txt";
/// Tempo máximo aguardando mídia (vídeo/imagens/3D) carregar antes de capturar o snapshot limpo.
const WIN_WALLPAPER_SYNC_TIMEOUT: Duration = Duration::from_secs(4);
/// Limite defensivo do JSON da cena.
const MAX_SCENE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// `output_generation` ainda não registrada no compositor.
const SCENE3D_UNBOUND: u64 = u64::MAX;
/// Intervalo mínimo entre quadros de widgets com molas em movimento (sem bloqueio de VSync).
const WIDGET_MIN_FRAME: Duration = Duration::from_micros(8_333);

/// Reaberturas de um vídeo com erro (1 s, 5 s, 30 s; depois desiste até a próxima cena).
const VIDEO_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

/// Decoder de um arquivo de vídeo da cena.
struct VideoSlot {
    path: PathBuf,
    decoder: MfVideoDecoder,
    paused: bool,
    /// Erro atual já registrado (log + status): não repetir a cada iteração.
    error_reported: bool,
    retries: usize,
    retry_at: Option<Instant>,
}

/// Registra um problema da cena (log + `EngineStatus.errors`, sem duplicar, até o teto).
fn push_issue(issues: &mut Vec<EngineIssue>, layer_id: Option<&str>, message: String) {
    eprintln!("[prismview-engine] {message}");
    if issues.len() < ENGINE_ISSUES_MAX && !issues.iter().any(|i| i.message == message) {
        issues.push(EngineIssue {
            layer_id: layer_id.map(str::to_string),
            message,
        });
    }
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
    /// `true` após carga de cena: reduz working set uma vez depois de apresentar o 1º quadro.
    needs_mem_trim: bool,
    /// Exibe o ícone da engine na bandeja do sistema enquanto executa (padrão: `true`).
    tray_visible: bool,
    /// Marca que a visibilidade ou o tooltip da bandeja precisa ser sincronizado no `host`.
    tray_sync_needed: bool,
    /// Atualiza o papel de parede real do Windows (`SPI_SETDESKWALLPAPER`) ao aplicar wallpaper.
    sync_windows_wallpaper: bool,
    /// Captura limpa pendente (deadline até onde aguardar vídeo/imagens/3D carregarem).
    pending_win_wallpaper: Option<Instant>,
    /// Alterna A/B no nome do arquivo para garantir que o Windows 11 sempre recarregue a imagem.
    win_wallpaper_slot: bool,
    /// Sinalizado pela thread de `SPI_SETDESKWALLPAPER` ao terminar: reanexa nossa janela acima do WorkerW.
    win_wallpaper_applied: Arc<std::sync::atomic::AtomicBool>,
    /// Segunda passada de reanexação após o crossfade assíncrono do Windows 11 (~650 ms).
    win_wallpaper_reattach_at: Option<Instant>,
    /// Entrega 3: widgets nativos (mesmo device/3D/sensores do wallpaper).
    widgets: Vec<WidgetRuntime>,
    /// O wallpaper (janelas WorkerW) deve existir: há cena aplicada ou `ClearDesktop` explícito.
    /// Só widgets no ar ⇒ nenhuma janela de wallpaper (o fundo do Windows continua visível).
    wallpaper_wanted: bool,
    /// Segundo do relógio em que as fórmulas dos bindings 3D foram avaliadas pela última vez.
    formulas_sec: i64,
    /// Dependências FX das fórmulas dos bindings 3D (decidem a demanda de sensores e o tique de 1 s).
    formula_deps: DirtyFlags,
    /// Problemas da cena atual (vídeo, arquivo de cena); limpo a cada `load_scene`.
    issues: Vec<EngineIssue>,
}

/// Ponto de entrada do modo nativo. Retorna o código de saída do processo.
pub fn run() -> i32 {
    // Login do Windows (`--native --login`, registrado pelo Studio): sem cena aplicada, sai na hora
    // em vez de cobrir o wallpaper do sistema com a cor de clear.
    let at_login = std::env::args().skip(1).any(|a| a == "--login");
    let has_scene = NativeApp::state_dir().is_some_and(|dir| dir.join(ACTIVE_SCENE_FILE).is_file());
    let has_widgets = NativeApp::state_dir().is_some_and(|dir| !load_active_widgets(&dir).is_empty());
    if at_login && !has_scene && !has_widgets {
        return 0;
    }

    // Sem console (processo destacado): `eprintln!`/pânicos → `native\engine.log` (2 × 1 MB).
    if let Some(dir) = NativeApp::state_dir() {
        if let Err(err) = log_file::redirect_stderr_to_file(&dir, "engine.log") {
            eprintln!("[prismview-engine] log: {err}");
        }
    }
    eprintln!("[prismview-engine] início pid={} {}", std::process::id(), Local::now().to_rfc3339());

    // Um motor por sessão: login + Studio subindo juntos, ou PING perdido, não podem gerar dois
    // processos na GPU/WorkerW. Mantido vivo até `run` retornar.
    let _single_instance = match acquire_single_instance() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            eprintln!("[prismview-engine] outro motor já está em execução; saindo");
            return 0;
        }
        Err(err) => {
            eprintln!("[prismview-engine] instância única: {err}");
            return 1;
        }
    };

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
    crate::engine_core::scene2d::set_image_waker(Arc::new(move || {
        proxy.wake();
    }));
    let (tx, rx) = mpsc::channel();
    if let Err(err) = ipc_server::spawn_named_pipe_server(
        move || {
            proxy.wake();
        },
        tx,
    ) {
        // Pipe já criado por outro processo (squatting, ou motor antigo sem o mutex ainda no ar):
        // sair em vez de rodar um segundo motor que o Studio não controla.
        eprintln!("[prismview-engine] IPC: {err}");
        return 1;
    }

    let mut app = NativeApp::new(rx);
    app.proxy = Some(host.proxy());
    let bus = Arc::new(SignalBus::new());
    match Win32SystemSensors::new(bus, SensorsConfig::default(), Some(host.proxy())) {
        Ok(sensors) => app.sensors = Some(sensors),
        Err(err) => eprintln!("[prismview-engine] sensores: {err}"),
    }
    app.restore_persisted_state();
    host.set_tray_visible(app.tray_visible);
    host.set_tray_paused(app.paused);
    app.rebuild_outputs(&mut host);
    app.restore_widgets(&host);
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
            needs_mem_trim: false,
            tray_visible: true,
            tray_sync_needed: false,
            sync_windows_wallpaper: false,
            pending_win_wallpaper: None,
            win_wallpaper_slot: false,
            win_wallpaper_applied: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            win_wallpaper_reattach_at: None,
            widgets: Vec::new(),
            wallpaper_wanted: false,
            formulas_sec: i64::MIN,
            formula_deps: DirtyFlags::NONE,
            issues: Vec::new(),
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
        let Some(shared) = self.ensure_scene3d_infra() else {
            return;
        };
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let proxy = self.proxy;
        if self.scene3d.is_none() {
            let mut scene3d = Scene3dRenderer::new(shared, gpu, Scene3dOptions::default());
            scene3d.set_render_scale_cap(self.governor.caps().render_scale_cap);
            scene3d.set_msaa_cap(self.governor.caps().msaa);
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
            scene3d.set_media_cover(gpu, &self.ctx.media.cover_path);
        }
        self.scene3d_last_update = None;
        self.scene3d_next_at = None;
        self.dirty = true;
    }

    /// `Scene3dShared` (pipelines/PMREM por device) + `AssetLoader` únicos, criados sob demanda e
    /// compartilhados por wallpaper e widgets.
    fn ensure_scene3d_infra(&mut self) -> Option<Arc<Scene3dShared>> {
        let gpu = self.gpu.as_ref()?;
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
        Some(shared)
    }

    fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let proxy = self.proxy?;
        Some(Arc::new(move || {
            let _ = proxy.wake();
        }))
    }

    /// Eventos do loader (1 por iteração) para o 3D do wallpaper e de todos os widgets.
    fn poll_loader(&mut self) {
        let (Some(gpu), Some(loader)) = (self.gpu.as_ref(), self.loader.as_mut()) else {
            return;
        };
        loader.poll(&mut self.load_events);
        for event in &self.load_events {
            if let LoadEvent::Failed { path, error, .. } = event {
                eprintln!("[prismview-engine] 3d {}: {error}", path.display());
            }
            if let Some(scene3d) = self.scene3d.as_mut() {
                scene3d.handle_load_event(gpu, event);
            }
            for w in &mut self.widgets {
                w.handle_load_event(gpu, event);
            }
        }
        self.load_events.clear();
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

    /// Problemas atuais para o Studio: os registrados + modelos 3D com erro + imagens ilegíveis.
    fn status_issues(&self) -> Vec<EngineIssue> {
        let mut out = self.issues.clone();
        if let Some(scene3d) = self.scene3d.as_ref() {
            for (id, message) in scene3d.object_errors() {
                if out.len() >= ENGINE_ISSUES_MAX {
                    break;
                }
                out.push(EngineIssue {
                    layer_id: Some(id.to_string()),
                    message: format!("Objeto 3D: {message}"),
                });
            }
        }
        if let Some(renderer) = self.renderer.as_ref() {
            for path in renderer.failed_images() {
                if out.len() >= ENGINE_ISSUES_MAX {
                    break;
                }
                out.push(EngineIssue {
                    layer_id: None,
                    message: format!("Imagem não carregou: {path}"),
                });
            }
        }
        out
    }

    /// Arquivos de cena vindos do IPC: só dentro de `%APPDATA%\PrismView\native\` (o Studio grava
    /// ali); qualquer outro caminho é recusado.
    fn read_allowed_scene_file(raw: &str) -> Result<PrismScene, String> {
        let dir = Self::state_dir().ok_or_else(|| "scene_path_denied: APPDATA ausente".to_string())?;
        let dir = std::fs::canonicalize(&dir).map_err(|e| format!("scene_path_denied: {e}"))?;
        let path = std::fs::canonicalize(raw).map_err(|e| format!("scene_file: {e}"))?;
        if !path.starts_with(&dir) {
            return Err(format!("scene_path_denied: {raw}"));
        }
        Self::read_scene_file(&path)
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
        if let Ok(raw) = std::fs::read_to_string(dir.join(TRAY_ENABLED_FILE)) {
            self.tray_visible = parse_tray_enabled(&raw);
        }
        if let Ok(raw) = std::fs::read_to_string(dir.join(SYNC_WINDOWS_WALLPAPER_FILE)) {
            let t = raw.trim();
            self.sync_windows_wallpaper =
                t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("on");
        }
        let scene_path = dir.join(ACTIVE_SCENE_FILE);
        if scene_path.is_file() {
            match Self::read_scene_file(&scene_path) {
                Ok(scene) => {
                    self.scene = Some(scene);
                    self.wallpaper_wanted = true;
                }
                Err(err) => push_issue(&mut self.issues, None, format!("cena persistida: {err}")),
            }
        }
    }

    /// Agenda a atualização do papel de parede real do Windows assim que vídeo/imagens/3D
    /// estiverem prontos (com timeout defensivo).
    fn schedule_windows_wallpaper_sync(&mut self) {
        if self.sync_windows_wallpaper && self.scene.is_some() {
            self.pending_win_wallpaper = Some(Instant::now() + WIN_WALLPAPER_SYNC_TIMEOUT);
            self.dirty = true;
        }
    }

    /// Captura a versão limpa da cena atual (sem texto/relógio/telemetria/mídia) e grava em
    /// segundo plano como papel de parede real do Windows quando todos os assets visuais já
    /// subiram para a GPU (ou no timeout).
    fn maybe_sync_windows_wallpaper(&mut self, now: Instant) -> Option<Instant> {
        let deadline = self.pending_win_wallpaper?;
        if self.scene.is_none() || !self.sync_windows_wallpaper {
            self.pending_win_wallpaper = None;
            return None;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return Some(now + Duration::from_millis(200));
        };
        let Some(output) = self.outputs.first() else {
            return Some(now + Duration::from_millis(200));
        };
        // Aguarda pelo menos 1 build da cena para enfileirar imagens e medir a caixa 3D.
        if output.canvas.built_size == (0, 0) {
            return Some(now + Duration::from_millis(50));
        }
        let renderer_ready = self
            .renderer
            .as_ref()
            .is_some_and(|r| !r.has_pending_visual_assets());
        let scene3d_ready = self.scene3d.as_ref().map_or(true, |s| {
            !s.is_loading() && self.scene3d_bound_gen != SCENE3D_UNBOUND
        });
        if (!renderer_ready || !scene3d_ready) && now < deadline {
            return Some((now + Duration::from_millis(100)).min(deadline));
        }

        self.pending_win_wallpaper = None;
        let (w, h) = output.surface.size();
        let ui_scale = self
            .scene
            .as_ref()
            .and_then(|s| s.ui_scale(w as f32, h as f32))
            .unwrap_or(output.scale);
        let clear = self.clear_color;
        let Some((sw, sh, rgb)) = self
            .renderer
            .as_mut()
            .and_then(|r| r.render_clean_snapshot(gpu, (w, h), ui_scale, &self.ctx, clear))
        else {
            return None;
        };
        let Some(dir) = Self::state_dir() else {
            return None;
        };
        self.win_wallpaper_slot = !self.win_wallpaper_slot;
        let (cur_name, old_name) = if self.win_wallpaper_slot {
            ("windows_wallpaper_a.png", "windows_wallpaper_b.png")
        } else {
            ("windows_wallpaper_b.png", "windows_wallpaper_a.png")
        };
        let target = dir.join(cur_name);
        let old = dir.join(old_name);
        let tmp = dir.join("windows_wallpaper.tmp.png");
        let applied_flag = Arc::clone(&self.win_wallpaper_applied);
        let proxy = self.proxy;
        let _ = std::thread::Builder::new()
            .name("prism-win-wallpaper".into())
            .spawn(move || {
                let _ = std::fs::create_dir_all(&dir);
                if let Err(err) = image::save_buffer_with_format(
                    &tmp,
                    &rgb,
                    sw,
                    sh,
                    image::ColorType::Rgb8,
                    image::ImageFormat::Png,
                ) {
                    eprintln!("[prismview-engine] windows_wallpaper png: {err}");
                    return;
                }
                let _ = std::fs::remove_file(&target);
                if let Err(err) = std::fs::rename(&tmp, &target) {
                    eprintln!("[prismview-engine] windows_wallpaper rename: {err}");
                    let _ = std::fs::remove_file(&tmp);
                    return;
                }
                match set_windows_desktop_wallpaper(&target) {
                    Ok(()) => {
                        applied_flag.store(true, std::sync::atomic::Ordering::Release);
                        if let Some(p) = proxy {
                            p.wake();
                        }
                    }
                    Err(err) => eprintln!("[prismview-engine] {err}"),
                }
                let _ = std::fs::remove_file(&old);
            });
        None
    }

    /// Troca a cena ativa (IPC `LoadScene`/`LoadSceneFile`). Recarga imediata: o `SceneRenderer`
    /// é reaproveitado (device, pipelines, fontes); só a cena e os recursos dela são trocados.
    fn load_scene(&mut self, host: &mut Win32DesktopHost, scene: PrismScene) -> IpcResponse {
        if !self.wallpaper_wanted || self.outputs.is_empty() {
            // Motor estava só com widgets: cria as janelas de wallpaper agora.
            self.wallpaper_wanted = true;
            self.rebuild_outputs(host);
        }
        self.issues.clear();
        // A cena é SEMPRE aceita: sem GPU/saídas agora (Explorer reiniciando, monitor religando,
        // device perdido), `rebuild_outputs` → `ensure_renderer` aplica `self.scene` depois.
        self.ctx.now = Local::now();
        self.ctx.globals = scene.globals_map();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_scene(scene.clone(), &self.ctx);
        }
        self.scene = Some(scene);
        self.refresh_scene3d();
        self.refresh_videos();
        self.dirty = true;
        self.needs_mem_trim = true;
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
        for w in &self.widgets {
            if Self::widget_visible(host, w) {
                any_visible = true;
                all_locked = false;
            } else if !self.session_locked(host) {
                all_locked = false;
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

    /// Sessão bloqueada / usuário ausente (qualquer monitor reporta `Locked`).
    fn session_locked(&self, host: &Win32DesktopHost) -> bool {
        host.monitors()
            .first()
            .is_some_and(|m| host.monitor_visibility(&m.info.id) == DesktopVisibility::Locked)
    }

    /// Widget "sempre no topo" só pausa com a sessão bloqueada; widget no desktop pausa quando o
    /// monitor dele está coberto (janela maximizada / tela cheia), como o wallpaper.
    fn widget_visible(host: &Win32DesktopHost, w: &WidgetRuntime) -> bool {
        let monitor = w.monitor.as_ref().map(|m| m.0.as_str());
        let vis = match monitor {
            Some(id) => host.monitor_visibility(id),
            None => host
                .monitors()
                .first()
                .map_or(DesktopVisibility::Visible, |m| host.monitor_visibility(&m.info.id)),
        };
        match vis {
            DesktopVisibility::Visible => true,
            DesktopVisibility::Covered => w.spec.always_on_top,
            DesktopVisibility::Locked => false,
        }
    }

    fn monitor_bounds(host: &Win32DesktopHost) -> Vec<(String, Bounds)> {
        host.monitors()
            .iter()
            .map(|m| (m.info.id.clone(), (m.info.x, m.info.y, m.info.width, m.info.height)))
            .collect()
    }

    fn refresh_widget_monitors(&mut self, host: &Win32DesktopHost) {
        let monitors = Self::monitor_bounds(host);
        for w in &mut self.widgets {
            w.monitor = monitor_for(w.bounds(), monitors.iter().map(|(id, b)| (id.as_str(), *b)))
                .map(|(id, b)| (id.to_string(), b));
        }
    }

    // ── Widgets nativos ──

    fn ensure_gpu(&mut self) -> bool {
        if self.gpu.is_some() {
            return true;
        }
        match GpuContext::new_headless() {
            Ok(gpu) => {
                self.gpu = Some(gpu);
                true
            }
            Err(err) => {
                eprintln!("[prismview-engine] gpu (widgets): {err}");
                false
            }
        }
    }

    fn persist_widgets(&self) {
        let Some(dir) = Self::state_dir() else {
            return;
        };
        let specs: Vec<NativeWidgetSpec> = self.widgets.iter().map(|w| w.spec.clone()).collect();
        if let Err(err) = save_active_widgets(&dir, &specs) {
            eprintln!("[prismview-engine] active_widgets.json: {err}");
        }
    }

    /// Recria os widgets de `active_widgets.json` (início, login, device perdido).
    fn restore_widgets(&mut self, host: &Win32DesktopHost) {
        let Some(dir) = Self::state_dir() else {
            return;
        };
        for spec in load_active_widgets(&dir) {
            if self.widgets.iter().any(|w| w.spec.id == spec.id) {
                continue;
            }
            let path = if spec.scene_path.trim().is_empty() {
                widget_scene_path(&dir, &spec.id)
            } else {
                PathBuf::from(&spec.scene_path)
            };
            match Self::read_scene_file(&path) {
                Ok(scene) => {
                    if let Err(err) = self.upsert_widget(host, spec, scene) {
                        eprintln!("[prismview-engine] widget persistido: {err}");
                    }
                }
                Err(err) => eprintln!("[prismview-engine] widget {}: {err}", spec.id),
            }
        }
    }

    /// Cria ou atualiza um widget (idempotente por `id`). Não persiste (o chamador decide).
    fn upsert_widget(&mut self, host: &Win32DesktopHost, spec: NativeWidgetSpec, scene: PrismScene) -> Result<(), String> {
        let spec = spec.sanitized().ok_or("invalid_widget_id")?;
        if !self.ensure_gpu() {
            return Err("gpu_unavailable".into());
        }
        let post = self.governor.caps().post_processing;
        let font_dirs = Self::font_dirs();
        self.ctx.now = Local::now();
        let index = match self.widgets.iter().position(|w| w.spec.id == spec.id) {
            Some(i) => {
                let w = &mut self.widgets[i];
                w.set_scene(scene, &mut self.ctx);
                if w.bounds() != (spec.x, spec.y, spec.width, spec.height) {
                    w.set_bounds(spec.x, spec.y, spec.width, spec.height);
                }
                if (w.spec.always_on_top, w.spec.locked, w.spec.click_through)
                    != (spec.always_on_top, spec.locked, spec.click_through)
                {
                    w.set_flags(spec.always_on_top, spec.locked, spec.click_through);
                }
                w.spec.scene_path = spec.scene_path;
                i
            }
            None => {
                let Some(gpu) = self.gpu.as_ref() else {
                    return Err("gpu_unavailable".into());
                };
                let runtime = WidgetRuntime::new(host, gpu, spec, scene, &font_dirs, post, &mut self.ctx)?;
                self.widgets.push(runtime);
                self.widgets.len() - 1
            }
        };
        self.refresh_widget_scene3d(index);
        self.refresh_widget_monitors(host);
        self.formulas_sec = i64::MIN;
        self.needs_mem_trim = true;
        self.governor.invalidate_frozen();
        Ok(())
    }

    fn refresh_widget_scene3d(&mut self, index: usize) {
        let has3d = self
            .widgets
            .get(index)
            .is_some_and(|w| w.scene().layers.iter().any(Self::layer_has_scene3d));
        let shared = if has3d { self.ensure_scene3d_infra() } else { None };
        let waker = self.waker();
        let cap = self.governor.caps().render_scale_cap;
        let (Some(gpu), Some(w)) = (self.gpu.as_ref(), self.widgets.get_mut(index)) else {
            return;
        };
        let deps = match (shared.as_ref(), self.loader.as_mut()) {
            (Some(shared), Some(loader)) => Some(Scene3dDeps {
                shared,
                loader,
                waker,
                render_scale_cap: cap,
                msaa_cap: self.governor.caps().msaa,
            }),
            _ => None,
        };
        w.refresh_scene3d(gpu, deps, &self.ctx.media.cover_path);
    }

    fn layer_has_scene3d(layer: &prism_core::schema::LayerNode) -> bool {
        (layer.kind == prism_core::schema::LayerKind::Scene3d && layer.scene3d.is_some())
            || layer.children.iter().any(Self::layer_has_scene3d)
    }

    fn remove_widget(&mut self, id: &str) -> bool {
        let Some(index) = self.widgets.iter().position(|w| w.spec.id == id) else {
            return false;
        };
        let runtime = self.widgets.remove(index);
        // Cena escrita pelo Studio em `native/widgets/`: apaga junto (fora dali não é nossa).
        if let Some(dir) = Self::state_dir() {
            let own = dir.join(WIDGET_SCENES_DIR);
            let path = PathBuf::from(&runtime.spec.scene_path);
            if path.starts_with(&own) {
                let _ = std::fs::remove_file(path);
            }
        }
        drop(runtime); // superfície (espera a GPU) → janela
        self.needs_mem_trim = true;
        trim_working_set();
        true
    }

    /// Nada para desenhar: sem wallpaper e sem widgets ⇒ o motor encerra (Windows volta ao normal).
    fn exit_if_idle(&mut self) {
        if !self.wallpaper_wanted && self.widgets.is_empty() {
            self.exit = true;
        }
    }

    /// Remove só o wallpaper (janelas + cena + `active_scene.json`); widgets seguem no ar.
    fn clear_wallpaper(&mut self, host: &mut Win32DesktopHost) {
        self.drop_scene3d();
        self.close_videos();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_scene();
        }
        self.scene = None;
        self.pending_win_wallpaper = None;
        self.wallpaper_wanted = false;
        self.outputs.clear();
        host.detach_all_wallpapers();
        if let Some(dir) = Self::state_dir() {
            let _ = std::fs::remove_file(dir.join(ACTIVE_SCENE_FILE));
        }
        trim_working_set();
        self.exit_if_idle();
    }

    /// Avalia as fórmulas FX dos bindings 3D (wallpaper + widgets). Barato e raro: ≤ 1×/s ou
    /// quando mídia/telemetria/globais mudam (nunca por quadro).
    fn update_formulas(&mut self) {
        let mut deps = DirtyFlags::NONE;
        if let Some(scene3d) = self.scene3d.as_mut() {
            if scene3d.has_formulas() {
                let ctx = &self.ctx;
                let changed = scene3d.update_formulas(|expr| {
                    let out = prism_core::fx::FxEngine::eval_expr(expr, ctx);
                    deps.insert(out.deps);
                    parse_formula_number(&out.value)
                });
                if changed {
                    self.scene3d_next_at = None;
                }
            }
        }
        for w in &mut self.widgets {
            if w.has_formulas() {
                deps.insert(w.update_formulas(&mut self.ctx));
            }
        }
        self.formula_deps = deps;
    }

    /// Sensores que as fórmulas dos bindings 3D leem (mídia/telemetria).
    fn formula_demand(&self) -> SignalKind {
        let mut d = SignalKind::NONE;
        if self.formula_deps.intersects(DirtyFlags::MEDIA) {
            d |= SignalKind::MEDIA;
        }
        if self.formula_deps.intersects(DirtyFlags::SYSTEM_METRICS) {
            d |= SignalKind::TELEMETRY | SignalKind::POWER;
        }
        d
    }

    fn has_formulas(&self) -> bool {
        self.scene3d.as_ref().is_some_and(Scene3dRenderer::has_formulas) || self.widgets.iter().any(WidgetRuntime::has_formulas)
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
                scene3d.set_msaa_cap(caps.msaa);
            }
            for w in &mut self.widgets {
                w.set_post_processing(caps.post_processing);
                w.set_render_scale_cap(caps.render_scale_cap, caps.msaa);
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
            for w in &mut self.widgets {
                w.hibernate();
            }
            self.scene3d_bound_gen = SCENE3D_UNBOUND;
            trim_working_set();
        }
        if update.needs_redraw() {
            self.dirty = true;
            for w in &mut self.widgets {
                w.dirty = true;
            }
        }
    }

    /// Declara ao bus o que a cena consome (produtores fora da demanda dormem) e o que acorda o
    /// loop. `set_demand` só notifica os sensores quando muda: barato a cada iteração.
    fn sync_signal_demand(&mut self, host: &Win32DesktopHost) {
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
            // Wallpaper OR widgets visíveis: um único bus/sensores para todos.
            let all = combined_demand(
                demand_2d | demand_3d | self.formula_demand(),
                self.widgets.iter().map(|w| (Self::widget_visible(host, w), w.signal_demand())),
            );
            // `POWER` entra abaixo de qualquer jeito; fora daqui só conta se a cena o lê.
            let scene_power = (demand_2d | demand_3d).contains(SignalKind::POWER)
                || self.widgets.iter().any(|w| w.signal_demand().contains(SignalKind::POWER));
            if scene_power {
                all
            } else {
                SignalKind(all.0 & !SignalKind::POWER.0)
            }
        } else {
            SignalKind::NONE
        };
        self.scene_demand = scene_demand;
        let demand = scene_demand | SignalKind::POWER;
        bus.set_demand(demand);
    }

    /// Lê o bus, avança os bindings 2D/vidro e trata toques no desktop. Sem alocação.
    fn process_signals(&mut self, host: &Win32DesktopHost, now: Instant) {
        let Some(bus) = self.sensors.as_ref().map(|s| Arc::clone(s.bus())) else {
            return;
        };
        if self.scene_demand.is_empty() {
            self.last_signals_at = None;
            return;
        }
        // Limita `dt` após acordar do repouso (`> 100ms` vira passo limpo de 1/60s) para que o
        // primeiro movimento do mouse saia suavemente sem teleporte das molas.
        let dt = self.last_signals_at.map_or(1.0 / 60.0, |at| {
            let secs = now.saturating_duration_since(at).as_secs_f32();
            if secs > 0.1 {
                1.0 / 60.0
            } else {
                secs.clamp(0.0005, 0.05)
            }
        });
        self.last_signals_at = Some(now);
        self.signal_reader.update(&bus, now, &mut self.snapshot);
        if self.scene_demand.contains(SignalKind::MOUSE) {
            // Amostra a posição exata do cursor no instante do quadro (1×/frame apenas acordado),
            // eliminando atraso de polling e perda do ponto final de movimentos rápidos.
            if let Ok(pos) = host.cursor_position() {
                self.snapshot.cursor = pos;
            }
        }
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

        // Widgets: mesmos sinais do quadro; cursor global (olhar em qualquer lugar da tela).
        let cursor = self.scene_demand.contains(SignalKind::MOUSE).then_some(self.snapshot.cursor);
        for w in &mut self.widgets {
            if Self::widget_visible(host, w) {
                w.apply_signals(&self.snapshot, dt, cursor);
            }
        }

        // Toque observado no desktop: widget click-through sob o ponto vence o wallpaper.
        if let Some(tap) = self.snapshot.desktop_tap {
            if let Some(index) = self
                .widgets
                .iter()
                .rposition(|w| w.spec.click_through && local_point(tap, w.bounds()).is_some())
            {
                let local = local_point(tap, self.widgets[index].bounds()).unwrap_or([0.0, 0.0]);
                self.widget_tap(index, local);
                return;
            }
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

    /// Toque num widget (px locais): mídia → SMTC; ação custom → globais/URL/app do widget.
    fn widget_tap(&mut self, index: usize, local: [f32; 2]) {
        let Some(w) = self.widgets.get_mut(index) else {
            return;
        };
        let Some(action) = w.tap(local) else {
            return;
        };
        match (action, action.media_command()) {
            (_, Some(command)) => {
                if let Some(sensors) = self.sensors.as_mut() {
                    if let Err(err) = sensors.media_control(command) {
                        eprintln!("[prismview-engine] mídia (widget): {err}");
                    }
                }
            }
            (DesktopAction::Custom(i), None) => w.run_touch_action(i),
            _ => {}
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
                push_issue(&mut self.issues, None, format!("Vídeo não encontrado: {}", path.display()));
                continue;
            }
            let mut decoder = MfVideoDecoder::new();
            if let Err(err) = decoder.bind_gpu(gpu.device()) {
                eprintln!("[prismview-engine] vídeo (gpu): {err}");
                continue;
            }
            decoder.set_looping(true);
            if let Some(proxy) = self.proxy {
                decoder.set_waker(Arc::new(move || {
                    proxy.wake();
                }));
            }
            // Não bloqueia o loop (vídeo 4K/HEVC pode levar segundos no MF): o anel é importado
            // no `advance()` quando o worker responder.
            match decoder.open_async(&path) {
                Ok(()) => self.videos.push(VideoSlot {
                    path,
                    decoder,
                    paused: false,
                    error_reported: false,
                    retries: 0,
                    retry_at: None,
                }),
                Err(err) => push_issue(
                    &mut self.issues,
                    None,
                    format!("Vídeo não abriu: {} ({err})", path.display()),
                ),
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
            // Reabertura agendada após erro (backoff).
            if let Some(at) = slot.retry_at {
                if now < at {
                    let wait = at - now;
                    next = Some(next.map_or(wait, |n| n.min(wait)));
                    continue;
                }
                slot.retry_at = None;
                slot.error_reported = false;
                if let Err(err) = slot.decoder.open_async(&slot.path) {
                    push_issue(
                        &mut self.issues,
                        None,
                        format!("Vídeo não reabriu: {} ({err})", slot.path.display()),
                    );
                    slot.error_reported = true;
                    continue;
                }
            }
            // Erro: 1 registro por ocorrência (sem log/alocação por quadro) e reabertura agendada.
            if slot.decoder.has_error() {
                if !slot.error_reported {
                    slot.error_reported = true;
                    let err = slot.decoder.last_error().unwrap_or_default();
                    push_issue(
                        &mut self.issues,
                        None,
                        format!("Erro no vídeo {}: {err}", slot.path.display()),
                    );
                    if let Some(delay) = VIDEO_RETRY_DELAYS.get(slot.retries) {
                        slot.retries += 1;
                        slot.retry_at = Some(now + *delay);
                        next = Some(next.map_or(*delay, |n| n.min(*delay)));
                    }
                }
                continue;
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
            if let (Some(gpu), Some(scene3d)) = (self.gpu.as_ref(), self.scene3d.as_mut()) {
                if scene3d.set_media_cover(gpu, &self.ctx.media.cover_path) {
                    self.dirty = true;
                }
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
        if let Some(gpu) = self.gpu.as_ref() {
            for w in &mut self.widgets {
                w.mark_fx_changed(gpu, changed, refresh_deps, &mut self.ctx);
            }
        }
        // Fórmulas dos bindings 3D leem o mesmo contexto: reavaliar neste quadro.
        self.formulas_sec = i64::MIN;
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
        if !apply_touch_action(&action, &mut self.ctx.globals, self.scene.as_mut()) {
            return;
        }
        self.formulas_sec = i64::MIN;
        if let Some(renderer) = self.renderer.as_mut() {
            if renderer.mark_dirty(DirtyFlags::GLOBALS) {
                self.dirty = true;
            }
        }
    }

    /// Solta superfícies (antes das janelas) e, se `with_gpu`, o device.
    fn release_gpu_resources(&mut self, with_gpu: bool) {
        // Ordem: superfícies/canvases → caches de bind groups do vidro → renderer → device.
        self.outputs.clear();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.release_glass_gpu();
        }
        if with_gpu {
            // Widgets (superfícies DComp + 3D próprio) caem antes do device; voltam de
            // `active_widgets.json` no próximo `rebuild_outputs`.
            self.widgets.clear();
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

    /// Pastas de fontes do projeto (`%APPDATA%\PrismView\fonts` + `fonts/` ao lado do executável).
    fn font_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::with_capacity(2);
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("PrismView").join("fonts"));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let bundled = parent.join("fonts");
                if !dirs.iter().any(|d| d == &bundled) {
                    dirs.push(bundled);
                }
            }
        }
        dirs
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
        renderer.set_font_resolver(Box::new(resolve_font_files));
        for dir in Self::font_dirs() {
            if dir.is_dir() {
                renderer.register_font_dir(&dir);
            }
        }
        if let Some(scene) = self.scene.clone() {
            self.ctx.now = Local::now();
            self.ctx.globals = scene.globals_map();
            renderer.set_scene(scene, &self.ctx);
            self.needs_mem_trim = true;
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

        if !self.wallpaper_wanted {
            // Só widgets: nenhuma janela no WorkerW (o fundo do Windows segue visível).
            host.detach_all_wallpapers();
            if self.widgets.is_empty() && self.ensure_gpu() {
                self.restore_widgets(host);
            }
            self.refresh_widget_monitors(host);
            self.rebuild_attempts = 0;
            self.retry_at = None;
            self.governor.invalidate_frozen();
            return;
        }

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
                        // A swapchain de bootstrap sai antes do alvo DirectComposition da janela.
                        drop(surface);
                        self.gpu = Some(gpu);
                    }
                    Err(err) => {
                        eprintln!("[prismview-engine] gpu: {err}");
                        continue;
                    }
                }
            }
            let Some(gpu) = self.gpu.as_ref() else {
                continue;
            };
            // Superfície DirectComposition (sem `Present`: overlays de jogos — RivaTuner/MSI
            // Afterburner, NVIDIA, Steam… — não se penduram no wallpaper). Swapchain só de reserva.
            let surface = match wallpaper_presenter(gpu, window, width, height) {
                Ok(presenter) => Ok(gpu.create_external_surface(presenter, width, height)),
                Err(err) => {
                    eprintln!("[prismview-engine] DirectComposition {monitor_id}: {err}; usando swapchain");
                    gpu.create_surface(handles, width, height)
                }
            };
            match surface {
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
        // Device recriado (perda de device): widgets voltam do estado persistido.
        if self.widgets.is_empty() && self.gpu.is_some() {
            self.restore_widgets(host);
        }
        self.refresh_widget_monitors(host);
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
        let scene = self.scene.as_ref();

        for output in self.outputs.iter_mut() {
            if host.monitor_visibility(&output.monitor_id) != DesktopVisibility::Visible {
                continue; // coberto/bloqueado: volta no VisibilityChanged
            }
            let (sw, sh) = output.surface.size();
            let ui_scale = scene
                .and_then(|s| s.ui_scale(sw as f32, sh as f32))
                .unwrap_or(output.scale);
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

    /// Desenha os widgets visíveis com mudança. `true` se algum apresentou.
    fn render_widgets(&mut self, host: &Win32DesktopHost, now: Instant) -> bool {
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };
        let mut presented = false;
        let mut device_lost = false;
        let mut broken: Option<usize> = None;
        for (i, w) in self.widgets.iter_mut().enumerate() {
            if !w.dirty || w.retry_at.is_some_and(|at| now < at) || !Self::widget_visible(host, w) {
                continue;
            }
            w.dirty = false;
            w.retry_at = None;
            match w.render(gpu, &mut self.ctx) {
                FrameStatus::Presented => {
                    self.frames += 1;
                    presented = true;
                }
                FrameStatus::Retry => {
                    w.dirty = true;
                    w.retry_at = Some(now + RETRY_INTERVAL);
                }
                FrameStatus::Unavailable => broken = Some(i),
                FrameStatus::DeviceLost => device_lost = true,
            }
        }
        if device_lost {
            self.release_gpu_resources(true);
            self.rebuild_attempts = 0;
            self.schedule_rebuild();
        } else if let Some(i) = broken {
            // Janela/cadeia inutilizável: recria só este widget a partir do estado persistido.
            let id = self.widgets[i].spec.id.clone();
            self.widgets.remove(i);
            eprintln!("[prismview-engine] widget {id}: superfície perdida, recriando");
            self.restore_widgets(host);
        }
        presented
    }

    fn handle_ipc(&mut self, host: &mut Win32DesktopHost, request: IpcRequest) -> IpcResponse {
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
                    pinned_widget_ids: self.widgets.iter().map(|w| w.spec.id.clone()).collect(),
                    active_monitor_count: u32::try_from(self.outputs.len()).unwrap_or(u32::MAX),
                    rendered_frames: self.frames,
                    errors: self.status_issues(),
                    gpu_adapter: self
                        .gpu
                        .as_ref()
                        .map_or_else(|| "Unavailable".to_string(), |g| g.adapter_name().to_string()),
                    power_state: self.governor.state().as_str().to_string(),
                },
            },
            IpcRequest::LoadScene { scene, .. } => {
                let res = self.load_scene(host, *scene);
                self.schedule_windows_wallpaper_sync();
                res
            }
            IpcRequest::LoadSceneFile { path, sync_windows } => match Self::read_allowed_scene_file(&path) {
                Ok(scene) => {
                    let res = self.load_scene(host, scene);
                    if sync_windows {
                        self.schedule_windows_wallpaper_sync();
                    }
                    res
                }
                Err(message) => IpcResponse::Error { message },
            },
            IpcRequest::ClearDesktop { color } => {
                if !self.wallpaper_wanted || self.outputs.is_empty() {
                    self.wallpaper_wanted = true;
                    self.rebuild_outputs(host);
                }
                // Estado limpo mesmo sem GPU: o próximo rebuild já nasce sem a cena antiga.
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
                self.tray_sync_needed = true;
                if !paused {
                    self.dirty = true;
                }
                IpcResponse::Ok
            }
            IpcRequest::SetTrayVisible { visible } => {
                self.tray_visible = visible;
                self.tray_sync_needed = true;
                if let Some(dir) = Self::state_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(dir.join(TRAY_ENABLED_FILE), if visible { "1" } else { "0" });
                }
                IpcResponse::Ok
            }
            IpcRequest::SetSyncWindowsWallpaper { enabled } => {
                self.sync_windows_wallpaper = enabled;
                if let Some(dir) = Self::state_dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(
                        dir.join(SYNC_WINDOWS_WALLPAPER_FILE),
                        if enabled { "1" } else { "0" },
                    );
                }
                if enabled {
                    self.schedule_windows_wallpaper_sync();
                } else {
                    self.pending_win_wallpaper = None;
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
            IpcRequest::ClearWallpaper => {
                self.clear_wallpaper(host);
                IpcResponse::Ok
            }
            IpcRequest::UpsertWidget {
                id,
                scene_path,
                scene,
                x,
                y,
                width,
                height,
                always_on_top,
                locked,
                click_through,
            } => {
                let scene = match scene {
                    Some(scene) => *scene,
                    None => match Self::read_allowed_scene_file(&scene_path) {
                        Ok(scene) => scene,
                        Err(message) => return IpcResponse::Error { message },
                    },
                };
                let spec = NativeWidgetSpec {
                    id,
                    scene_path,
                    x,
                    y,
                    width,
                    height,
                    always_on_top,
                    locked,
                    click_through,
                };
                match self.upsert_widget(host, spec, scene) {
                    Ok(()) => {
                        self.persist_widgets();
                        IpcResponse::Ok
                    }
                    Err(message) => IpcResponse::Error { message },
                }
            }
            IpcRequest::RemoveWidget { id } => {
                if self.remove_widget(&id) {
                    self.persist_widgets();
                }
                self.exit_if_idle();
                IpcResponse::Ok
            }
            IpcRequest::UpdateWidgetBounds { id, x, y, width, height } => {
                let Some(w) = self.widgets.iter_mut().find(|w| w.spec.id == id) else {
                    return IpcResponse::Error { message: "widget_not_found".into() };
                };
                let width = width.clamp(prism_core::schema::NATIVE_WIDGET_MIN_SIDE, prism_core::schema::NATIVE_WIDGET_MAX_SIDE);
                let height = height.clamp(prism_core::schema::NATIVE_WIDGET_MIN_SIDE, prism_core::schema::NATIVE_WIDGET_MAX_SIDE);
                w.set_bounds(x, y, width, height);
                self.refresh_widget_monitors(host);
                self.persist_widgets();
                IpcResponse::Ok
            }
            IpcRequest::SetWidgetFlags { id, always_on_top, locked, click_through } => {
                let Some(w) = self.widgets.iter_mut().find(|w| w.spec.id == id) else {
                    return IpcResponse::Error { message: "widget_not_found".into() };
                };
                w.set_flags(
                    always_on_top.unwrap_or(w.spec.always_on_top),
                    locked.unwrap_or(w.spec.locked),
                    click_through.unwrap_or(w.spec.click_through),
                );
                w.dirty = true;
                self.persist_widgets();
                IpcResponse::Ok
            }
            IpcRequest::ListWidgets => IpcResponse::Widgets {
                widgets: self.widgets.iter().map(|w| w.spec.clone()).collect(),
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
                for w in &mut self.widgets {
                    w.dirty = true;
                }
                self.governor.invalidate_frozen();
            }
        }
    }

    fn woken(&mut self, host: &mut Win32DesktopHost) {
        while let Ok(envelope) = self.ipc_rx.try_recv() {
            let IpcEnvelope { req, reply } = envelope;
            match req {
                // Responde logo após validar/parsear: vídeo, imagens e 3D carregam depois sem
                // segurar o Studio (antes: `engine_timeout` com a cena aparecendo segundos depois).
                IpcRequest::LoadSceneFile { path, sync_windows } => match Self::read_allowed_scene_file(&path) {
                    Ok(scene) => {
                        let _ = reply.send(IpcResponse::Ok);
                        self.load_scene(host, scene);
                        if sync_windows {
                            self.schedule_windows_wallpaper_sync();
                        }
                    }
                    Err(message) => {
                        eprintln!("[prismview-engine] LOAD_SCENE_FILE: {message}");
                        let _ = reply.send(IpcResponse::Error { message });
                    }
                },
                req => {
                    let response = self.handle_ipc(host, req);
                    let _ = reply.send(response);
                }
            }
        }
    }

    fn repaint_requested(&mut self, _host: &mut Win32DesktopHost) {
        self.dirty = true;
        for w in &mut self.widgets {
            w.dirty = true;
        }
        self.governor.invalidate_frozen();
    }

    fn tray_open_requested(&mut self, _host: &mut Win32DesktopHost) {
        if let Err(err) = open_studio() {
            eprintln!("[prismview-engine] bandeja (abrir studio): {err}");
        }
    }

    fn tray_menu_requested(&mut self, host: &mut Win32DesktopHost) {
        let Some(action) = host.show_tray_menu(self.paused) else {
            return;
        };
        match action {
            TrayAction::OpenStudio => {
                if let Err(err) = open_studio() {
                    eprintln!("[prismview-engine] bandeja (abrir studio): {err}");
                }
            }
            TrayAction::TogglePause => {
                self.paused = !self.paused;
                host.set_tray_paused(self.paused);
                if !self.paused {
                    self.dirty = true;
                    self.governor.invalidate_frozen();
                }
            }
            TrayAction::RemoveWallpaper => {
                // Widgets nativos continuam; sem eles o motor encerra (`exit_if_idle`).
                self.clear_wallpaper(host);
            }
            TrayAction::QuitEngine => {
                self.exit = true;
            }
        }
    }

    fn widgets_moved(&mut self, host: &mut Win32DesktopHost) {
        let mut changed = false;
        for w in &mut self.widgets {
            changed |= w.sync_bounds_from_window();
        }
        if changed {
            self.refresh_widget_monitors(host);
            self.persist_widgets();
            self.governor.invalidate_frozen();
        }
    }

    fn widget_clicked(&mut self, _host: &mut Win32DesktopHost, window: NativeWindowId, point: (i32, i32)) {
        if let Some(index) = self.widgets.iter().position(|w| w.window_id() == window) {
            self.widget_tap(index, [point.0 as f32, point.1 as f32]);
            self.governor.invalidate_frozen();
        }
    }

    fn about_to_wait(&mut self, host: &mut Win32DesktopHost) -> ControlFlow {
        if self.tray_sync_needed {
            self.tray_sync_needed = false;
            host.set_tray_visible(self.tray_visible);
            host.set_tray_paused(self.paused);
        }
        if self.exit {
            return ControlFlow::Exit;
        }

        let now = Instant::now();
        if self
            .win_wallpaper_applied
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            if self.wallpaper_wanted {
                // `SPI_SETDESKWALLPAPER` faz o Explorer reordenar/recriar o WorkerW acima da nossa
                // janela viva: redescobre a camada, reanexa e reconstrói o alvo DirectComposition
                // (GPU, 3D e vídeos permanecem vivos; sem flash preto pois o fundo já é a cena limpa).
                let _ = host.reattach_wallpapers();
                self.rebuild_outputs(host);
                self.win_wallpaper_reattach_at = Some(now + Duration::from_millis(650));
            }
        }
        if self.win_wallpaper_reattach_at.is_some_and(|at| now >= at) {
            self.win_wallpaper_reattach_at = None;
            if self.wallpaper_wanted {
                if host.reattach_wallpapers() {
                    self.rebuild_outputs(host);
                } else {
                    self.dirty = true;
                    self.governor.invalidate_frozen();
                }
            }
        }
        if self.rebuild_at.is_some_and(|at| now >= at) {
            self.rebuild_outputs(host);
        }

        // Governador (Fase 5): decide estado/caps antes de qualquer trabalho.
        let inputs = self.governor_inputs(host);
        let update = self.governor.update(now, &inputs);
        self.apply_power(update);

        // Sinais (Fase 4): demanda da cena atual → bus; bindings + toques no desktop.
        self.sync_signal_demand(host);
        self.process_signals(host, now);

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
            for w in &mut self.widgets {
                w.advance_clock(&self.ctx);
            }
            // Fórmulas dos bindings 3D (osso/clipe por hora, mídia, CPU...): ≤ 1×/s.
            let sec = self.ctx.now.timestamp();
            if sec != self.formulas_sec && self.has_formulas() {
                self.formulas_sec = sec;
                self.update_formulas();
            }
        }

        // Modelos que terminaram de carregar → 3D do wallpaper e dos widgets.
        self.poll_loader();
        // Imagens decodificadas em segundo plano → rebuild 2D (upload) no próximo quadro.
        if self.renderer.as_mut().is_some_and(SceneRenderer::poll_images) {
            self.dirty = true;
        }
        for w in &mut self.widgets {
            w.poll_images();
        }

        // 3D: carrega/anima/renderiza a textura antes do passe 2D que a compõe.
        let scene3d_at = self.step_scene3d(host, now);
        // Vídeo: troca o frame exibido (zero-copy) antes do passe 2D.
        let video_at = self.step_videos(now);
        let widget_interval = self.governor.frame_interval();
        let mut widget3d_at: Option<Instant> = None;
        if let Some(gpu) = self.gpu.as_ref() {
            for w in &mut self.widgets {
                let visible = Self::widget_visible(host, w);
                if let Some(at) = w.step_scene3d(gpu, now, animate && visible, widget_interval) {
                    widget3d_at = Some(widget3d_at.map_or(at, |cur: Instant| cur.min(at)));
                }
            }
        }

        // Teto de FPS / pausa / quadro congelado.
        let mut throttle_at = None;
        let due = self.retry_at.map_or(true, |at| now >= at);
        let widgets_dirty = self.widgets.iter().any(|w| w.dirty);
        if (self.dirty && due) || widgets_dirty {
            match self.governor.frame_gate(now) {
                FrameGate::Now => {
                    let mut drew = false;
                    if self.dirty && due {
                        self.dirty = false;
                        self.retry_at = None;
                        drew |= self.render(host);
                    }
                    if widgets_dirty {
                        drew |= self.render_widgets(host, now);
                    }
                    if drew {
                        // Ancora o FrameGate ao início do quadro (`now`), não após o bloqueio do
                        // VSync, garantindo 30 FPS reais em Eco e 15 FPS reais em Ultra.
                        self.governor.frame_presented(now);
                        if self.needs_mem_trim {
                            self.needs_mem_trim = false;
                            trim_working_set();
                        }
                    }
                }
                FrameGate::At(at) => throttle_at = Some(at),
                // Pausado/congelado: `dirty` fica pendente e é atendido na retomada/invalidação.
                FrameGate::Blocked => {}
            }
        }

        // Se a opção "Atualizar wallpaper do Windows" estiver ligada, grava a versão limpa
        // assim que os assets visuais (vídeo/imagens/3D) estiverem prontos na GPU.
        let win_wp_at = self.maybe_sync_windows_wallpaper(now);

        let any_visible = self
            .outputs
            .iter()
            .any(|o| host.monitor_visibility(&o.monitor_id) == DesktopVisibility::Visible);

        // Próximo tique do relógio, só com animação permitida e algum monitor visível.
        let clock_at = if animate && any_visible {
            self.renderer
                .as_ref()
                .and_then(|r| r.next_wake(&self.ctx.now))
                .map(|d| Instant::now() + d)
        } else {
            None
        };
        // Molas dos bindings ainda se movendo: usa `WaitUntil` na cadência alvo (nunca `Poll`,
        // pois em swapchain DWM/flip-model o `present()` pode voltar em ~2 ms e girar a 300+ FPS).
        // Se há 3D ativo, alinha à cadência do 3D (ex.: 60 FPS); em cenas puramente 2D, 16 ms (~60 FPS).
        let bindings_moving = animate
            && any_visible
            && self
                .renderer
                .as_ref()
                .is_some_and(SceneRenderer::bindings_animating);
        let frame_interval = self.governor.frame_interval();
        let binding_cadence = self
            .scene3d
            .as_ref()
            .and_then(Scene3dRenderer::frame_interval)
            .unwrap_or(Duration::from_millis(16))
            .max(frame_interval)
            .max(Duration::from_millis(16));
        let binding_at = bindings_moving.then(|| now + binding_cadence);
        let governor_at = self.governor.next_deadline(now);
        // Caixa 3D recém-construída por este render e 3D ainda sem quadro: volta já.
        let scene3d_at = scene3d_at.or_else(|| {
            let pending = animate
                && self.scene3d.as_ref().is_some_and(Scene3dRenderer::needs_frame)
                && self.scene3d_placement(host).is_some();
            pending.then(Instant::now)
        });
        // Widgets: relógio, molas 2D, 3D pendente e retentativas (só os visíveis acordam o loop).
        let mut widget_at: Option<Instant> = widget3d_at;
        let mut push = |at: Instant| widget_at = Some(widget_at.map_or(at, |cur| cur.min(at)));
        if animate {
            let after_present = Instant::now();
            for w in &self.widgets {
                if !Self::widget_visible(host, w) {
                    continue;
                }
                if let Some(d) = w.next_clock_wake(&self.ctx) {
                    push(after_present + d);
                }
                if w.bindings_animating() {
                    // Widgets não bloqueiam no VSync (cadeia DComp): ritmo próprio ≥ 1/120 s.
                    push(now + frame_interval.max(WIDGET_MIN_FRAME));
                }
                if w.scene3d_pending() {
                    push(after_present);
                }
                if let Some(at) = w.retry_at {
                    push(at);
                }
            }
            // Fórmulas que dependem do relógio: próximo segundo cheio.
            if self.has_formulas() && self.formula_deps.intersects(DirtyFlags::TIME_SECOND.union(DirtyFlags::TIME_MINUTE)) {
                let ms = 1000u64.saturating_sub(u64::from(self.ctx.now.timestamp_subsec_millis()));
                push(after_present + Duration::from_millis(ms.max(1)));
            }
        }
        // Enquanto já há próximo quadro agendado (3D / parallax / throttle), suprime wakes de
        // `MOUSE` (mouses 500-1000 Hz não acordam o loop 1000×/s entre quadros de 60 FPS). O
        // cursor exato continua sendo lido em `process_signals` no instante de cada quadro; ao
        // assentar, `MOUSE` volta imediatamente para `wake_mask` (0% CPU em repouso, <0,1ms wake).
        let frame_scheduled = scene3d_at.is_some()
            || binding_at.is_some()
            || throttle_at.is_some()
            || widget3d_at.is_some();
        let mut mouse_race_wake: Option<Instant> = None;
        if let Some(sensors) = self.sensors.as_ref() {
            let bus = sensors.bus();
            let base = self.scene_demand | SignalKind::POWER;
            if frame_scheduled && base.contains(SignalKind::MOUSE) {
                bus.set_wake_mask(SignalKind(base.0 & !SignalKind::MOUSE.0));
            } else {
                bus.set_wake_mask(base);
                if base.contains(SignalKind::MOUSE) && bus.has_pending(SignalKind::MOUSE) {
                    mouse_race_wake = Some(Instant::now() + Duration::from_millis(8));
                }
            }
        }
        let next = [
            self.retry_at,
            self.rebuild_at,
            clock_at,
            binding_at,
            throttle_at,
            governor_at,
            scene3d_at,
            video_at,
            widget_at,
            win_wp_at,
            self.win_wallpaper_reattach_at,
            mouse_race_wake,
        ]
        .into_iter()
        .flatten()
        .min();
        match next {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        }
    }

    fn exiting(&mut self, host: &mut Win32DesktopHost) {
        // Remove o ícone da bandeja (NIM_DELETE) antes de liberar superfícies/device.
        host.set_tray_visible(false);
        // Widgets (cadeias DComp esperam a GPU) → superfícies → device, antes de o host
        // destruir as janelas: nenhuma janela órfã fica no desktop.
        self.widgets.clear();
        self.release_gpu_resources(true);
    }
}
