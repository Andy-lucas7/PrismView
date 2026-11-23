//! Cena 2D (Fase 1): traduz `PrismScene` (contrato .prism v2) em quads do `Compositor2D`.
//!
//! - Layout: posição normalizada `x/y` dentro da caixa do pai (+ offsets em px) ancorada por
//!   `anchor`; rotação/escala em torno do centro da caixa; filhos herdam a transformação
//!   (afim composta) e a opacidade. `Stack` empilha filhos em fluxo (horizontal/vertical).
//! - Z-index: ordem de pré-ordem da árvore (pai < filhos, irmão posterior por cima); o `Canvas`
//!   ordena por `(z, ordem)`.
//! - FX: cada propriedade passa por `FxEngine`. A lista de desenho só é refeita quando
//!   `DirtyFlags` do FX (`TIME_MINUTE`, `TIME_SECOND`, ...) intersectam as dependências da cena,
//!   quando a cena/tamanho/DPI mudam, ou quando `mark_dirty` é chamado. Relógio parado no mesmo
//!   minuto = zero trabalho de CPU: só o passe de apresentação é regravado.
//! - Texto: `core::text` (glyphon) por slot; imagens: decodificadas 1× e enviadas pré-multiplicadas.
//!
//! Vidro óptico (Fase 2, `core::glass`): camadas `Shape` (Rect/Circle/Line) e `Text` com fill
//! `Adaptive`/`ClassicBlur`/`OpticalCrystal` viram marcadores Z no `Canvas` e são desenhadas com o
//! passe de vidro sobre o fundo (imagem ou frame de vídeo registrado com
//! `set_video_backdrop`). Sem fundo de mídia (cor sólida), caem no preenchimento com a cor de tint.
//!
//! Vídeo: fundo `BackgroundSource::Video` e camadas `Video` recebem frames zero-copy do decoder
//! (`set_video_frame`, um `TextureId` por slot do anel, sem bind group por quadro). `Svg`/`Icon`:
//! `svg_markup` rasterizado por resvg no tamanho da caixa (cache por markup+tamanho). Backdrop do
//! vidro = última mídia full-bleed desenhada abaixo dele (imagem/vídeo de fundo, vídeo ou cena 3D
//! full-bleed). Parallax por mouse = bindings `CURSOR_X/Y` (gerados pelo conversor do Estúdio).
//!
//! Ainda não renderizados: `DepthSubject`, carrossel de fundo, blend modes que leem o backdrop e
//! contorno/blur de sombra em texto.
//!
//! Regras: sem `unwrap/expect`; nenhum código de SO. Alocações só ao reconstruir a lista.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Local, Timelike};
use prism_core::fx::{EvalContext, FxEngine};
use prism_core::schema::{
    Anchor, BackgroundSource, BlendMode, DirtyFlags, FillMode, GradientKind as SchemaGradient,
    LayerKind, LayerNode, PrismScene, ShapeKind, StackDir, StaticOrFx, TextAlign, TouchAction,
    VisibilityCondition,
};

use super::bindings::{LayerBindingSolver, LayerFx};
use super::compositor::{
    BlendKind, BoundSubtree, Canvas, Compositor2D, GradientKind, QuadInstance, QuadKind,
    Scene3dPlacement, TextureId,
};
use super::interaction::{DesktopAction, HitRegion};
use super::signals::{SignalKind, SignalSnapshot};
use super::glass::{
    hash_combine, hash_str, GlassBackdrop, GlassDraw, GlassMode, GlassParams, GlassRenderer,
    GlyphSource,
};
use super::gpu::{FrameStatus, GpuContext, WindowSurface};
use super::text::{FontResolver, TextAlignment, TextLayout, TextSpec, TextSystem};
use crate::platform::GpuVideoFrame;

mod geometry;
#[allow(unused_imports)]
use geometry::*;

mod images;
#[allow(unused_imports)]
pub use images::*;

// ───────────────────────── SceneRenderer ─────────────────────────

/// Frames de um vídeo (fundo ou camada). Um `TextureId` por slot do anel do decoder, criado 1×
/// (a view fica no compositor: serve também de backdrop do vidro). Trocar o frame exibido só troca
/// o id usado na lista — nenhum bind group novo por quadro.
struct VideoBinding {
    slots: Vec<(usize, TextureId)>,
    generation: u64,
    current: Option<TextureId>,
    /// Id estável usado na lista de desenho; aponta para `current` (troca de frame = só
    /// `set_alias`, sem rebuild).
    alias: Option<TextureId>,
    size: (u32, u32),
    content_hash: u64,
}

