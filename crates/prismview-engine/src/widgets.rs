//! Widgets nativos (Entrega 3): janelas DirectComposition transparentes no mesmo processo do
//! wallpaper, reaproveitando **o mesmo** `GpuContext`, `Scene3dShared`, `AssetLoader` e
//! `SignalBus`. Cada widget é leve: janela + `WindowSurface` externa (cadeia DComp + 1 textura) +
//! `SceneRenderer` + `Canvas` + `Option<Scene3dRenderer>`.
//!
//! Estado persistido em `%APPDATA%\PrismView\native\active_widgets.json` (posição após arrasto
//! incluída): o motor recria os widgets sozinho no login, mesmo com o Studio fechado.
//!
//! Funções puras no topo (persistência, demanda combinada, coordenadas de cursor/toque) são
//! testadas sem Win32/GPU.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use prism_core::fx::EvalContext;
use prism_core::schema::{ActiveWidgetsFile, NativeWidgetSpec, PrismScene, TouchAction, TouchActionType};

use crate::engine_core::assets::{AssetLoader, LoadEvent};
use crate::engine_core::compositor::Canvas;
use crate::engine_core::gpu::{FrameStatus, GpuContext, WindowSurface};
use crate::engine_core::interaction::DesktopAction;
use crate::engine_core::scene2d::SceneRenderer;
use crate::engine_core::scene3d::{Scene3dConfig, Scene3dOptions, Scene3dRenderer, Scene3dShared};
use crate::engine_core::signals::{SignalKind, SignalSnapshot};
use crate::platform::windows::{resolve_font_files, Win32DesktopHost, WidgetWindow, WidgetWindowConfig};

/// Arquivo de estado dos widgets (dentro de `native/`).
pub const ACTIVE_WIDGETS_FILE: &str = "active_widgets.json";
/// Pasta das cenas dos widgets (`native/widgets/<id>.json`).
pub const WIDGET_SCENES_DIR: &str = "widgets";
/// `output_generation` do 3D ainda não ligada ao compositor.
const SCENE3D_UNBOUND: u64 = u64::MAX;
/// Alcance padrão do olhar quando o monitor do widget é desconhecido (meia tela 1080p).
const DEFAULT_LOOK_REACH: (f32, f32) = (960.0, 540.0);

/// Retângulo `(x, y, w, h)` em px físicos do desktop virtual.
pub type Bounds = (i32, i32, u32, u32);

// ───────────────────────── Funções puras ─────────────────────────

/// Lê `active_widgets.json` (ausente/inválido = vazio).
pub fn load_active_widgets(dir: &Path) -> Vec<NativeWidgetSpec> {
    std::fs::read_to_string(dir.join(ACTIVE_WIDGETS_FILE))
        .map(|raw| ActiveWidgetsFile::parse(&raw).widgets)
        .unwrap_or_default()
}

/// Grava `active_widgets.json` de forma atômica (tmp + rename). Lista vazia apaga o arquivo.
pub fn save_active_widgets(dir: &Path, widgets: &[NativeWidgetSpec]) -> std::io::Result<()> {
    let path = dir.join(ACTIVE_WIDGETS_FILE);
    if widgets.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir)?;
    let file = ActiveWidgetsFile {
        widgets: widgets.to_vec(),
        ..ActiveWidgetsFile::default()
    };
    let tmp = dir.join(format!("{ACTIVE_WIDGETS_FILE}.tmp"));
    std::fs::write(&tmp, file.to_json())?;
    std::fs::rename(&tmp, &path)
}

/// Caminho padrão da cena de um widget.
pub fn widget_scene_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(WIDGET_SCENES_DIR).join(format!("{id}.json"))
}

/// Demanda do bus = wallpaper OR widgets visíveis (os ocultos não pedem nada) + `POWER`.
pub fn combined_demand(wallpaper: SignalKind, widgets: impl IntoIterator<Item = (bool, SignalKind)>) -> SignalKind {
    let mut demand = wallpaper;
    for (visible, kind) in widgets {
        if visible {
            demand |= kind;
        }
    }
    demand | SignalKind::POWER
}

