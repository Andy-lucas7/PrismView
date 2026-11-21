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
//! Ainda não renderizados: `Svg`/`Icon` (tesselação), `DepthSubject`, carrossel de fundo,
//! parallax por mouse (Fase 4), blend modes que leem o backdrop e contorno/blur de sombra em texto.
//!
//! Regras: sem `unwrap/expect`; nenhum código de SO. Alocações só ao reconstruir a lista.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local, Timelike};
use prism_core::fx::{EvalContext, FxEngine};
use prism_core::schema::{
    Anchor, BackgroundSource, BlendMode, DirtyFlags, FillMode, GradientKind as SchemaGradient,
    LayerKind, LayerNode, PrismScene, ShapeKind, StackDir, StaticOrFx, TextAlign,
    VisibilityCondition,
};

use super::compositor::{
    BlendKind, Canvas, Compositor2D, GradientKind, QuadInstance, QuadKind, TextureId,
};
use super::glass::{
    hash_combine, hash_str, GlassBackdrop, GlassDraw, GlassMode, GlassParams, GlassRenderer,
    GlyphSource,
};
use super::gpu::{FrameStatus, GpuContext, WindowSurface};
use super::text::{TextAlignment, TextLayout, TextSpec, TextSystem};

// ───────────────────────── Geometria ─────────────────────────

/// Transformação afim 2D: `m` = [m00, m10, m01, m11] (colunas X e Y), `t` = translação.
#[derive(Clone, Copy, Debug)]
struct Affine {
    m: [f32; 4],
    t: [f32; 2],
}

impl Affine {
    const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 1.0],
        t: [0.0, 0.0],
    };

    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.m[0] * p[0] + self.m[2] * p[1] + self.t[0],
            self.m[1] * p[0] + self.m[3] * p[1] + self.t[1],
        ]
    }

    /// `self ∘ T(center) ∘ R(rot) ∘ S(scale)`: nó filho posicionado em `center` (espaço do pai).
    fn child(&self, center: [f32; 2], rot: f32, scale: f32) -> Self {
        let (sn, cs) = rot.sin_cos();
        let (c0, c1) = (cs * scale, sn * scale); // coluna X de R·S
        let (c2, c3) = (-sn * scale, cs * scale); // coluna Y de R·S
        let m = &self.m;
        Self {
            m: [
                m[0] * c0 + m[2] * c1,
                m[1] * c0 + m[3] * c1,
                m[0] * c2 + m[2] * c3,
                m[1] * c2 + m[3] * c3,
            ],
            t: self.apply(center),
        }
    }

    /// Fator de escala uniforme (norma da coluna X).
    fn scale(&self) -> f32 {
        self.m[0].hypot(self.m[1])
    }

    fn is_identity_linear(&self) -> bool {
        (self.m[0] - 1.0).abs() < 1e-4
            && self.m[1].abs() < 1e-4
            && self.m[2].abs() < 1e-4
            && (self.m[3] - 1.0).abs() < 1e-4
    }
}

#[derive(Clone, Copy, Debug)]
struct Size {
    w: f32,
    h: f32,
}

/// Caixa do pai no espaço local do pai (`left/top` relativos ao centro do pai, ou 0,0 na raiz).
#[derive(Clone, Copy, Debug)]
struct ParentBox {
    left: f32,
    top: f32,
    w: f32,
    h: f32,
}

fn anchor_factors(anchor: Anchor) -> (f32, f32) {
    match anchor {
        Anchor::TopLeft => (0.0, 0.0),
        Anchor::TopCenter => (0.5, 0.0),
        Anchor::TopRight => (1.0, 0.0),
        Anchor::CenterLeft => (0.0, 0.5),
        Anchor::Center => (0.5, 0.5),
        Anchor::CenterRight => (1.0, 0.5),
        Anchor::BottomLeft => (0.0, 1.0),
        Anchor::BottomCenter => (0.5, 1.0),
        Anchor::BottomRight => (1.0, 1.0),
    }
}

fn map_blend(mode: BlendMode) -> BlendKind {
    match mode {
        BlendMode::Plus => BlendKind::Plus,
        BlendMode::Multiply => BlendKind::Multiply,
        BlendMode::Screen => BlendKind::Screen,
        _ => BlendKind::Normal,
    }
}

fn with_alpha(mut rgba: [f32; 4], opacity: f32) -> [f32; 4] {
    rgba[3] *= opacity;
    rgba
}

/// `#RRGGBB` / `#AARRGGBB`; texto inválido vira branco opaco.
fn parse_color(raw: &str) -> [f32; 4] {
    FxEngine::parse_rgba_hex(raw).unwrap_or([1.0, 1.0, 1.0, 1.0])
}