/// Identidade do backdrop "saída 3D" no cache de blur do vidro.
const SCENE3D_BACKDROP_HASH: u64 = 0x5CE3_D0B4_CD70_0001;

mod svg;
#[allow(unused_imports)]
use svg::*;

/// Resolve um asset da cena: `custom_assets` → absoluto, ou relativo a `asset_root`.
fn resolve_asset_path(scene: &PrismScene, asset_root: Option<&Path>, raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mapped = scene.custom_assets.get(raw).map_or(raw, String::as_str);
    let path = Path::new(mapped);
    match asset_root {
        Some(root) if !path.is_absolute() => Some(root.join(path)),
        _ => Some(path.to_path_buf()),
    }
}

/// Saída do `Scene3dRenderer` registrada no compositor (view pertence ao renderer 3D).
#[derive(Clone, Copy)]
struct Scene3dOutput {
    id: TextureId,
    size: (u32, u32),
}

/// Recursos compartilhados por todas as saídas (monitores) de um `GpuContext`.
pub struct SceneRenderer {
    compositor: Compositor2D,
    glass: GlassRenderer,
    /// Vídeos da cena (fundo e camadas) por caminho resolvido.
    videos: HashMap<String, VideoBinding>,
    /// Muda a cada frame novo de fonte viva (vídeo, cena 3D): chave do blur do vidro.
    live_token: u64,
    /// Geração dos deltas de binding: muda a cada `apply_signals` com mudança. As listas
    /// recebem o delta pelo caminho rápido (sem rebuild) quando possível.
    fx_gen: u64,
    svgs: SvgCache,
    /// `TouchAction`s não-mídia da cena (índice = `DesktopAction::Custom`) e id da camada → índice.
    touch_table: Vec<TouchAction>,
    touch_index: HashMap<String, u32>,
    text: TextSystem,
    images: ImageCache,
    text_bindings: HashMap<String, TextBinding>,
    key_scratch: String,
    scene: Option<PrismScene>,
    deps: DirtyFlags,
    version: u64,
    clock_sec: i64,
    clock_min: i64,
    asset_root: Option<PathBuf>,
    /// Bindings reativos 2D/vidro (Fase 4): deltas somados à base FX em cada rebuild.
    layer_bindings: LayerBindingSolver,
    /// Textura final da cena 3D (camada `Scene3d`), se o chamador já renderizou uma.
    scene3d_out: Option<Scene3dOutput>,
}

impl SceneRenderer {
    /// `format` = formato da swapchain das saídas (`WindowSurface::format`).
    pub fn new(gpu: &GpuContext, format: wgpu::TextureFormat) -> Self {
        Self {
            compositor: Compositor2D::new(gpu.device(), gpu.queue(), format),
            glass: GlassRenderer::new(gpu.device(), format),
            videos: HashMap::new(),
            live_token: 0,
            fx_gen: 0,
            svgs: SvgCache::default(),
            touch_table: Vec::new(),
            touch_index: HashMap::new(),
            text: TextSystem::new(gpu.device(), gpu.queue()),
            images: ImageCache::default(),
            text_bindings: HashMap::new(),
            key_scratch: String::with_capacity(64),
            scene: None,
            deps: DirtyFlags::NONE,
            version: 0,
            clock_sec: i64::MIN,
            clock_min: i64::MIN,
            asset_root: None,
            layer_bindings: LayerBindingSolver::new(),
            scene3d_out: None,
        }
    }

    /// `Scene3DConfig` (JSON camelCase) da primeira camada 3D da cena, se houver.
    pub fn scene3d_config(&self) -> Option<&serde_json::Value> {
        fn find(layers: &[LayerNode]) -> Option<&serde_json::Value> {
            layers.iter().find_map(|l| {
                if l.kind == LayerKind::Scene3d && !l.hidden {
                    if let Some(cfg) = l.scene3d.as_ref() {
                        return Some(cfg);
                    }
                }
                find(&l.children)
            })
        }
        self.scene.as_ref().and_then(|s| find(&s.layers))
    }