/// Ponto da tela → px locais do widget (`None` fora dele).
pub fn local_point(point: (i32, i32), bounds: Bounds) -> Option<[f32; 2]> {
    let (x, y, w, h) = bounds;
    let lx = i64::from(point.0) - i64::from(x);
    let ly = i64::from(point.1) - i64::from(y);
    (lx >= 0 && ly >= 0 && lx < i64::from(w) && ly < i64::from(h)).then(|| [lx as f32, ly as f32])
}

/// Cursor da tela inteira → 0..1 para "olhar para o mouse": centro do widget = 0,5; meia largura/
/// altura do monitor dele = borda (limitado). O personagem segue o mouse em qualquer lugar.
pub fn look_pointer(cursor: (i32, i32), bounds: Bounds, monitor: Option<Bounds>) -> [f32; 2] {
    let (x, y, w, h) = bounds;
    let cx = x as f32 + w as f32 * 0.5;
    let cy = y as f32 + h as f32 * 0.5;
    let (rx, ry) = monitor.map_or(DEFAULT_LOOK_REACH, |(_, _, mw, mh)| {
        ((mw as f32 * 0.5).max(1.0), (mh as f32 * 0.5).max(1.0))
    });
    let nx = ((cursor.0 as f32 - cx) / rx).clamp(-1.0, 1.0);
    let ny = ((cursor.1 as f32 - cy) / ry).clamp(-1.0, 1.0);
    [0.5 + nx * 0.5, 0.5 + ny * 0.5]
}