/// UV que preenche a caixa (`box_w`×`box_h`) com a imagem sem distorcer (recorte central).
fn cover_uv(img_w: u32, img_h: u32, box_w: f32, box_h: f32) -> [f32; 4] {
    let (iw, ih) = (img_w.max(1) as f32, img_h.max(1) as f32);
    if box_w <= 0.0 || box_h <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let image_aspect = iw / ih;
    let box_aspect = box_w / box_h;
    if image_aspect > box_aspect {
        let f = box_aspect / image_aspect;
        let m = (1.0 - f) * 0.5;
        [m, 0.0, 1.0 - m, 1.0]
    } else {
        let f = image_aspect / box_aspect;
        let m = (1.0 - f) * 0.5;
        [0.0, m, 1.0, 1.0 - m]
    }
}

// ───────────────────────── Imagens ─────────────────────────

#[derive(Clone, Copy)]
struct ImageEntry {
    id: TextureId,
    w: u32,
    h: u32,
}

#[derive(Default)]
struct ImageCache {
    loaded: HashMap<String, ImageEntry>,
    failed: HashSet<String>,
}

impl ImageCache {
    fn get(
        &mut self,
        path: &Path,
        compositor: &mut Compositor2D,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<ImageEntry> {
        let key = path.to_string_lossy();
        if let Some(entry) = self.loaded.get(&*key) {
            return Some(*entry);
        }
        if self.failed.contains(&*key) {
            return None;
        }
        let key = key.into_owned();
        match Self::load(path, compositor, device, queue) {
            Some(entry) => {
                self.loaded.insert(key, entry);
                Some(entry)
            }
            None => {
                self.failed.insert(key);
                None
            }
        }
    }

    /// Decodifica (CPU, uma única vez), limita ao máximo do device, pré-multiplica e envia.
    fn load(
        path: &Path,
        compositor: &mut Compositor2D,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<ImageEntry> {
        let mut img = image::open(path).ok()?.into_rgba8();
        let (w, h) = img.dimensions();
        let max = compositor.max_texture_dim();
        if w > max || h > max {
            let k = max as f32 / w.max(h) as f32;
            let nw = ((w as f32 * k) as u32).max(1);
            let nh = ((h as f32 * k) as u32).max(1);
            img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
        }
        let (w, h) = img.dimensions();
        for px in img.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            if a != 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        let id = compositor.create_texture_rgba(device, queue, w, h, img.as_raw())?;
        Some(ImageEntry { id, w, h })
    }

    fn release_all(&mut self, compositor: &mut Compositor2D) {
        for (_, entry) in self.loaded.drain() {
            compositor.release_texture(entry.id);
        }
        self.failed.clear();
    }
}

#[derive(Clone, Copy)]
struct TextBinding {
    id: TextureId,
    gen: u64,
}

// ───────────────────────── SceneRenderer ─────────────────────────

/// Frame de vídeo usado como fundo (e backdrop do vidro). A view pertence ao chamador (decoder).
#[derive(Clone, Copy)]
struct VideoState {
    id: TextureId,
    size: (u32, u32),
    content_hash: u64,
    frame_token: u64,
}

/// Recursos compartilhados por todas as saídas (monitores) de um `GpuContext`.
pub struct SceneRenderer {
    compositor: Compositor2D,
    glass: GlassRenderer,
    video: Option<VideoState>,
    video_view: Option<wgpu::TextureView>,
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
}

impl SceneRenderer {
    /// `format` = formato da swapchain das saídas (`WindowSurface::format`).
    pub fn new(gpu: &GpuContext, format: wgpu::TextureFormat) -> Self {
        Self {
            compositor: Compositor2D::new(gpu.device(), gpu.queue(), format),
            glass: GlassRenderer::new(gpu.device(), format),
            video: None,
            video_view: None,
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
        }
    }

    pub fn create_canvas(&self, gpu: &GpuContext) -> Canvas {
        let mut canvas = self.compositor.create_canvas(gpu.device());
        canvas.glass = Some(self.glass.create_layer(gpu.device()));
        canvas
    }

    /// Registra o frame de vídeo atual como fundo (substitui imagem de fundo) e backdrop do
    /// vidro. Chamar quando o **slot/view** do frame muda (cria 1 bind group); para frames novos na
    /// mesma view use `advance_video_frame`. `content_hash` identifica o vídeo.
    pub fn set_video_backdrop(
        &mut self,
        gpu: &GpuContext,
        view: wgpu::TextureView,
        size: (u32, u32),
        content_hash: u64,
        frame_token: u64,
    ) {
        let existing = self.video.map(|v| v.id);
        let id = self.compositor.bind_view(gpu.device(), existing, &view);
        let layout_changed = self
            .video
            .map_or(true, |v| v.size != size || v.id != id || v.content_hash != content_hash);
        self.video_view = Some(view);
        self.video = Some(VideoState {
            id,
            size,
            content_hash,
            frame_token,
        });
        if layout_changed {
            self.version += 1; // refaz a lista (UV de cover / backdrop do vidro)
        }
    }

    /// Novo frame de vídeo na mesma view. `true` se é preciso redesenhar.
    pub fn advance_video_frame(&mut self, frame_token: u64) -> bool {
        match self.video.as_mut() {
            Some(v) if v.frame_token != frame_token => {
                v.frame_token = frame_token;
                true
            }
            _ => false,
        }
    }

    pub fn clear_video_backdrop(&mut self) {
        if let Some(v) = self.video.take() {
            self.compositor.release_texture(v.id);
            self.glass.invalidate_backdrop(v.content_hash);
            self.version += 1;
        }
        self.video_view = None;
    }

    pub fn glass_stats(&self) -> super::glass::GlassStats {
        self.glass.stats()
    }

    /// Hibernação de VRAM (Fase 5): solta pirâmide de blur e mapas de glifo do vidro.
    pub fn release_glass_gpu(&mut self) {
        self.glass.release();
    }

    /// Pasta base para resolver caminhos relativos de assets (ex.: pacote `.prism` extraído).
    pub fn set_asset_root(&mut self, root: Option<PathBuf>) {
        self.asset_root = root;
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

    fn reset_resources(&mut self) {
        self.images.release_all(&mut self.compositor);
        for (_, binding) in self.text_bindings.drain() {
            self.compositor.release_texture(binding.id);
        }
        self.text.clear();
    }

    pub fn set_scene(&mut self, scene: PrismScene, ctx: &EvalContext) {
        self.reset_resources();
        self.deps = FxEngine::collect_scene_dependencies(&scene, ctx);
        self.scene = Some(scene);
        self.clock_sec = ctx.now.timestamp();
        self.clock_min = self.clock_sec.div_euclid(60);
        self.version += 1;
    }

    pub fn clear_scene(&mut self) {
        self.reset_resources();
        self.scene = None;
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
        let stale = canvas.built_version != self.version
            || canvas.built_size != (w, h)
            || (canvas.built_scale - ui_scale).abs() > 1e-4;
        if stale {
            self.rebuild(gpu, canvas, canvas_key, (w, h), ui_scale, ctx);
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
        let video_id = self.video.map(|v| v.id);
        let video_view = self.video_view.as_ref();
        let frame_token = self.video.map_or(0, |v| v.frame_token);

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
                            if Some(id) == video_id {
                                video_view.map(|v| (v, version))
                            } else {
                                compositor.texture_view(id).map(|v| (v, version))
                            }
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
        let video = self.video;

        if let Some(scene) = self.scene.as_ref() {
            let mut frame = Frame {
                video,
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
                ui,
                vw: size.0 as f32,
                vh: size.1 as f32,
                z: 0,
            };
            frame.build();
        }

        // Libera slots/bindings de texto desta saída que a cena já não usa.
        let mut suffix = String::with_capacity(canvas_key.len() + 1);
        suffix.push('@');
        suffix.push_str(canvas_key);
        self.text.sweep(serial, |key| key.ends_with(&suffix));
        let text = &self.text;
        let compositor = &mut self.compositor;
        self.text_bindings.retain(|key, binding| {
            if !key.ends_with(&suffix) || text.has_slot(key) {
                true
            } else {
                compositor.release_texture(binding.id);
                false
            }
        });
    }
}

// ───────────────────────── Construção do frame ─────────────────────────

#[derive(Clone, Copy)]
struct TextDraw {
    layout: TextLayout,
    tex: Option<TextureId>,
}

struct FillStyle {
    color: [f32; 4],
    gradient: Option<(GradientKind, [f32; 4], [f32; 2])>,
    hollow: bool,
}

struct Frame<'a> {
    video: Option<VideoState>,
    /// Backdrop do vidro (mídia de fundo desenhada em `background`), se houver.
    backdrop: Option<GlassBackdrop>,
    scene: &'a PrismScene,
    ctx: &'a EvalContext,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    compositor: &'a mut Compositor2D,
    text: &'a mut TextSystem,
    images: &'a mut ImageCache,
    bindings: &'a mut HashMap<String, TextBinding>,
    canvas: &'a mut Canvas,
    canvas_key: &'a str,
    scratch: &'a mut String,
    asset_root: Option<&'a Path>,
    ui: f32,
    vw: f32,
    vh: f32,
    z: u32,
}

impl Frame<'_> {
    fn build(&mut self) {
        self.background();
        let root = ParentBox {
            left: 0.0,
            top: 0.0,
            w: self.vw,
            h: self.vh,
        };
        let scene = self.scene;
        for layer in &scene.layers {
            self.layer(layer, &root, Affine::IDENTITY, 1.0, None);
        }
    }

    // ── Avaliação FX ──

    fn f(&self, prop: &StaticOrFx<f32>) -> f32 {
        let v = FxEngine::eval_f32(prop, self.ctx).value;
        if v.is_finite() {
            v
        } else {
            0.0
        }
    }

    fn color(&self, prop: &StaticOrFx<String>) -> [f32; 4] {
        parse_color(&FxEngine::eval_string(prop, self.ctx).value)
    }

    fn next_z(&mut self) -> u32 {
        self.z = self.z.saturating_add(1);
        self.z
    }

    fn is_visible(&self, layer: &LayerNode) -> bool {
        if layer.hidden || !FxEngine::eval_bool(&layer.visible, self.ctx).value {
            return false;
        }
        let ctx = self.ctx;
        match layer.visibility_cond {
            VisibilityCondition::Always | VisibilityCondition::DesktopOnly => true,
            VisibilityCondition::Charging => ctx.system.battery_charging,
            VisibilityCondition::NotCharging => !ctx.system.battery_charging,
            VisibilityCondition::LowBattery => ctx.system.battery_percent <= 20,
            VisibilityCondition::Day => ctx.weather.is_day,
            VisibilityCondition::Night => !ctx.weather.is_day,
            VisibilityCondition::MediaPlaying => ctx.media.playing,
        }
    }

    fn resolve_asset(&self, raw: &str) -> Option<PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let mapped = self.scene.custom_assets.get(raw).map_or(raw, String::as_str);
        let path = Path::new(mapped);
        match self.asset_root {
            Some(root) if !path.is_absolute() => Some(root.join(path)),
            _ => Some(path.to_path_buf()),
        }
    }

    // ── Vidro óptico ──

    /// Backdrop cover sobre a viewport (mesmo mapeamento do quad de fundo).
    fn backdrop_for(
        &self,
        texture: TextureId,
        size: (u32, u32),
        content_hash: u64,
        live: bool,
    ) -> GlassBackdrop {
        let (iw, ih) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let k = (self.vw / iw).max(self.vh / ih).max(1e-3);
        GlassBackdrop {
            texture,
            size,
            uv_rect: cover_uv(size.0, size.1, self.vw, self.vh),
            src_per_screen_px: 1.0 / k,
            content_hash,
            live,
        }
    }

    fn is_glass(layer: &LayerNode) -> bool {
        matches!(
            layer.fill,
            FillMode::Adaptive | FillMode::ClassicBlur | FillMode::OpticalCrystal
        )
    }

    /// Mapeia `OpticalCrystalParams` para os parâmetros do shader (px físicos). Hipóteses de
    /// mapeamento onde o schema não é 1:1 com o TS: dispersão `×400` → `chromaticAberration`
    /// (0,006 → 2,4 px), `fresnel_intensity` → especular, `prismatic_rim_intensity` → borda,
    /// `grain` → ruído; `Adaptive`/`ClassicBlur` = só blur + véu (sem lente/dispersão/especular).
    fn glass_params(&self, layer: &LayerNode, opacity: f32, radius: f32) -> GlassParams {
        let c = &layer.crystal;
        let ui = self.ui;
        let tint = self.color(&c.tint_rgba);
        let mut p = GlassParams {
            blur_px: (self.f(&c.blur_px) * ui).max(0.0),
            frost: self.f(&c.frost_amount).clamp(0.0, 0.94),
            tint: [tint[0], tint[1], tint[2]],
            noise: c.grain.max(0.0),
            saturation: self.f(&c.saturation).max(0.0),
            opacity,
            radius_px: radius,
            bevel_px: (self.f(&c.bevel_width) * ui).max(0.0),
            light_angle_deg: self.f(&c.light_angle_deg),
            ..GlassParams::default()
        };
        if layer.fill == FillMode::OpticalCrystal {
            p.refraction_px = self.f(&c.refraction) * ui;
            p.chroma_px = (self.f(&c.dispersion_rgb) * 400.0).max(0.0);
            p.specular = self.f(&c.fresnel_intensity).max(0.0);
            p.border = self.f(&c.prismatic_rim_intensity).max(0.0);
        } else {
            // Mantém o véu: o alpha do tint vira parte da opacidade do véu.
            p.frost = (p.frost + tint[3] * 0.5).clamp(0.0, 0.94);
        }
        p
    }

    /// Enfileira um vidro-placa. `false` se não há backdrop de mídia (cai no preenchimento).
    #[allow(clippy::too_many_arguments)]
    fn push_glass(
        &mut self,
        layer: &LayerNode,
        mode: GlassMode,
        half: [f32; 2],
        xf: &Affine,
        center: [f32; 2],
        z: u32,
        opacity: f32,
        radius: f32,
        glyph: Option<GlyphSource>,
    ) -> bool {
        let Some(backdrop) = self.backdrop else {
            return false;
        };
        let params = self.glass_params(layer, opacity, radius);
        let Some(glass) = self.canvas.glass.as_mut() else {
            return false;
        };
        let id = glass.push(GlassDraw {
            mode,
            backdrop,
            params,
            center,
            half,
            basis: xf.m,
            glyph,
        });
        self.canvas.push_marker(z, id);
        true
    }

    // ── Fundo ──

    fn background(&mut self) {
        let (vw, vh) = (self.vw, self.vh);
        let center = [vw * 0.5, vh * 0.5];
        let half = [vw * 0.5, vh * 0.5];

        // Vídeo (frame registrado por `set_video_backdrop`) tem prioridade sobre a imagem; carrossel
        // ainda não existe; sem mídia de fundo = cor de clear (e vidro cai no tint).
        let scene = self.scene;
        if let Some(video) = self.video {
            let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                .kind(QuadKind::Texture)
                .uv(cover_uv(video.size.0, video.size.1, vw, vh));
            self.canvas.push(0, BlendKind::Normal, video.id, inst);
            self.backdrop = Some(self.backdrop_for(video.id, video.size, video.content_hash, true));
        } else if let BackgroundSource::Image { path } = &scene.background {
            if let Some(path) = self.resolve_asset(path) {
                if let Some(img) = self
                    .images
                    .get(&path, self.compositor, self.device, self.queue)
                {
                    let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                        .kind(QuadKind::Texture)
                        .uv(cover_uv(img.w, img.h, vw, vh));
                    self.canvas.push(0, BlendKind::Normal, img.id, inst);
                    let hash = hash_str(&path.to_string_lossy());
                    self.backdrop = Some(self.backdrop_for(img.id, (img.w, img.h), hash, false));
                }
            }
        }

        let dim = scene.dim.clamp(0.0, 1.0);
        if dim > 0.001 {
            let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                .color([0.0, 0.0, 0.0, dim]);
            self.canvas.push(0, BlendKind::Normal, TextureId::WHITE, inst);
        }
    }

    // ── Medição e layout ──

    fn measure(&mut self, layer: &LayerNode) -> Size {
        match layer.kind {
            LayerKind::Text => {
                let draw = self.layout_text(layer);
                Size {
                    w: draw.layout.width,
                    h: draw.layout.height,
                }
            }
            LayerKind::Stack => self.stack_size(layer),
            _ => Size {
                w: (self.f(&layer.width_px) * self.ui).max(0.0),
                h: (self.f(&layer.height_px) * self.ui).max(0.0),
            },
        }
    }

    fn stack_size(&mut self, layer: &LayerNode) -> Size {
        let spacing = layer.stack_spacing_px * self.ui;
        let horizontal = layer.stack_dir == StackDir::Horizontal;
        let (mut main, mut cross, mut count) = (0.0f32, 0.0f32, 0u32);
        for child in &layer.children {
            if !self.is_visible(child) {
                continue;
            }
            let s = self.measure(child);
            let (m, c) = if horizontal { (s.w, s.h) } else { (s.h, s.w) };
            main += m;
            cross = cross.max(c);
            count += 1;
        }
        if count > 1 {
            main += spacing * (count - 1) as f32;
        }
        if horizontal {
            Size { w: main, h: cross }
        } else {
            Size { w: cross, h: main }
        }
    }

    fn layout_text(&mut self, layer: &LayerNode) -> TextDraw {
        self.scratch.clear();
        self.scratch.push_str(&layer.id);
        self.scratch.push('@');
        self.scratch.push_str(self.canvas_key);

        let content = FxEngine::eval_text(&layer.text.text, self.ctx).value;
        let size_px = (self.f(&layer.text.size_px) * self.ui).max(1.0);
        let spec = TextSpec {
            text: &content,
            font_id: &layer.text.font_id,
            size_px,
            weight: layer.text.weight,
            line_spacing: layer.text.line_spacing,
            align: match layer.text.align {
                TextAlign::Start => TextAlignment::Start,
                TextAlign::Center => TextAlignment::Center,
                TextAlign::End => TextAlignment::End,
            },
        };
        let layout = self.text.update(self.device, self.scratch.as_str(), &spec);

        let mut tex = None;
        if !layout.empty {
            let existing = self.bindings.get(self.scratch.as_str()).copied();
            match existing {
                Some(b) if b.gen == layout.tex_gen => tex = Some(b.id),
                _ => {
                    if let Some(view) = self.text.texture_view(self.scratch.as_str()) {
                        let id =
                            self.compositor
                                .bind_view(self.device, existing.map(|b| b.id), view);
                        self.bindings.insert(
                            self.scratch.clone(),
                            TextBinding {
                                id,
                                gen: layout.tex_gen,
                            },
                        );
                        tex = Some(id);
                    }
                }
            }
        }
        TextDraw { layout, tex }
    }

    // ── Camadas ──

    fn layer(
        &mut self,
        layer: &LayerNode,
        parent: &ParentBox,
        parent_xf: Affine,
        inherited_opacity: f32,
        forced_center: Option<[f32; 2]>,
    ) {
        if !self.is_visible(layer) {
            return;
        }
        let opacity = self.f(&layer.opacity).clamp(0.0, 1.0) * inherited_opacity;
        if opacity <= 0.003 {
            return;
        }
        let scale = self.f(&layer.scale);
        if scale.abs() < 1e-3 {
            return;
        }
        let ui = self.ui;

        let mut text_draw = None;
        let size = match layer.kind {
            LayerKind::Text => {
                let draw = self.layout_text(layer);
                text_draw = Some(draw);
                Size {
                    w: draw.layout.width,
                    h: draw.layout.height,
                }
            }
            LayerKind::Stack => self.stack_size(layer),
            _ => Size {
                w: (self.f(&layer.width_px) * ui).max(0.0),
                h: (self.f(&layer.height_px) * ui).max(0.0),
            },
        };

        let offset = [self.f(&layer.offset_x_px) * ui, self.f(&layer.offset_y_px) * ui];
        let center = match forced_center {
            Some(c) => [c[0] + offset[0], c[1] + offset[1]],
            None => {
                let ax = parent.left + self.f(&layer.x) * parent.w + offset[0];
                let ay = parent.top + self.f(&layer.y) * parent.h + offset[1];
                let (fx, fy) = anchor_factors(layer.anchor);
                [ax + (0.5 - fx) * size.w, ay + (0.5 - fy) * size.h]
            }
        };
        let rot = self.f(&layer.rotation_deg).to_radians();
        let xf = parent_xf.child(center, rot, scale);

        let z = self.next_z();
        let blend = map_blend(layer.blend);
        match layer.kind {
            LayerKind::Text => self.draw_text(layer, text_draw, &xf, z, blend, opacity),
            LayerKind::Shape => self.draw_shape(layer, size, &xf, z, blend, opacity),
            LayerKind::Image => self.draw_image(layer, size, &xf, z, blend, opacity),
            // Svg/Icon (tesselação lyon) e DepthSubject (segmentação) ainda não têm renderer.
            LayerKind::Stack | LayerKind::Svg | LayerKind::Icon | LayerKind::DepthSubject => {}
        }

        if layer.children.is_empty() {
            return;
        }
        if layer.kind == LayerKind::Stack {
            self.stack_children(layer, size, &xf, opacity);
        } else {
            let pb = ParentBox {
                left: -size.w * 0.5,
                top: -size.h * 0.5,
                w: size.w,
                h: size.h,
            };
            for child in &layer.children {
                self.layer(child, &pb, xf, opacity, None);
            }
        }
    }

    fn stack_children(&mut self, layer: &LayerNode, size: Size, xf: &Affine, opacity: f32) {
        let spacing = layer.stack_spacing_px * self.ui;
        let horizontal = layer.stack_dir == StackDir::Horizontal;
        let pb = ParentBox {
            left: -size.w * 0.5,
            top: -size.h * 0.5,
            w: size.w,
            h: size.h,
        };
        let mut cursor = if horizontal { -size.w * 0.5 } else { -size.h * 0.5 };
        for child in &layer.children {
            if !self.is_visible(child) {
                continue;
            }
            let s = self.measure(child);
            let (center, advance) = if horizontal {
                ([cursor + s.w * 0.5, 0.0], s.w)
            } else {
                ([0.0, cursor + s.h * 0.5], s.h)
            };
            self.layer(child, &pb, *xf, opacity, Some(center));
            cursor += advance + spacing;
        }
    }

    // ── Preenchimento ──

    fn fill_style(&self, layer: &LayerNode, opacity: f32) -> FillStyle {
        let base = with_alpha(self.color(&layer.color), opacity);
        match layer.fill {
            FillMode::Solid => FillStyle {
                color: base,
                gradient: None,
                hollow: false,
            },
            FillMode::Gradient => {
                let kind = match layer.gradient_kind {
                    SchemaGradient::Linear => GradientKind::Linear,
                    SchemaGradient::Radial => GradientKind::Radial,
                    SchemaGradient::Sweep => GradientKind::Sweep,
                };
                // Ângulo estilo CSS: 0° aponta para cima, 90° para a direita.
                let a = layer.gradient_angle_deg.to_radians();
                FillStyle {
                    color: base,
                    gradient: Some((
                        kind,
                        with_alpha(parse_color(&layer.color2), opacity),
                        [a.sin(), -a.cos()],
                    )),
                    hollow: false,
                }
            }
            FillMode::Hollow => FillStyle {
                color: base,
                gradient: None,
                hollow: true,
            },
            // Vidro óptico/blur: Fase 2. Até lá, preenchimento translúcido com o tint do cristal.
            FillMode::Adaptive | FillMode::ClassicBlur | FillMode::OpticalCrystal => FillStyle {
                color: with_alpha(self.color(&layer.crystal.tint_rgba), opacity),
                gradient: None,
                hollow: false,
            },
        }
    }

    fn push_shadow(
        &mut self,
        layer: &LayerNode,
        quad: ShadowQuad,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if layer.shadow.radius_px <= 0.0 {
            return;
        }
        let ui = self.ui;
        let color = with_alpha(parse_color(&layer.shadow.color_rgba), opacity);
        if color[3] <= 0.003 {
            return;
        }
        let center = [
            quad.center[0] + layer.shadow.dx_px * ui,
            quad.center[1] + layer.shadow.dy_px * ui,
        ];
        let blur_local = layer.shadow.radius_px * ui / quad.scale.max(1e-3);
        let inst = QuadInstance::new(center, quad.half, quad.basis, quad.scale)
            .radius(quad.radius)
            .color(color)
            .shadow(blur_local);
        self.canvas.push(z, blend, TextureId::WHITE, inst);
    }

    // ── Desenho por tipo ──

    fn draw_text(
        &mut self,
        layer: &LayerNode,
        draw: Option<TextDraw>,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        let Some(draw) = draw else {
            return;
        };
        let (Some(tex), false) = (draw.tex, draw.layout.empty) else {
            return;
        };
        let scale = xf.scale();
        let half = [draw.layout.quad_width * 0.5, draw.layout.quad_height * 0.5];
        let mut center = xf.t;
        // Sem rotação/escala: alinha o canto do quad à grade de pixels (texto nítido, 1:1).
        if xf.is_identity_linear() {
            center[0] = (center[0] - half[0]).round() + half[0];
            center[1] = (center[1] - half[1]).round() + half[1];
        }
        let uv = [0.0, 0.0, draw.layout.uv_max[0], draw.layout.uv_max[1]];

        // Vidro nos glifos: normais geradas na GPU a partir da máscara do slot de texto.
        if Self::is_glass(layer) && self.backdrop.is_some() {
            let content = FxEngine::eval_text(&layer.text.text, self.ctx).value;
            let mut token = hash_combine(hash_str(&content), hash_str(&layer.text.font_id));
            token = hash_combine(token, u64::from(draw.layout.quad_width.to_bits()));
            token = hash_combine(token, u64::from(draw.layout.quad_height.to_bits()));
            token = hash_combine(token, u64::from(layer.text.weight));
            token = hash_combine(token, draw.layout.tex_gen);
            let source = GlyphSource {
                key: format!("{}@{}", layer.id, self.canvas_key),
                token,
                size: (
                    draw.layout.quad_width.ceil().max(1.0) as u32,
                    draw.layout.quad_height.ceil().max(1.0) as u32,
                ),
                bevel_px: (self.f(&layer.crystal.bevel_width) * self.ui).max(0.0),
            };
            if self.push_glass(
                layer,
                GlassMode::Glyph,
                half,
                xf,
                center,
                z,
                opacity,
                0.0,
                Some(source),
            ) {
                return;
            }
        }

        // Sombra do texto: cópia deslocada da máscara (sem blur até a Fase 2).
        if layer.shadow.radius_px > 0.0 {
            let ui = self.ui;
            let color = with_alpha(parse_color(&layer.shadow.color_rgba), opacity);
            if color[3] > 0.003 {
                let c = [
                    center[0] + layer.shadow.dx_px * ui,
                    center[1] + layer.shadow.dy_px * ui,
                ];
                let inst = QuadInstance::new(c, half, xf.m, scale)
                    .kind(QuadKind::Mask)
                    .color(color)
                    .uv(uv);
                self.canvas.push(z, blend, tex, inst);
            }
        }

        let fill = self.fill_style(layer, opacity);
        let mut inst = QuadInstance::new(center, half, xf.m, scale)
            .kind(QuadKind::Mask)
            .color(fill.color)
            .uv(uv);
        if let Some((kind, color2, dir)) = fill.gradient {
            inst = inst.gradient(kind, color2, dir);
        }
        self.canvas.push(z, blend, tex, inst);
    }

    fn draw_shape(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 {
            return;
        }
        let ui = self.ui;
        let scale = xf.scale();
        let half = [size.w * 0.5, size.h * 0.5];
        let min_half = half[0].min(half[1]);
        let radius = match layer.shape_kind {
            ShapeKind::Rect | ShapeKind::Bar => layer.corner_radius_px * ui,
            ShapeKind::Circle | ShapeKind::Ring | ShapeKind::Line => min_half,
        };
        let quad = ShadowQuad {
            center: xf.t,
            half,
            basis: xf.m,
            scale,
            radius,
        };
        self.push_shadow(layer, quad, z, blend, opacity);

        let fill = self.fill_style(layer, opacity);
        let stroke_w = layer.stroke.width_px.max(0.0) * ui;
        let stroke_color = with_alpha(parse_color(&layer.stroke.color_rgba), opacity);

        let mut base = QuadInstance::new(xf.t, half, xf.m, scale).radius(radius);

        match layer.shape_kind {
            ShapeKind::Ring => {
                let thickness = if stroke_w > 0.0 {
                    stroke_w
                } else {
                    (min_half * 0.12).max(2.0 * ui)
                };
                let inst = base.color([0.0; 4]).border(thickness, fill.color);
                self.canvas.push(z, blend, TextureId::WHITE, inst);
            }
            ShapeKind::Bar => {
                let track = with_alpha(parse_color(&layer.track_color), opacity);
                self.canvas
                    .push(z, blend, TextureId::WHITE, base.color(track));

                let pct = FxEngine::eval_expr(&layer.progress_expr, self.ctx)
                    .value
                    .trim()
                    .trim_end_matches('%')
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(0.0);
                let progress = if pct.is_finite() {
                    (pct / 100.0).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let fw = size.w * progress;
                if fw >= 0.5 {
                    let local = [-size.w * 0.5 + fw * 0.5, 0.0];
                    let c = xf.apply(local);
                    let mut bar = QuadInstance::new(c, [fw * 0.5, half[1]], xf.m, scale)
                        .radius(radius)
                        .color(fill.color);
                    if let Some((kind, color2, dir)) = fill.gradient {
                        bar = bar.gradient(kind, color2, dir);
                    }
                    self.canvas.push(z, blend, TextureId::WHITE, bar);
                }
            }
            ShapeKind::Rect | ShapeKind::Circle | ShapeKind::Line => {
                if Self::is_glass(layer)
                    && self.push_glass(
                        layer,
                        GlassMode::Plate,
                        half,
                        xf,
                        xf.t,
                        z,
                        opacity,
                        radius,
                        None,
                    )
                {
                    return;
                }
                if fill.hollow {
                    let w = if stroke_w > 0.0 { stroke_w } else { 2.0 * ui };
                    let c = if stroke_w > 0.0 { stroke_color } else { fill.color };
                    base = base.color([0.0; 4]).border(w, c);
                } else {
                    base = base.color(fill.color);
                    if let Some((kind, color2, dir)) = fill.gradient {
                        base = base.gradient(kind, color2, dir);
                    }
                    if stroke_w > 0.0 {
                        base = base.border(stroke_w, stroke_color);
                    }
                }
                self.canvas.push(z, blend, TextureId::WHITE, base);
            }
        }
    }

    fn draw_image(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 {
            return;
        }
        let raw = FxEngine::eval_string(&layer.asset, self.ctx).value;
        let Some(path) = self.resolve_asset(&raw) else {
            return;
        };
        let Some(img) = self
            .images
            .get(&path, self.compositor, self.device, self.queue)
        else {
            return;
        };

        let scale = xf.scale();
        let half = [size.w * 0.5, size.h * 0.5];
        let radius = layer.corner_radius_px * self.ui;
        let quad = ShadowQuad {
            center: xf.t,
            half,
            basis: xf.m,
            scale,
            radius,
        };
        self.push_shadow(layer, quad, z, blend, opacity);

        let inst = QuadInstance::new(xf.t, half, xf.m, scale)
            .kind(QuadKind::Texture)
            .radius(radius)
            .color([1.0, 1.0, 1.0, opacity])
            .uv(cover_uv(img.w, img.h, size.w, size.h));
        self.canvas.push(z, blend, img.id, inst);
    }
}

/// Geometria usada para a sombra de uma camada.
#[derive(Clone, Copy)]
struct ShadowQuad {
    center: [f32; 2],
    half: [f32; 2],
    basis: [f32; 4],
    scale: f32,
    radius: f32,
}