    /// Registra a view de saída do `Scene3dRenderer`. Chamar só quando a view foi recriada
    /// (resize/hibernação): reaproveita o `TextureId` e refaz a lista se o tamanho mudou.
    /// A view fica no compositor: também serve de backdrop do vidro quando o 3D é full-bleed.
    pub fn set_scene3d_output(&mut self, gpu: &GpuContext, view: wgpu::TextureView, size: (u32, u32)) {
        let existing = self.scene3d_out.map(|o| o.id);
        let id = self.compositor.bind_owned_view(gpu.device(), existing, view);
        let changed = self.scene3d_out.map_or(true, |o| o.id != id || o.size != size);
        self.scene3d_out = Some(Scene3dOutput { id, size });
        if changed {
            self.version += 1;
        }
    }

    /// Novo quadro 3D na mesma textura: o blur do vidro que a usa como backdrop é refeito.
    pub fn scene3d_frame_rendered(&mut self) {
        self.live_token = self.live_token.wrapping_add(1);
    }

    /// Esquece a saída 3D (renderer 3D descartado ou hibernado).
    pub fn clear_scene3d_output(&mut self) {
        if let Some(out) = self.scene3d_out.take() {
            self.compositor.release_texture(out.id);
            self.version += 1;
        }
    }

    /// Caixa da camada 3D na última construção de `canvas`.
    pub fn scene3d_placement(canvas: &Canvas) -> Option<Scene3dPlacement> {
        canvas.scene3d
    }

    /// Avança os bindings 2D/vidro com o snapshot do quadro. `true` se a lista precisa ser refeita
    /// (algum delta mudou). Cena sem bindings = custo zero.
    pub fn apply_signals(&mut self, snap: &SignalSnapshot, dt: f32) -> bool {
        if self.scene.is_none() || !self.layer_bindings.solve(snap, dt) {
            return false;
        }
        // Não é mudança estrutural: o `render` reaplica o delta nas instâncias (caminho rápido)
        // e só refaz a lista se a subárvore não permitir.
        self.fx_gen = self.fx_gen.wrapping_add(1);
        true
    }

    /// Caminho rápido dos bindings: volta à base do rebuild e aplica, por subárvore com binding
    /// (filhos antes dos pais), `xf(fx1) ∘ xf(fx0)⁻¹`. Sem alocação. `false` = precisa de rebuild
    /// (opacidade/vidro mudou, subárvore com vidro/3D, camada pulada, estado de animação virou).
    fn apply_bindings_fast(canvas: &mut Canvas, solver: &LayerBindingSolver, queue: &wgpu::Queue) -> bool {
        if canvas.bound_skipped {
            return false;
        }
        canvas.reset_to_base();
        for i in 0..canvas.bound.len() {
            let (seq, hits, lin, t) = {
                let b = &canvas.bound[i];
                let fx1 = solver.fx(&b.key).unwrap_or_default();
                if solver.is_layer_animating(&b.key) != b.moving0 {
                    return false;
                }
                if fx1 == b.fx0 {
                    continue;
                }
                if b.rigid
                    || (fx1.opacity - b.fx0.opacity).abs() > 1e-4
                    || fx1.glass != b.fx0.glass
                {
                    return false;
                }
                let parent = Affine {
                    m: b.parent_m,
                    t: b.parent_t,
                };
                let xf = |fx: &LayerFx| {
                    parent.child(
                        [
                            b.center_base[0] + fx.offset[0] * b.ui,
                            b.center_base[1] + fx.offset[1] * b.ui,
                        ],
                        b.rot_base + fx.rotation_deg.to_radians(),
                        b.scale_base * (1.0 + fx.scale),
                    )
                };
                let Some(inv0) = xf(&b.fx0).inverse() else {
                    return false;
                };
                let delta = xf(&fx1).then_apply(&inv0);
                (b.seq.clone(), b.hits.clone(), delta.m, delta.t)
            };
            if !canvas.transform_range(&seq, &hits, lin, t) {
                return false;
            }
        }
        canvas.upload_instances(queue);
        true
    }

