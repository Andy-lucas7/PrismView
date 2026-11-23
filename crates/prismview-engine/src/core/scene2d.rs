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

    /// Inversa (`None` se degenerada).
    fn inverse(&self) -> Option<Self> {
        let m = &self.m;
        let det = m[0] * m[3] - m[2] * m[1];
        if !det.is_finite() || det.abs() < 1e-8 {
            return None;
        }
        let k = 1.0 / det;
        let inv = [m[3] * k, -m[1] * k, -m[2] * k, m[0] * k];
        let t = [
            -(inv[0] * self.t[0] + inv[2] * self.t[1]),
            -(inv[1] * self.t[0] + inv[3] * self.t[1]),
        ];
        Some(Self { m: inv, t })
    }

    /// `self ∘ other`.
    fn then_apply(&self, other: &Self) -> Self {
        let (a, b) = (&self.m, &other.m);
        Self {
            m: [
                a[0] * b[0] + a[2] * b[1],
                a[1] * b[0] + a[3] * b[1],
                a[0] * b[2] + a[2] * b[3],
                a[1] * b[2] + a[3] * b[3],
            ],
            t: self.apply(other.t),
        }
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

/// Teto de memória da decodificação de uma imagem (bytes).
const IMAGE_MAX_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
/// Lado máximo aceito na decodificação (px); maiores são recusados.
const IMAGE_MAX_SIDE: u32 = 16384;

/// Fator (≤ 1) para a imagem ainda cobrir a saída inteira (`viewport`) sem ultrapassar `max`.
/// Maior que isso só gastaria VRAM: na tela ela nunca aparece maior que a saída.
fn image_target_scale(size: (u32, u32), viewport: (u32, u32), max: u32) -> f32 {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let cover = (viewport.0.max(1) as f32 / w).max(viewport.1.max(1) as f32 / h);
    let device = max.max(1) as f32 / w.max(h);
    cover.min(device).min(1.0)
}

/// Acorda o loop quando uma imagem termina de decodificar (registrado 1× pelo host).
pub type ImageWaker = Arc<dyn Fn() + Send + Sync>;

static IMAGE_WAKER: OnceLock<ImageWaker> = OnceLock::new();

/// Registra quem acorda o loop após decodificações em segundo plano (chamar uma vez no início).
pub fn set_image_waker(waker: ImageWaker) {
    if IMAGE_WAKER.set(waker).is_err() {
        eprintln!("[scene2d] image waker já registrado");
    }
}

/// Pixels RGBA8 pré-multiplicados prontos para upload.
struct DecodedImage {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// Imagens: decodificadas em threads (nunca no loop de render) e enviadas à GPU no rebuild
/// seguinte. Enquanto decodifica, a camada não é desenhada.
#[derive(Default)]
struct ImageCache {
    loaded: HashMap<String, ImageEntry>,
    failed: HashSet<String>,
    pending: HashMap<String, mpsc::Receiver<Option<DecodedImage>>>,
    /// Alguma decodificação terminou desde o último `take_completed`.
    completed: Arc<AtomicBool>,
}

impl ImageCache {
    fn get(
        &mut self,
        path: &Path,
        viewport: (u32, u32),
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
        let Some(rx) = self.pending.get(&key) else {
            self.spawn_decode(path, key, compositor.max_texture_dim(), viewport);
            return None;
        };
        let decoded = match rx.try_recv() {
            Ok(decoded) => decoded,
            Err(mpsc::TryRecvError::Empty) => return None,
            // Thread caiu (pânico no decoder): trata como falha.
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        self.pending.remove(&key);
        let entry = decoded.and_then(|img| {
            compositor
                .create_texture_rgba_mipped(device, queue, img.w, img.h, &img.rgba)
                .map(|id| ImageEntry { id, w: img.w, h: img.h })
        });
        match entry {
            Some(entry) => {
                self.loaded.insert(key, entry);
                Some(entry)
            }
            None => {
                eprintln!("[scene2d] imagem não carregou: {}", path.display());
                self.failed.insert(key);
                None
            }
        }
    }

    fn spawn_decode(&mut self, path: &Path, key: String, max_dim: u32, viewport: (u32, u32)) {
        let (tx, rx) = mpsc::channel();
        let completed = Arc::clone(&self.completed);
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("prismview-image-decode".into())
            .spawn(move || {
                let decoded = Self::decode(&path, max_dim, viewport);
                let delivered = tx.send(decoded).is_ok();
                completed.store(true, Ordering::Release);
                if delivered {
                    if let Some(wake) = IMAGE_WAKER.get() {
                        wake();
                    }
                }
            });
        match spawned {
            Ok(_) => {
                self.pending.insert(key, rx);
            }
            Err(err) => {
                eprintln!("[scene2d] thread de imagem: {err}");
                self.failed.insert(key);
            }
        }
    }

    /// Decodifica (CPU, fora do loop), limita ao máximo do device e pré-multiplica.
    /// Decodifica com limites (sem "bomba" de descompressão), reduz ao necessário para cobrir a
    /// saída (`viewport`, px) e ao máximo do device, e pré-multiplica. Fora do loop de render.
    fn decode(path: &Path, max: u32, viewport: (u32, u32)) -> Option<DecodedImage> {
        let decoded = (|| -> image::ImageResult<image::DynamicImage> {
            let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(IMAGE_MAX_ALLOC_BYTES);
            limits.max_image_width = Some(IMAGE_MAX_SIDE);
            limits.max_image_height = Some(IMAGE_MAX_SIDE);
            reader.limits(limits);
            reader.decode()
        })();
        let mut img = match decoded {
            Ok(img) => img.into_rgba8(),
            Err(err) => {
                eprintln!("[scene2d] imagem {}: {err}", path.display());
                return None;
            }
        };
        let (w, h) = img.dimensions();
        let k = image_target_scale((w, h), viewport, max);
        if k < 1.0 {
            let nw = ((w as f32 * k).ceil() as u32).max(1);
            let nh = ((h as f32 * k).ceil() as u32).max(1);
            img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::CatmullRom);
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
        Some(DecodedImage { w, h, rgba: img.into_raw() })
    }

    /// `true` uma vez por lote de decodificações concluídas (o rebuild as envia à GPU).
    fn take_completed(&self) -> bool {
        !self.pending.is_empty() && self.completed.swap(false, Ordering::AcqRel)
    }

    fn release_all(&mut self, compositor: &mut Compositor2D) {
        for (_, entry) in self.loaded.drain() {
            compositor.release_texture(entry.id);
        }
        self.failed.clear();
        // Threads em curso terminam sozinhas (o envio falha com o receptor descartado).
        self.pending.clear();
    }
}

#[derive(Clone)]
struct TextBinding {
    id: TextureId,
    gen: u64,
    key: Arc<str>,
}

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

// ───────────────────────── SVG (resvg) ─────────────────────────

/// Máximo de rasters de SVG mantidos (markup × tamanho); acima disso o cache é esvaziado.
const SVG_CACHE_MAX: usize = 96;
/// Lado máximo do raster de um SVG (px físicos).
const SVG_MAX_PX: u32 = 2048;

#[derive(Default)]
struct SvgCache {
    loaded: HashMap<(u64, u32, u32), ImageEntry>,
    failed: HashSet<u64>,
}

impl SvgCache {
    fn get(
        &mut self,
        markup: &str,
        w: u32,
        h: u32,
        compositor: &mut Compositor2D,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<ImageEntry> {
        let hash = hash_str(markup);
        if self.failed.contains(&hash) {
            return None;
        }
        if let Some(entry) = self.loaded.get(&(hash, w, h)) {
            return Some(*entry);
        }
        if self.loaded.len() >= SVG_CACHE_MAX {
            self.release_all(compositor);
        }
        let Some(rgba) = rasterize_svg(markup, w, h) else {
            self.failed.insert(hash);
            return None;
        };
        let id = compositor.create_texture_rgba(device, queue, w, h, &rgba)?;
        let entry = ImageEntry { id, w, h };
        self.loaded.insert((hash, w, h), entry);
        Some(entry)
    }

    fn release_all(&mut self, compositor: &mut Compositor2D) {
        for (_, entry) in self.loaded.drain() {
            compositor.release_texture(entry.id);
        }
        self.failed.clear();
    }
}

/// Rasteriza um documento SVG em `w×h` (contain, centralizado). Saída RGBA8 pré-multiplicada.
fn rasterize_svg(markup: &str, w: u32, h: u32) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(markup, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let (sw, sh) = (size.width().max(1e-3), size.height().max(1e-3));
    let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    let k = (w as f32 / sw).min(h as f32 / sh);
    let tx = (w as f32 - sw * k) * 0.5;
    let ty = (h as f32 - sh * k) * 0.5;
    resvg::render(&tree, tiny_skia::Transform::from_row(k, 0.0, 0.0, k, tx, ty), &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// Tamanho de raster em degraus de 4 px (evita refazer o raster por variações sub-pixel).
fn svg_bucket(px: f32) -> u32 {
    let v = if px.is_finite() { px.ceil().max(4.0) as u32 } else { 4 };
    (v.div_ceil(4) * 4).min(SVG_MAX_PX)
}

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

// ───────────────────────── Construção do frame ─────────────────────────

#[derive(Clone)]
struct TextDraw {
    layout: TextLayout,
    tex: Option<TextureId>,
    content_hash: u64,
    slot_key: Option<Arc<str>>,
}

struct FillStyle {
    color: [f32; 4],
    gradient: Option<(GradientKind, [f32; 4], [f32; 2])>,
    hollow: bool,
}

struct Frame<'a> {
    videos: &'a HashMap<String, VideoBinding>,
    svgs: &'a mut SvgCache,
    touch_index: &'a HashMap<String, u32>,
    scene3d_out: Option<Scene3dOutput>,
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
    /// Deltas dos bindings reativos por camada (somados à base; nunca gravados nela).
    fx: &'a LayerBindingSolver,
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
            self.layer(layer, &root, Affine::IDENTITY, 1.0, None, false);
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
        resolve_asset_path(self.scene, self.asset_root, raw)
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

    /// Mapeia `OpticalCrystalParams` para os parâmetros do shader (px físicos), em paridade 1:1
    /// com `OpticalGlassSurface.tsx` / `opticalGlassWebGL.ts`:
    /// - No modo `Glyph`, `pigment` recebe a cor + alpha reais do texto (`layer.color`), garantindo
    ///   cor e opacidade fiéis mesmo com `frost == 0`.
    /// - `brightness`, `contrast`, `saturation`, `light_angle_deg`, `refraction`, `bevel_width`,
    ///   dispersão (`×400`), especular (`fresnel_intensity`) e borda (`prismatic_rim_intensity`).
    fn glass_params(
        &self,
        layer: &LayerNode,
        mode: GlassMode,
        opacity: f32,
        radius: f32,
    ) -> GlassParams {
        let c = &layer.crystal;
        let ui = self.ui;
        let tint = self.color(&c.tint_rgba);
        let g = self.fx.fx(layer.id.as_str()).map(|f| f.glass).unwrap_or_default();
        let pigment = if mode == GlassMode::Glyph {
            self.color(&layer.color)
        } else {
            [1.0, 1.0, 1.0, 0.0]
        };
        let mut p = GlassParams {
            blur_px: ((self.f(&c.blur_px) + g.blur_px) * ui).clamp(0.0, 80.0 * ui),
            frost: (self.f(&c.frost_amount) + g.frost).clamp(0.0, 0.94),
            tint: [tint[0], tint[1], tint[2]],
            noise: c.grain.clamp(0.0, 0.4),
            brightness: self.f(&c.brightness).clamp(0.5, 1.8),
            contrast: self.f(&c.contrast).clamp(0.6, 1.8),
            saturation: self.f(&c.saturation).clamp(0.0, 2.6),
            opacity: opacity.clamp(0.0, 1.0),
            radius_px: radius,
            bevel_px: self.f(&c.bevel_width).clamp(4.0, 80.0) * ui,
            light_angle_deg: self.f(&c.light_angle_deg),
            pigment,
            ..GlassParams::default()
        };
        if layer.fill == FillMode::OpticalCrystal {
            p.refraction_px = (self.f(&c.refraction) + g.refraction).clamp(0.0, 60.0) * ui.max(0.5);
            p.chroma_px = ((self.f(&c.dispersion_rgb) + g.dispersion) * 400.0).clamp(0.0, 6.0);
            p.specular = (self.f(&c.fresnel_intensity) + g.specular).clamp(0.0, 1.0);
            p.border = (self.f(&c.prismatic_rim_intensity) + g.rim).clamp(0.0, 0.8);
        } else if p.frost <= 0.001 && tint[3] > 0.001 {
            // `Adaptive`/`ClassicBlur` legado sem `frost_amount`: aproveita o alpha do tint.
            p.frost = (tint[3] * 0.5).clamp(0.0, 0.94);
        }
        p
    }

    /// Enfileira um vidro-placa ou glifo. `false` se não há backdrop de mídia (cai no preenchimento).
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
        let params = self.glass_params(layer, mode, opacity, radius);
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

        // Vídeo (frames de `set_video_frame`) ou imagem; carrossel ainda não existe; sem mídia de
        // fundo = cor de clear (e o vidro usa a próxima mídia full-bleed ou cai no tint).
        let scene = self.scene;
        let videos = self.videos;
        if let BackgroundSource::Video { path, .. } = &scene.background {
            let binding = self
                .resolve_asset(path)
                .and_then(|p| videos.get(&*p.to_string_lossy()));
            if let Some((id, v)) = binding.and_then(|v| v.alias.map(|id| (id, v))) {
                let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                    .kind(QuadKind::Opaque)
                    .uv(cover_uv(v.size.0, v.size.1, vw, vh));
                self.canvas.push(0, BlendKind::Normal, id, inst);
                self.backdrop = Some(self.backdrop_for(id, v.size, v.content_hash, true));
            }
        } else if let BackgroundSource::Image { path } = &scene.background {
            if let Some(path) = self.resolve_asset(path) {
                if let Some(img) = self
                    .images
                    .get(&path, (self.vw as u32, self.vh as u32), self.compositor, self.device, self.queue)
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
        let content_hash = hash_str(&content);
        let size_px = (self.f(&layer.text.size_px) * self.ui).max(1.0);
        let spec = TextSpec {
            text: &content,
            font_id: &layer.text.font_id,
            size_px,
            weight: layer.text.weight,
            line_spacing: layer.text.line_spacing,
            letter_spacing_em: layer.text.letter_spacing_em,
            align: match layer.text.align {
                TextAlign::Start => TextAlignment::Start,
                TextAlign::Center => TextAlignment::Center,
                TextAlign::End => TextAlignment::End,
            },
        };
        let layout = self.text.update(self.device, self.scratch.as_str(), &spec);

        let mut tex = None;
        let mut slot_key = None;
        if !layout.empty {
            let existing = self.bindings.get(self.scratch.as_str());
            match existing {
                Some(b) if b.gen == layout.tex_gen => {
                    tex = Some(b.id);
                    slot_key = Some(Arc::clone(&b.key));
                }
                other => {
                    let prev_id = other.map(|b| b.id);
                    let reused_key = other.map(|b| Arc::clone(&b.key));
                    if let Some(view) = self.text.texture_view(self.scratch.as_str()) {
                        let id = self.compositor.bind_view(self.device, prev_id, view);
                        let key = reused_key.unwrap_or_else(|| Arc::from(self.scratch.as_str()));
                        self.bindings.insert(
                            self.scratch.clone(),
                            TextBinding {
                                id,
                                gen: layout.tex_gen,
                                key: Arc::clone(&key),
                            },
                        );
                        tex = Some(id);
                        slot_key = Some(key);
                    }
                }
            }
        }
        TextDraw {
            layout,
            tex,
            content_hash,
            slot_key,
        }
    }

    // ── Camadas ──

    fn layer(
        &mut self,
        layer: &LayerNode,
        parent: &ParentBox,
        parent_xf: Affine,
        inherited_opacity: f32,
        forced_center: Option<[f32; 2]>,
        parent_moving: bool,
    ) {
        let bound = self.fx.has_binding(layer.id.as_str());
        if !self.is_visible(layer) {
            // Binding pode trazê-la de volta: o caminho rápido não tem itens dela.
            self.canvas.bound_skipped |= bound;
            return;
        }
        // Base (estática/FX) + delta dos bindings: o delta é recalculado por quadro, sem deriva.
        let fx = self.fx.fx(layer.id.as_str()).unwrap_or_default();
        let own_moving = self.fx.is_layer_animating(layer.id.as_str());
        let moving = parent_moving || own_moving;
        let opacity = (self.f(&layer.opacity) + fx.opacity).clamp(0.0, 1.0) * inherited_opacity;
        if opacity <= 0.003 {
            self.canvas.bound_skipped |= bound;
            return;
        }
        let base_scale = self.f(&layer.scale);
        let scale = base_scale * (1.0 + fx.scale);
        if scale.abs() < 1e-3 {
            self.canvas.bound_skipped |= bound;
            return;
        }
        // Início da subárvore (caminho rápido dos bindings: `Canvas::bound`).
        let seq0 = self.canvas.next_seq();
        let hits0 = self.canvas.hits.len();
        let markers0 = self.canvas.marker_count();
        let had_scene3d = self.canvas.scene3d.is_some();
        let ui = self.ui;

        let mut text_draw = None;
        let size = match layer.kind {
            LayerKind::Text => {
                let draw = self.layout_text(layer);
                let sz = Size {
                    w: draw.layout.width,
                    h: draw.layout.height,
                };
                text_draw = Some(draw);
                sz
            }
            LayerKind::Stack => self.stack_size(layer),
            LayerKind::Shape
                if layer.id.ends_with("__pill")
                    && layer.children.len() == 1
                    && layer.children[0].kind == LayerKind::Text =>
            {
                let child = &layer.children[0];
                let draw = self.layout_text(child);
                let font_px = (self.f(&child.text.size_px) * ui).max(1.0);
                let pad_x = (font_px * 0.34).max(12.0 * ui);
                let pad_y = (font_px * 0.16).max(6.0 * ui);
                Size {
                    w: draw.layout.width + pad_x * 2.0,
                    h: draw.layout.height + pad_y * 2.0,
                }
            }
            _ if layer.fill_parent => Size {
                w: parent.w.max(0.0),
                h: parent.h.max(0.0),
            },
            _ => Size {
                w: (self.f(&layer.width_px) * ui).max(0.0),
                h: (self.f(&layer.height_px) * ui).max(0.0),
            },
        };

        let offset = [
            (self.f(&layer.offset_x_px) + fx.offset[0]) * ui,
            (self.f(&layer.offset_y_px) + fx.offset[1]) * ui,
        ];
        let center = match forced_center {
            Some(c) => [c[0] + offset[0], c[1] + offset[1]],
            None => {
                let ax = parent.left + self.f(&layer.x) * parent.w + offset[0];
                let ay = parent.top + self.f(&layer.y) * parent.h + offset[1];
                let (fx, fy) = anchor_factors(layer.anchor);
                [ax + (0.5 - fx) * size.w, ay + (0.5 - fy) * size.h]
            }
        };
        let rot = (self.f(&layer.rotation_deg) + fx.rotation_deg).to_radians();
        let xf = parent_xf.child(center, rot, scale);
        // Parâmetros sem o delta do binding (o caminho rápido recompõe com o delta novo).
        let bound_base = bound.then(|| {
            (
                [center[0] - fx.offset[0] * ui, center[1] - fx.offset[1] * ui],
                rot - fx.rotation_deg.to_radians(),
            )
        });

        // Controle de mídia tocável: mesma caixa orientada do desenho (mín. 10 px lógicos).
        let action = DesktopAction::from_touch(layer.touch.action_type).or_else(|| {
            self.touch_index
                .get(layer.id.as_str())
                .map(|&i| DesktopAction::Custom(i))
        });
        if let Some(action) = action {
            let min = 10.0 * ui;
            let half = [(size.w * 0.5).max(min), (size.h * 0.5).max(min)];
            if let Some(region) = HitRegion::new(xf.t, xf.m, half, action) {
                self.canvas.hits.push(region);
            }
        }

        let z = self.next_z();
        let blend = map_blend(layer.blend);
        match layer.kind {
            LayerKind::Text => self.draw_text(layer, text_draw, &xf, z, blend, opacity, moving),
            LayerKind::Shape => self.draw_shape(layer, size, &xf, z, blend, opacity),
            LayerKind::Image => self.draw_image(layer, size, &xf, z, blend, opacity),
            LayerKind::Scene3d => self.draw_scene3d(size, &xf, z, blend, opacity, layer.fill_parent),
            LayerKind::Video => self.draw_video(layer, size, &xf, z, blend, opacity),
            LayerKind::Svg | LayerKind::Icon => self.draw_svg(layer, size, &xf, z, blend, opacity),
            // DepthSubject (segmentação) ainda não tem renderer.
            LayerKind::Stack | LayerKind::Group | LayerKind::DepthSubject => {}
        }

        if !layer.children.is_empty() {
            if layer.kind == LayerKind::Stack {
                self.stack_children(layer, size, &xf, opacity, moving);
            } else {
                let pb = ParentBox {
                    left: -size.w * 0.5,
                    top: -size.h * 0.5,
                    w: size.w,
                    h: size.h,
                };
                for child in &layer.children {
                    self.layer(child, &pb, xf, opacity, None, moving);
                }
            }
        }

        // Fim da subárvore: pós-ordem (filhos já gravados antes deste pai).
        if let Some((center_base, rot_base)) = bound_base {
            let rigid = self.canvas.marker_count() != markers0
                || (!had_scene3d && self.canvas.scene3d.is_some());
            let entry = BoundSubtree {
                key: Arc::from(layer.id.as_str()),
                seq: seq0..self.canvas.next_seq(),
                hits: hits0..self.canvas.hits.len(),
                parent_m: parent_xf.m,
                parent_t: parent_xf.t,
                center_base,
                rot_base,
                scale_base: base_scale,
                ui,
                fx0: fx,
                moving0: own_moving,
                rigid,
            };
            self.canvas.bound.push(entry);
        }
    }

    fn stack_children(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        opacity: f32,
        moving: bool,
    ) {
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
            self.layer(child, &pb, *xf, opacity, Some(center), moving);
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
            // Fallback sem backdrop de mídia: texto preserva sua cor/alpha reais; placa usa o tint/frost.
            FillMode::Adaptive | FillMode::ClassicBlur | FillMode::OpticalCrystal => {
                let fallback = if layer.kind == LayerKind::Text {
                    let c = self.color(&layer.color);
                    if c[3] > 0.003 {
                        c
                    } else {
                        self.color(&layer.crystal.tint_rgba)
                    }
                } else {
                    let mut c = self.color(&layer.crystal.tint_rgba);
                    let frost = self.f(&layer.crystal.frost_amount).clamp(0.0, 0.94);
                    if frost > 0.002 {
                        c[3] = frost;
                    }
                    c
                };
                FillStyle {
                    color: with_alpha(fallback, opacity),
                    gradient: None,
                    hollow: false,
                }
            }
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

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        layer: &LayerNode,
        draw: Option<TextDraw>,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
        moving: bool,
    ) {
        let Some(draw) = draw else {
            return;
        };
        let (Some(tex), false) = (draw.tex, draw.layout.empty) else {
            return;
        };
        let stretch_y = self.f(&layer.text.stem_stretch.stretch).clamp(1.0, 2.8);
        let text_xf = if (stretch_y - 1.0).abs() > 1e-3 {
            Affine {
                m: [xf.m[0], xf.m[1], xf.m[2] * stretch_y, xf.m[3] * stretch_y],
                t: xf.t,
            }
        } else {
            *xf
        };
        let scale = text_xf.scale();
        let half = [draw.layout.quad_width * 0.5, draw.layout.quad_height * 0.5];
        let mut center = text_xf.t;
        let glass_glyph = Self::is_glass(layer) && self.backdrop.is_some();
        // Sem rotação/escala e em repouso (fora de vidro/parallax): alinha à grade de pixels (1:1).
        // Em glifos de vidro ou durante parallax, mantém subpixel contínuo para movimento liso 60+ FPS.
        if !glass_glyph && !moving && text_xf.is_identity_linear() {
            center[0] = (center[0] - half[0]).round() + half[0];
            center[1] = (center[1] - half[1]).round() + half[1];
        }
        let uv = [0.0, 0.0, draw.layout.uv_max[0], draw.layout.uv_max[1]];

        // Vidro nos glifos: normais geradas na GPU a partir da máscara do slot de texto.
        if glass_glyph {
            if let Some(slot_key) = draw.slot_key {
                let mut token = hash_combine(draw.content_hash, hash_str(&layer.text.font_id));
                token = hash_combine(token, u64::from(draw.layout.quad_width.to_bits()));
                token = hash_combine(token, u64::from(draw.layout.quad_height.to_bits()));
                token = hash_combine(token, u64::from(layer.text.weight));
                token = hash_combine(token, u64::from(layer.text.letter_spacing_em.to_bits()));
                token = hash_combine(token, draw.layout.tex_gen);
                let source = GlyphSource {
                    key: slot_key,
                    token,
                    size: (
                        draw.layout.quad_width.ceil().max(1.0) as u32,
                        draw.layout.quad_height.ceil().max(1.0) as u32,
                    ),
                    bevel_px: self.f(&layer.crystal.bevel_width).clamp(4.0, 80.0) * self.ui,
                };
                if self.push_glass(
                    layer,
                    GlassMode::Glyph,
                    half,
                    &text_xf,
                    center,
                    z,
                    opacity,
                    0.0,
                    Some(source),
                ) {
                    return;
                }
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
                let inst = QuadInstance::new(c, half, text_xf.m, scale)
                    .kind(QuadKind::Mask)
                    .color(color)
                    .uv(uv);
                self.canvas.push(z, blend, tex, inst);
            }
        }

        let fill = self.fill_style(layer, opacity);
        let mut inst = QuadInstance::new(center, half, text_xf.m, scale)
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

    /// Camada 3D: registra a caixa (só a primeira da cena; um `Scene3dRenderer` por motor) e
    /// desenha a textura de saída já renderizada (pré-multiplicada, mesmo aspecto da caixa).
    #[allow(clippy::too_many_arguments)]
    fn draw_scene3d(
        &mut self,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
        fill_parent: bool,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 || self.canvas.scene3d.is_some() {
            return;
        }
        let half = [size.w * 0.5, size.h * 0.5];
        self.canvas.scene3d = Some(Scene3dPlacement {
            center: xf.t,
            half,
            basis: xf.m,
        });
        let Some(out) = self.scene3d_out else {
            return; // 1º quadro: o 3D ainda não renderizou; a lista é refeita ao registrar.
        };
        let inst = QuadInstance::new(xf.t, half, xf.m, xf.scale())
            .kind(QuadKind::Texture)
            .color([1.0, 1.0, 1.0, opacity])
            .uv(cover_uv(out.size.0, out.size.1, size.w, size.h));
        self.canvas.push(z, blend, out.id, inst);
        // 3D full-bleed: vira o backdrop do vidro desenhado por cima (blur refeito por quadro 3D).
        if fill_parent {
            self.backdrop = Some(self.backdrop_for(out.id, out.size, SCENE3D_BACKDROP_HASH, true));
        }
    }

    /// Camada de vídeo: frame atual do decoder (zero-copy), cover na caixa, cantos arredondados.
    fn draw_video(
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
        let videos = self.videos;
        let Some(v) = videos.get(&*path.to_string_lossy()) else {
            return;
        };
        let Some(id) = v.alias else {
            return; // 1º frame ainda não decodificado; `set_video_frame` refaz a lista
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
            .kind(QuadKind::Opaque)
            .radius(radius)
            .color([1.0, 1.0, 1.0, opacity])
            .uv(cover_uv(v.size.0, v.size.1, size.w, size.h));
        self.canvas.push(z, blend, id, inst);
        if layer.fill_parent {
            self.backdrop = Some(self.backdrop_for(id, v.size, v.content_hash, true));
        }
    }

    /// Ícone/SVG: `svg_markup` rasterizado (resvg) no tamanho físico da caixa, tingido por `color`
    /// (`currentColor` do markup = branco; SVG multicolorido usa `color` branco).
    fn draw_svg(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if layer.svg_markup.trim().is_empty() || size.w <= 0.5 || size.h <= 0.5 {
            return;
        }
        let scale = xf.scale();
        let (pw, ph) = (svg_bucket(size.w * scale), svg_bucket(size.h * scale));
        let Some(img) = self
            .svgs
            .get(&layer.svg_markup, pw, ph, self.compositor, self.device, self.queue)
        else {
            return;
        };
        let color = with_alpha(self.color(&layer.color), opacity);
        let half = [size.w * 0.5, size.h * 0.5];
        let inst = QuadInstance::new(xf.t, half, xf.m, scale)
            .kind(QuadKind::Texture)
            .color(color);
        self.canvas.push(z, blend, img.id, inst);
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
            .get(&path, (self.vw as u32, self.vh as u32), self.compositor, self.device, self.queue)
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

#[cfg(test)]
mod fast_path_tests {
    use super::Affine;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    /// O delta `xf1 ∘ xf0⁻¹` leva qualquer ponto desenhado com `xf0` ao mesmo ponto com `xf1`
    /// (o que o caminho rápido aplica às instâncias em vez de refazer o layout).
    #[test]
    fn delta_maps_old_world_to_new_world() {
        let parent = Affine::IDENTITY.child([400.0, 300.0], 0.3, 1.5);
        let xf0 = parent.child([12.0, -8.0], 0.1, 1.0);
        let xf1 = parent.child([20.0, 4.0], 0.25, 1.2);
        let delta = xf1.then_apply(&xf0.inverse().expect("inversível"));
        for local in [[0.0, 0.0], [50.0, -20.0], [-7.5, 33.0]] {
            assert!(close(delta.apply(xf0.apply(local)), xf1.apply(local)));
        }
    }

    #[test]
    fn image_is_reduced_to_cover_the_output_only() {
        use super::image_target_scale;
        // 6000×4000 num 1080p: cobre com 1920×1280 (k = 0,32).
        let k = image_target_scale((6000, 4000), (1920, 1080), 16384);
        assert!((k - 0.32).abs() < 0.01, "k = {k}");
        // Imagem menor que a saída nunca é ampliada aqui.
        assert_eq!(image_target_scale((800, 600), (1920, 1080), 16384), 1.0);
        // Teto do device vence.
        assert!(image_target_scale((40000, 100), (100, 100), 16384) <= 16384.0 / 40000.0);
    }

    #[test]
    fn degenerate_affine_has_no_inverse() {
        let flat = Affine::IDENTITY.child([0.0, 0.0], 0.0, 0.0);
        assert!(flat.inverse().is_none());
    }
}