/// Monitor que contém o centro do widget (o mais próximo se o centro caiu fora de todos).
pub fn monitor_for<'a>(bounds: Bounds, monitors: impl IntoIterator<Item = (&'a str, Bounds)>) -> Option<(&'a str, Bounds)> {
    let (x, y, w, h) = bounds;
    let cx = i64::from(x) + i64::from(w) / 2;
    let cy = i64::from(y) + i64::from(h) / 2;
    let mut best: Option<(i64, (&'a str, Bounds))> = None;
    for (id, m) in monitors {
        let (mx, my, mw, mh) = (i64::from(m.0), i64::from(m.1), i64::from(m.2), i64::from(m.3));
        let dx = (mx - cx).max(cx - (mx + mw - 1)).max(0);
        let dy = (my - cy).max(cy - (my + mh - 1)).max(0);
        let dist = dx * dx + dy * dy;
        if best.map_or(true, |(d, _)| dist < d) {
            best = Some((dist, (id, m)));
        }
    }
    best.map(|(_, m)| m)
}

/// Aplica uma ação de toque não-mídia sobre `globals`/`scene`. Devolve `true` se globais mudaram
/// (o chamador marca `DirtyFlags::GLOBALS`). Compartilhado por wallpaper e widgets.
pub fn apply_touch_action(action: &TouchAction, globals: &mut HashMap<String, String>, scene: Option<&mut PrismScene>) -> bool {
    let key = action.global_name.trim().to_lowercase();
    let new_value = match action.action_type {
        TouchActionType::OpenUrl => {
            if let Err(err) = crate::platform::windows::shell_open(&action.url, true) {
                eprintln!("[prismview-engine] abrir URL: {err}");
            }
            None
        }
        TouchActionType::OpenApp => {
            if let Err(err) = crate::platform::windows::shell_open(&action.target_exec, false) {
                eprintln!("[prismview-engine] abrir app: {err}");
            }
            None
        }
        TouchActionType::ToggleGlobal if !key.is_empty() => {
            let cur = globals.get(&key).map(|v| v.trim().to_ascii_lowercase());
            let on = matches!(cur.as_deref(), Some(v) if !v.is_empty() && v != "0" && v != "false");
            Some(if on { "0" } else { "1" }.to_string())
        }
        TouchActionType::SetGlobal if !key.is_empty() => Some(action.global_value.clone()),
        TouchActionType::NextGlobalList if !key.is_empty() => {
            let options = scene
                .as_deref()
                .and_then(|s| s.globals.iter().find(|g| g.name.to_lowercase() == key))
                .map(|g| g.options.clone())
                .unwrap_or_default();
            if options.is_empty() {
                None
            } else {
                let cur = globals.get(&key).cloned().unwrap_or_default();
                let at = options.iter().position(|o| *o == cur).map_or(0, |i| (i + 1) % options.len());
                options.get(at).cloned()
            }
        }
        _ => None,
    };
    let Some(value) = new_value else {
        return false;
    };
    globals.insert(key.clone(), value.clone());
    if let Some(scene) = scene {
        scene.set_global(&key, value);
    }
    true
}

/// Texto FX → número (`"42"`, `"42%"`, `" 3.5 "`). `None` se não numérico.
pub fn parse_formula_number(raw: &str) -> Option<f32> {
    raw.trim().trim_end_matches('%').trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

// ───────────────────────── Runtime ─────────────────────────

/// Recursos compartilhados que o widget precisa para (re)montar o 3D.
pub struct Scene3dDeps<'a> {
    pub shared: &'a Arc<Scene3dShared>,
    pub loader: &'a mut AssetLoader,
    pub waker: Option<Arc<dyn Fn() + Send + Sync>>,
    pub render_scale_cap: f32,
    pub msaa_cap: u32,
}

/// Um widget no ar. Ordem dos campos = ordem de drop: 3D → canvas → renderer → superfície
/// (o presenter espera a última cópia na GPU) → janela.
pub struct WidgetRuntime {
    scene3d: Option<Scene3dRenderer>,
    canvas: Canvas,
    renderer: SceneRenderer,
    surface: WindowSurface,
    window: WidgetWindow,
    pub spec: NativeWidgetSpec,
    scene: PrismScene,
    /// Globais desta cena (trocadas com o `EvalContext` comum só durante o uso: sem alocar).
    globals: HashMap<String, String>,
    canvas_key: String,
    scene3d_bound_gen: u64,
    scene3d_last_update: Option<Instant>,
    scene3d_next_at: Option<Instant>,
    pub dirty: bool,
    pub retry_at: Option<Instant>,
    /// Monitor que contém o widget (visibilidade/cobertura) e seu retângulo.
    pub monitor: Option<(String, Bounds)>,
}

impl WidgetRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: &Win32DesktopHost,
        gpu: &GpuContext,
        spec: NativeWidgetSpec,
        scene: PrismScene,
        font_dirs: &[PathBuf],
        post_processing: bool,
        ctx: &mut EvalContext,
    ) -> Result<Self, String> {
        let cfg = WidgetWindowConfig {
            x: spec.x,
            y: spec.y,
            width: spec.width,
            height: spec.height,
            always_on_top: spec.always_on_top,
            locked: spec.locked,
            click_through: spec.click_through,
        };
        let window = host.create_widget_window(&cfg).map_err(|e| format!("widget_window: {e}"))?;
        let presenter = host
            .create_widget_presenter(gpu, &window, spec.width, spec.height)
            .map_err(|e| format!("widget_composition: {e}"))?;
        let surface = gpu.create_external_surface(Box::new(presenter), spec.width, spec.height);
        let mut renderer = SceneRenderer::new(gpu, surface.format());
        renderer.set_post_processing(post_processing);
        renderer.set_font_resolver(Box::new(resolve_font_files));
        for dir in font_dirs {
            if dir.is_dir() {
                renderer.register_font_dir(dir);
            }
        }
        let canvas = renderer.create_canvas(gpu);
        let canvas_key = format!("widget:{}", spec.id);
        let mut this = Self {
            scene3d: None,
            canvas,
            renderer,
            surface,
            window,
            globals: scene.globals_map(),
            scene: scene.clone(),
            spec,
            canvas_key,
            scene3d_bound_gen: SCENE3D_UNBOUND,
            scene3d_last_update: None,
            scene3d_next_at: None,
            dirty: true,
            retry_at: None,
            monitor: None,
        };
        this.with_globals(ctx, |r, ctx| r.set_scene(scene, ctx));
        Ok(this)
    }

    /// Roda `f` com as globais deste widget no contexto comum (troca de ponteiros, sem cópia).
    fn with_globals<R>(&mut self, ctx: &mut EvalContext, f: impl FnOnce(&mut SceneRenderer, &EvalContext) -> R) -> R {
        std::mem::swap(&mut ctx.globals, &mut self.globals);
        let out = f(&mut self.renderer, ctx);
        std::mem::swap(&mut ctx.globals, &mut self.globals);
        out
    }

    pub fn window_id(&self) -> crate::platform::NativeWindowId {
        self.window.id()
    }

    pub fn scene(&self) -> &PrismScene {
        &self.scene
    }

    pub fn bounds(&self) -> Bounds {
        (self.spec.x, self.spec.y, self.spec.width, self.spec.height)
    }

    /// Troca a cena (mesmo renderer/fontes/pipelines).
    pub fn set_scene(&mut self, scene: PrismScene, ctx: &mut EvalContext) {
        self.globals = scene.globals_map();
        self.scene = scene.clone();
        self.with_globals(ctx, |r, ctx| r.set_scene(scene, ctx));
        self.dirty = true;
    }

    /// (Re)lê o `Scene3DConfig` da cena; sem 3D = solta o renderer 3D.
    pub fn refresh_scene3d(&mut self, gpu: &GpuContext, deps: Option<Scene3dDeps<'_>>, cover: &str) {
        let config = self
            .renderer
            .scene3d_config()
            .and_then(|raw| serde_json::from_value::<Scene3dConfig>(raw.clone()).ok());
        let (Some(config), Some(deps)) = (config, deps) else {
            if self.scene3d.take().is_some() {
                self.renderer.clear_scene3d_output();
            }
            self.scene3d_bound_gen = SCENE3D_UNBOUND;
            return;
        };
        let scene3d = self.scene3d.get_or_insert_with(|| {
            let mut s = Scene3dRenderer::new(Arc::clone(deps.shared), gpu, Scene3dOptions::default());
            s.set_waker(deps.waker.clone());
            s
        });
        scene3d.set_render_scale_cap(deps.render_scale_cap);
        scene3d.set_msaa_cap(deps.msaa_cap);
        scene3d.set_config(gpu, config, deps.loader);
        scene3d.set_media_cover(gpu, cover);
        self.scene3d_bound_gen = SCENE3D_UNBOUND;
        self.scene3d_last_update = None;
        self.scene3d_next_at = None;
        self.dirty = true;
    }

    #[allow(dead_code)]
    pub fn has_scene3d(&self) -> bool {
        self.scene3d.is_some()
    }

    pub fn handle_load_event(&mut self, gpu: &GpuContext, event: &LoadEvent) {
        if let Some(s) = self.scene3d.as_mut() {
            s.handle_load_event(gpu, event);
        }
    }

    pub fn set_render_scale_cap(&mut self, cap: f32, msaa_cap: u32) {
        if let Some(s) = self.scene3d.as_mut() {
            s.set_render_scale_cap(cap);
            s.set_msaa_cap(msaa_cap);
        }
        self.scene3d_next_at = None;
    }

    pub fn set_post_processing(&mut self, enabled: bool) {
        if self.renderer.set_post_processing(enabled) {
            self.dirty = true;
        }
    }

    /// Hibernação de VRAM (governador): solta blur do vidro e alvos 3D; volta sob demanda.
    pub fn hibernate(&mut self) {
        self.renderer.release_glass_gpu();
        self.renderer.clear_scene3d_output();
        if let Some(s) = self.scene3d.as_mut() {
            s.release_targets();
        }
        self.scene3d_bound_gen = SCENE3D_UNBOUND;
    }

    /// Sinais que este widget consome.
    pub fn signal_demand(&self) -> SignalKind {
        let d2 = if self.renderer.has_scene() {
            self.renderer.signal_demand()
        } else {
            SignalKind::NONE
        };
        let d3 = self.scene3d.as_ref().map_or(SignalKind::NONE, |s| {
            // CURSOR_LOOK/órbita/hover leem o cursor global.
            let cursor = if s.wants_cursor() { SignalKind::MOUSE } else { SignalKind::NONE };
            s.signal_demand() | cursor
        });
        // Toques (controles 2D / CLICK_MESH) já pedem DESKTOP_TAP: é por ele que cliques em
        // widgets click-through voltam ao motor.
        d2 | d3
    }

    /// Bindings 2D + cursor/sinais do 3D para o quadro. Sem alocação.
    pub fn apply_signals(&mut self, snap: &SignalSnapshot, dt: f32, cursor: Option<(i32, i32)>) {
        if self.renderer.apply_signals(snap, dt) {
            self.dirty = true;
        }
        if let Some(s) = self.scene3d.as_mut() {
            s.set_signals(snap);
            let bounds = (self.spec.x, self.spec.y, self.spec.width, self.spec.height);
            let (hover, look) = match cursor {
                Some(c) => {
                    let hover = local_point(c, bounds).and_then(|p| {
                        SceneRenderer::scene3d_placement(&self.canvas).and_then(|pl| pl.to_local(p))
                    });
                    (hover, Some(look_pointer(c, bounds, self.monitor.as_ref().map(|m| m.1))))
                }
                None => (None, None),
            };
            s.set_pointers(hover, look);
        }
    }

    /// Contexto FX mudou (`changed` = flags). Refaz dependências/lista e a capa do 3D.
    pub fn mark_fx_changed(&mut self, gpu: &GpuContext, changed: prism_core::schema::DirtyFlags, refresh_deps: bool, ctx: &mut EvalContext) {
        let cover_changed = changed.contains(prism_core::schema::DirtyFlags::MEDIA);
        self.with_globals(ctx, |r, ctx| {
            if refresh_deps {
                r.refresh_deps(ctx);
            }
        });
        if self.renderer.mark_dirty(changed) {
            self.dirty = true;
        }
        if cover_changed {
            if let Some(s) = self.scene3d.as_mut() {
                if s.set_media_cover(gpu, &ctx.media.cover_path) {
                    self.dirty = true;
                }
            }
        }
    }

    pub fn advance_clock(&mut self, ctx: &EvalContext) -> bool {
        let changed = self.renderer.advance_clock(&ctx.now);
        self.dirty |= changed;
        changed
    }

    pub fn next_clock_wake(&self, ctx: &EvalContext) -> Option<Duration> {
        self.renderer.next_wake(&ctx.now)
    }

    pub fn bindings_animating(&self) -> bool {
        self.renderer.bindings_animating()
    }

    pub fn has_formulas(&self) -> bool {
        self.scene3d.as_ref().is_some_and(Scene3dRenderer::has_formulas)
    }

    /// Reavalia fórmulas dos bindings 3D com as globais deste widget (≤ 1×/s ou na mudança).
    /// Devolve as dependências FX das fórmulas (relógio/mídia/telemetria).
    pub fn update_formulas(&mut self, ctx: &mut EvalContext) -> prism_core::schema::DirtyFlags {
        let mut deps = prism_core::schema::DirtyFlags::NONE;
        let Some(scene3d) = self.scene3d.as_mut() else {
            return deps;
        };
        std::mem::swap(&mut ctx.globals, &mut self.globals);
        let changed = scene3d.update_formulas(|expr| {
            let out = prism_core::fx::FxEngine::eval_expr(expr, ctx);
            deps.insert(out.deps);
            parse_formula_number(&out.value)
        });
        std::mem::swap(&mut ctx.globals, &mut self.globals);
        if changed {
            self.scene3d_next_at = None;
        }
        deps
    }

    /// Toque (px locais): controle 2D primeiro; senão clique 3D (CLICK_MESH).
    pub fn tap(&mut self, local: [f32; 2]) -> Option<DesktopAction> {
        if let Some(action) = SceneRenderer::hit_test(&self.canvas, local) {
            return Some(action);
        }
        let p = SceneRenderer::scene3d_placement(&self.canvas).and_then(|pl| pl.to_local(local))?;
        if let Some(s) = self.scene3d.as_mut() {
            if s.click(p) {
                self.scene3d_next_at = None;
            }
        }
        None
    }

    /// Ação não-mídia `index` deste widget (globais da própria cena).
    pub fn run_touch_action(&mut self, index: u32) {
        let Some(action) = self.renderer.touch_action(index).cloned() else {
            return;
        };
        if apply_touch_action(&action, &mut self.globals, Some(&mut self.scene))
            && self.renderer.mark_dirty(prism_core::schema::DirtyFlags::GLOBALS)
        {
            self.dirty = true;
        }
    }

    /// 3D: update/render no teto de FPS; liga a textura nova ao compositor. Devolve o próximo
    /// instante em que o 3D precisa de quadro (`None` = só por evento).
    pub fn step_scene3d(&mut self, gpu: &GpuContext, now: Instant, animate: bool, min_interval: Duration) -> Option<Instant> {
        let scene3d = self.scene3d.as_mut()?;
        if !animate {
            self.scene3d_last_update = None;
            return None;
        }
        let Some(placement) = SceneRenderer::scene3d_placement(&self.canvas) else {
            // Lista 2D ainda não montada: o 1º render cria a caixa e volta aqui.
            self.dirty = true;
            return None;
        };
        let (w, h) = placement.pixel_size();
        scene3d.set_viewport(w, h);
        let cadence = scene3d.frame_interval().unwrap_or(Duration::ZERO).max(min_interval);
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
                label: Some("prismview_widget_scene3d"),
            });
            if scene3d.render(gpu, &mut encoder) {
                gpu.queue().submit(Some(encoder.finish()));
                let generation = scene3d.output_generation();
                if generation != self.scene3d_bound_gen {
                    if let (Some(texture), Some(size)) = (scene3d.output_texture(), scene3d.output_size()) {
                        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                        self.renderer.set_scene3d_output(gpu, view, size);
                        self.scene3d_bound_gen = generation;
                    }
                }
                self.renderer.scene3d_frame_rendered();
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

    /// Imagens decodificadas em segundo plano ficaram prontas: redesenha o widget.
    pub fn poll_images(&mut self) {
        if self.renderer.poll_images() {
            self.dirty = true;
        }
    }

    /// 3D com quadro pendente (caixa pronta): o loop deve voltar já.
    pub fn scene3d_pending(&self) -> bool {
        self.scene3d.as_ref().is_some_and(Scene3dRenderer::needs_frame)
            && SceneRenderer::scene3d_placement(&self.canvas).is_some()
    }

    /// Desenha e apresenta (alpha pré-multiplicado, fundo transparente).
    pub fn render(&mut self, gpu: &GpuContext, ctx: &mut EvalContext) -> FrameStatus {
        let (w, h) = self.surface.size();
        let ui_scale = match self.scene.logical_size {
            Some([lw, lh]) if lw > 0.0 && lh > 0.0 => (w as f32 / lw).min(h as f32 / lh),
            _ => 1.0,
        };
        std::mem::swap(&mut ctx.globals, &mut self.globals);
        let status = self.renderer.render(
            gpu,
            &mut self.surface,
            &mut self.canvas,
            &self.canvas_key,
            ui_scale,
            ctx,
            [0.0, 0.0, 0.0, 0.0],
        );
        std::mem::swap(&mut ctx.globals, &mut self.globals);
        status
    }

    /// Aplica bounds pedidos pelo Studio.
    pub fn set_bounds(&mut self, x: i32, y: i32, width: u32, height: u32) {
        self.window.set_bounds(x, y, width, height);
        self.spec.x = x;
        self.spec.y = y;
        self.spec.width = width;
        self.spec.height = height;
        self.surface.resize(width, height);
        self.dirty = true;
    }

    pub fn set_flags(&mut self, always_on_top: bool, locked: bool, click_through: bool) {
        self.spec.always_on_top = always_on_top;
        self.spec.locked = locked;
        self.spec.click_through = click_through;
        self.window.set_flags(always_on_top, locked, click_through);
    }

    /// Relê o retângulo real da janela (após arrasto/DPI). `true` se mudou.
    pub fn sync_bounds_from_window(&mut self) -> bool {
        let Some((x, y, w, h)) = self.window.bounds() else {
            return false;
        };
        if (x, y, w, h) == self.bounds() {
            return false;
        }
        self.spec.x = x;
        self.spec.y = y;
        if (w, h) != (self.spec.width, self.spec.height) {
            self.spec.width = w;
            self.spec.height = h;
            self.surface.resize(w, h);
        }
        self.dirty = true;
        true
    }

    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        self.window.is_alive()
    }

    #[allow(dead_code)]
    pub fn hide(&self) {
        self.window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, x: i32) -> NativeWidgetSpec {
        NativeWidgetSpec {
            id: id.into(),
            scene_path: format!("C:/s/{id}.json"),
            x,
            y: 10,
            width: 300,
            height: 200,
            always_on_top: false,
            locked: true,
            click_through: false,
        }
    }

    #[test]
    fn active_widgets_persist_roundtrip_and_empty_removes() {
        let dir = std::env::temp_dir().join(format!("prismview_widgets_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_active_widgets(&dir).is_empty());
        let list = vec![spec("a", -1500), spec("b", 40)];
        save_active_widgets(&dir, &list).expect("save");
        assert_eq!(load_active_widgets(&dir), list);
        // Posição após arrasto sobrescreve.
        let moved = vec![spec("a", 7)];
        save_active_widgets(&dir, &moved).expect("save2");
        assert_eq!(load_active_widgets(&dir)[0].x, 7);
        save_active_widgets(&dir, &[]).expect("clear");
        assert!(!dir.join(ACTIVE_WIDGETS_FILE).exists());
        assert!(!dir.join(format!("{ACTIVE_WIDGETS_FILE}.tmp")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn demand_is_or_of_visible_widgets_plus_power() {
        let wall = SignalKind::AUDIO;
        let d = combined_demand(wall, [(true, SignalKind::MOUSE), (false, SignalKind::MEDIA)]);
        assert!(d.contains(SignalKind::AUDIO));
        assert!(d.contains(SignalKind::MOUSE));
        assert!(!d.contains(SignalKind::MEDIA), "widget oculto não acorda sensores");
        assert!(d.contains(SignalKind::POWER));
        let idle = combined_demand(SignalKind::NONE, std::iter::empty());
        assert_eq!(idle, SignalKind::POWER);
    }

    #[test]
    fn local_coordinates_inside_and_outside() {
        let b = (-1920, 100, 400, 300);
        assert_eq!(local_point((-1920, 100), b), Some([0.0, 0.0]));
        assert_eq!(local_point((-1521, 399), b), Some([399.0, 299.0]));
        assert_eq!(local_point((-1520, 200), b), None, "borda direita é exclusiva");
        assert_eq!(local_point((-1921, 200), b), None);
    }

    #[test]
    fn look_pointer_tracks_whole_screen() {
        let b = (1600, 800, 200, 200); // centro (1700, 900)
        let mon = Some((0, 0, 1920, 1080));
        assert_eq!(look_pointer((1700, 900), b, mon), [0.5, 0.5]);
        let left = look_pointer((0, 900), b, mon);
        assert!(left[0] < 0.1 && (left[1] - 0.5).abs() < 1e-6, "{left:?}");
        assert_eq!(look_pointer((-50_000, 50_000), b, mon), [0.0, 1.0]);
        // Sem monitor: alcance padrão.
        let p = look_pointer((1700 + 480, 900), b, None);
        assert!((p[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn monitor_for_widget_center() {
        let mons = [("A", (0, 0, 1920, 1080)), ("B", (-1920, 0, 1920, 1080))];
        assert_eq!(monitor_for((-500, 10, 100, 100), mons).map(|m| m.0), Some("B"));
        assert_eq!(monitor_for((100, 10, 100, 100), mons).map(|m| m.0), Some("A"));
        // Fora de tudo (abaixo de A): o mais próximo.
        assert_eq!(monitor_for((100, 5000, 100, 100), mons).map(|m| m.0), Some("A"));
    }

    #[test]
    fn formula_numbers() {
        assert_eq!(parse_formula_number(" 42 "), Some(42.0));
        assert_eq!(parse_formula_number("37%"), Some(37.0));
        assert_eq!(parse_formula_number("abc"), None);
        assert_eq!(parse_formula_number("NaN"), None);
    }
}