    /// Sinais que os bindings 2D consomem (+ `DESKTOP_TAP` se há controles de mídia tocáveis).
    pub fn signal_demand(&self) -> SignalKind {
        let mut demand = self.layer_bindings.demand();
        if self.has_touch_actions() {
            demand |= SignalKind::DESKTOP_TAP;
        }
        // Fórmulas `$mi()$`/`$sys()$`/`$bi()$`: produtores de mídia/telemetria precisam rodar.
        if self.deps.intersects(DirtyFlags::MEDIA) {
            demand |= SignalKind::MEDIA;
        }
        if self.deps.intersects(DirtyFlags::SYSTEM_METRICS) {
            demand |= SignalKind::TELEMETRY | SignalKind::POWER;
        }
        demand
    }

    /// Molas 2D ainda em movimento (o loop deve continuar desenhando).
    pub fn bindings_animating(&self) -> bool {
        self.layer_bindings.is_animating()
    }

    fn has_touch_actions(&self) -> bool {
        fn any(layers: &[LayerNode]) -> bool {
            layers
                .iter()
                .any(|l| l.touch.is_active() || any(&l.children))
        }
        self.scene.as_ref().is_some_and(|s| any(&s.layers))
    }

    /// Ação tocável em `p` (px físicos locais da saída), pela lista do último rebuild de `canvas`.
    pub fn hit_test(canvas: &Canvas, p: [f32; 2]) -> Option<DesktopAction> {
        canvas.hits.hit_test(p)
    }

    pub fn create_canvas(&self, gpu: &GpuContext) -> Canvas {
        let mut canvas = self.compositor.create_canvas(gpu.device());
        canvas.glass = Some(self.glass.create_layer(gpu.device()));
        canvas
    }

    /// Vídeos que a cena usa: `(caminho resolvido, é_fundo)`. Raro (troca de cena).
    pub fn video_sources(&self) -> Vec<(PathBuf, bool)> {
        fn walk(layers: &[LayerNode], scene: &PrismScene, root: Option<&Path>, out: &mut Vec<(PathBuf, bool)>) {
            for l in layers {
                if l.kind == LayerKind::Video && !l.hidden {
                    let raw = match &l.asset {
                        StaticOrFx::Static(v) => v.as_str(),
                        StaticOrFx::Fx { fallback, .. } => fallback.as_str(),
                    };
                    if let Some(p) = resolve_asset_path(scene, root, raw) {
                        if !out.iter().any(|(q, _)| *q == p) {
                            out.push((p, false));
                        }
                    }
                }
                walk(&l.children, scene, root, out);
            }
        }
        let mut out = Vec::new();
        let Some(scene) = self.scene.as_ref() else {
            return out;
        };
        let root = self.asset_root.as_deref();
        if let BackgroundSource::Video { path, .. } = &scene.background {
            if let Some(p) = resolve_asset_path(scene, root, path) {
                out.push((p, true));
            }
        }
        walk(&scene.layers, scene, root, &mut out);
        out
    }

    /// Frame atual do decoder de `path`. Cria o `TextureId` do slot na 1ª vez que ele aparece
    /// (5 por vídeo no anel do MF); depois só alterna o id exibido. `true` = redesenhar.
    pub fn set_video_frame(&mut self, gpu: &GpuContext, path: &Path, frame: &GpuVideoFrame<'_>) -> bool {
        let key = path.to_string_lossy();
        if !self.videos.contains_key(&*key) {
            self.videos.insert(
                key.to_string(),
                VideoBinding {
                    slots: Vec::with_capacity(5),
                    generation: frame.generation,
                    current: None,
                    alias: None,
                    size: (frame.width, frame.height),
                    content_hash: hash_str(&key),
                },
            );
        }
        let Some(entry) = self.videos.get_mut(&*key) else {
            return false;
        };
        if entry.generation != frame.generation {
            for (_, id) in entry.slots.drain(..) {
                self.compositor.release_texture(id);
            }
            if let Some(alias) = entry.alias.take() {
                self.compositor.release_texture(alias);
                self.version += 1;
            }
            entry.current = None;
            entry.generation = frame.generation;
        }
        let id = match entry.slots.iter().find(|(slot, _)| *slot == frame.slot) {
            Some(&(_, id)) => id,
            None => {
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let id = self.compositor.bind_owned_view(gpu.device(), None, view);
                entry.slots.push((frame.slot, id));
                id
            }
        };
        let size = (frame.width, frame.height);
        if entry.current != Some(id) {
            entry.current = Some(id);
            match entry.alias {
                // Frame novo: só redireciona o alias (lista de desenho intacta).
                Some(alias) => self.compositor.set_alias(alias, id),
                // 1º frame: a camada passa a existir na lista → rebuild estrutural.
                None => {
                    entry.alias = Some(self.compositor.create_alias(id));
                    self.version += 1;
                }
            }
        }
        if entry.size != size {
            entry.size = size; // UV de cover depende do tamanho → rebuild
            self.version += 1;
        }
        self.live_token = self.live_token.wrapping_add(1);
        true
    }

    /// Solta as texturas de todos os vídeos (troca de cena / decoders fechados).
    pub fn clear_videos(&mut self) {
        if self.videos.is_empty() {
            return;
        }
        for (_, v) in self.videos.drain() {
            for (_, id) in v.slots {
                self.compositor.release_texture(id);
            }
            if let Some(alias) = v.alias {
                self.compositor.release_texture(alias);
            }
            self.glass.invalidate_backdrop(v.content_hash);
        }
        self.version += 1;
    }

    /// Ação de toque não-mídia (`DesktopAction::Custom`).
    pub fn touch_action(&self, index: u32) -> Option<&TouchAction> {
        self.touch_table.get(index as usize)
    }

    /// Recalcula as dependências FX (ex.: mídia começou a tocar → `$mi(pos)$` passa a depender
    /// do segundo). Raro: avalia todas as propriedades uma vez.
    pub fn refresh_deps(&mut self, ctx: &EvalContext) {
        if let Some(scene) = self.scene.as_ref() {
            self.deps = FxEngine::collect_scene_dependencies(scene, ctx);
        }
    }

    #[allow(dead_code)] // API reservada (ainda sem chamador)
    pub fn glass_stats(&self) -> super::glass::GlassStats {
        self.glass.stats()
    }

    /// Hibernação de VRAM (Fase 5): solta pirâmide de blur e mapas de glifo do vidro.
    pub fn release_glass_gpu(&mut self) {
        self.glass.release();
    }

    /// PowerGovernor: liga/desliga pós-processamento (blur do vidro). `true` se o quadro precisa
    /// ser refeito.
    pub fn set_post_processing(&mut self, enabled: bool) -> bool {
        self.glass.set_blur_enabled(enabled)
    }

    /// Pasta base para resolver caminhos relativos de assets (ex.: pacote `.prism` extraído).
    pub fn set_asset_root(&mut self, root: Option<PathBuf>) {
        self.asset_root = root;
        self.version += 1;
    }

    pub fn set_font_resolver(&mut self, resolver: FontResolver) {
        self.text.set_font_resolver(resolver);
        self.version += 1;
    }

    pub fn register_font_dir(&mut self, dir: &Path) {
        self.text.register_font_dir(dir);
        self.version += 1;
    }

    pub fn register_font_file(&mut self, path: &Path) -> bool {
        let ok = self.text.register_font_file(path);
        if ok {
            self.version += 1;
        }
        ok
    }

    pub fn register_font_data(&mut self, data: Vec<u8>) {
        self.text.register_font_data(data);
        self.version += 1;
    }

    pub fn has_scene(&self) -> bool {
        self.scene.is_some()
    }

    /// Imagens que não decodificaram/enviaram (para o status do Studio).
    pub fn failed_images(&self) -> impl Iterator<Item = &str> {
        self.images.failed.iter().map(String::as_str)
    }

    /// Imagens decodificadas em segundo plano ficaram prontas: invalida a lista (o rebuild faz o
    /// upload). Chamar 1× por iteração do loop; `true` = redesenhar.
    pub fn poll_images(&mut self) -> bool {
        if self.images.take_completed() {
            self.version += 1;
            true
        } else {
            false
        }
    }

    fn reset_resources(&mut self) {
        self.images.release_all(&mut self.compositor);
        self.svgs.release_all(&mut self.compositor);
        for (_, binding) in self.text_bindings.drain() {
            self.compositor.release_texture(binding.id);
        }
        self.text.clear();
        self.glass.release();
    }

    pub fn set_scene(&mut self, scene: PrismScene, ctx: &EvalContext) {
        self.reset_resources();
        self.deps = FxEngine::collect_scene_dependencies(&scene, ctx);
        self.layer_bindings.set_specs(&scene.bindings);
        self.index_scene(&scene);
        self.scene = Some(scene);
        self.clock_sec = ctx.now.timestamp();
        self.clock_min = self.clock_sec.div_euclid(60);
        self.version += 1;
    }

    /// Tabela de toques não-mídia + fontes de arquivo do projeto (alias `font_id` → família).
    fn index_scene(&mut self, scene: &PrismScene) {
        fn walk(
            layers: &[LayerNode],
            scene: &PrismScene,
            asset_root: Option<&Path>,
            table: &mut Vec<TouchAction>,
            index: &mut HashMap<String, u32>,
            fonts: &mut Vec<(String, PathBuf)>,
        ) {
            for l in layers {
                if l.touch.is_active() && DesktopAction::from_touch(l.touch.action_type).is_none() {
                    index.insert(l.id.clone(), u32::try_from(table.len()).unwrap_or(u32::MAX));
                    table.push(l.touch.clone());
                }
                if l.kind == LayerKind::Text && !l.text.font_path.trim().is_empty() {
                    if let Some(resolved) =
                        resolve_asset_path(scene, asset_root, l.text.font_path.trim())
                    {
                        fonts.push((l.text.font_id.clone(), resolved));
                    }
                }
                walk(&l.children, scene, asset_root, table, index, fonts);
            }
        }
        self.touch_table.clear();
        self.touch_index.clear();
        let mut fonts = Vec::new();
        walk(
            &scene.layers,
            scene,
            self.asset_root.as_deref(),
            &mut self.touch_table,
            &mut self.touch_index,
            &mut fonts,
        );
        for (font_id, path) in fonts {
            if path.is_file() && !self.text.register_font_alias(&font_id, &path) {
                eprintln!("[prismview-engine] fonte ilegível: {}", path.display());
            }
        }
    }

    pub fn clear_scene(&mut self) {
        self.reset_resources();
        self.clear_videos();
        self.touch_table.clear();
        self.touch_index.clear();
        self.scene = None;
        self.layer_bindings.clear();
        self.deps = DirtyFlags::NONE;
        self.version += 1;
    }

    /// Sinaliza mudança de uma fonte de dados do FX. `true` se a lista precisa ser refeita.
    pub fn mark_dirty(&mut self, flags: DirtyFlags) -> bool {
        if self.scene.is_none() {
            return false;
        }
        let geometry = DirtyFlags::GEOMETRY.union(DirtyFlags::BACKGROUND);
        if flags.intersects(geometry) || self.deps.intersects(flags) {
            self.version += 1;
            true
        } else {
            false
        }
    }

    /// Atualiza o relógio interno e invalida a cena só se `$df(..)$`/`$time$` mudou de fato.
    pub fn advance_clock(&mut self, now: &DateTime<Local>) -> bool {
        let sec = now.timestamp();
        let min = sec.div_euclid(60);
        let mut changed = DirtyFlags::NONE;
        if sec != self.clock_sec {
            changed.insert(DirtyFlags::TIME_SECOND);
        }
        if min != self.clock_min {
            changed.insert(DirtyFlags::TIME_MINUTE);
        }
        self.clock_sec = sec;
        self.clock_min = min;
        if changed.is_empty() {
            false
        } else {
            self.mark_dirty(changed)
        }
    }

    /// Quanto falta até o próximo instante em que o relógio invalida a cena (`None` = cena
    /// estática em relação ao tempo: o loop pode dormir indefinidamente).
    pub fn next_wake(&self, now: &DateTime<Local>) -> Option<Duration> {
        self.scene.as_ref()?;
        let ms = u64::from(now.timestamp_subsec_millis().min(999));
        if self.deps.intersects(DirtyFlags::TIME_SECOND) {
            return Some(Duration::from_millis(1000 - ms + 2));
        }
        if self.deps.intersects(DirtyFlags::TIME_MINUTE) {
            let sec = u64::from(now.second().min(59));
            return Some(Duration::from_millis((60 - sec) * 1000 - ms + 2));
        }
        None
    }

    /// Desenha a cena em `surface`. A lista do `canvas` só é refeita se a cena, o tamanho ou o
    /// DPI mudaram; caso contrário apenas regrava o passe. `canvas_key` identifica a saída
    /// (id do monitor) para isolar os slots de texto de cada DPI.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        gpu: &GpuContext,
        surface: &mut WindowSurface,
        canvas: &mut Canvas,
        canvas_key: &str,
        ui_scale: f32,
        ctx: &EvalContext,
        clear: [f32; 4],
    ) -> FrameStatus {
        let (w, h) = surface.size();
        let ui_scale = if ui_scale.is_finite() && ui_scale > 0.0 {
            ui_scale
        } else {
            1.0
        };
        let mut stale = canvas.built_version != self.version
            || canvas.built_size != (w, h)
            || (canvas.built_scale - ui_scale).abs() > 1e-4;
        if !stale && canvas.built_fx_gen != self.fx_gen {
            // Só os deltas de binding mudaram (parallax, molas): sem layout nem reordenação.
            stale = !Self::apply_bindings_fast(canvas, &self.layer_bindings, gpu.queue());
            canvas.built_fx_gen = self.fx_gen;
        }
        if stale {
            self.rebuild(gpu, canvas, canvas_key, (w, h), ui_scale, ctx);
            canvas.built_fx_gen = self.fx_gen;
            canvas.built_version = self.version;
            canvas.built_size = (w, h);
            canvas.built_scale = ui_scale;
            self.text.flush(gpu.device(), gpu.queue());
            canvas.flush(&self.compositor, gpu.device(), gpu.queue());
            if let Some(layer) = canvas.glass.as_mut() {
                self.glass.flush(layer, gpu.device(), gpu.queue());
            }
        }

        // Campos disjuntos: o vidro é mutado em `prepare`, o resto só é lido.
        let compositor = &self.compositor;
        let text = &self.text;
        let glass = &mut self.glass;
        let frame_token = self.live_token;

        surface.render_with(gpu, |encoder, view| {
            // 1) Blur em cache + mapas de glifo que faltam (fora de qualquer passe de render).
            if let Some(layer) = canvas.glass.as_mut() {
                if !layer.is_empty() {
                    glass.prepare(
                        gpu.device(),
                        gpu.queue(),
                        encoder,
                        layer,
                        frame_token,
                        |id| {
                            let version = compositor.texture_version(id);
                            compositor.texture_view(id).map(|v| (v, version))
                        },
                        |key| text.texture_view(key),
                    );
                }
            }
            // 2) Passe do compositor; cada marcador Z desenha um vidro na posição certa.
            let canvas = &*canvas;
            let glass = &*glass;
            compositor.render_with(encoder, view, clear, canvas, |pass, marker| {
                if let Some(layer) = canvas.glass.as_ref() {
                    glass.draw(layer, pass, marker);
                }
            });
        })
    }

    fn rebuild(
        &mut self,
        gpu: &GpuContext,
        canvas: &mut Canvas,
        canvas_key: &str,
        size: (u32, u32),
        ui: f32,
        ctx: &EvalContext,
    ) {
        canvas.begin(size.0, size.1);
        if let Some(layer) = canvas.glass.as_mut() {
            layer.begin(size.0, size.1);
        }
        self.glass.begin_build();
        let serial = self.text.begin_pass();

        if let Some(scene) = self.scene.as_ref() {
            let mut frame = Frame {
                videos: &self.videos,
                svgs: &mut self.svgs,
                touch_index: &self.touch_index,
                scene3d_out: self.scene3d_out,
                backdrop: None,
                scene,
                ctx,
                device: gpu.device(),
                queue: gpu.queue(),
                compositor: &mut self.compositor,
                text: &mut self.text,
                images: &mut self.images,
                bindings: &mut self.text_bindings,
                canvas,
                canvas_key,
                scratch: &mut self.key_scratch,
                asset_root: self.asset_root.as_deref(),
                fx: &self.layer_bindings,
                ui,
                vw: size.0 as f32,
                vh: size.1 as f32,
                z: 0,
            };
            frame.build();
        }

        // Libera slots/bindings de texto desta saída que a cena já não usa (sem alocar).
        self.key_scratch.clear();
        self.key_scratch.push('@');
        self.key_scratch.push_str(canvas_key);
        let suffix = self.key_scratch.as_str();
        self.text.sweep(serial, |key| key.ends_with(suffix));
        let text = &self.text;
        let compositor = &mut self.compositor;
        self.text_bindings.retain(|key, binding| {
            if !key.ends_with(suffix) || text.has_slot(key) {
                true
            } else {
                compositor.release_texture(binding.id);
                false
            }
        });
    }
}

mod frame;
#[allow(unused_imports)]
use frame::*;
