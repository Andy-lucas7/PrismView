//! API pública do vidro: parâmetros, backdrop e a lista de vidros por saída.

#[allow(unused_imports)]
use super::*;

// ───────────────────────── API pública: parâmetros e lista ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GlassMode {
    /// Placa retangular arredondada (SDF analítico).
    #[default]
    Plate,
    /// Texto: normais derivadas da máscara dos glifos.
    Glyph,
}

/// Backdrop do vidro: mídia de fundo cobrindo a viewport inteira.
#[derive(Clone, Copy, Debug)]
pub struct GlassBackdrop {
    /// Textura no `Compositor2D` (imagem própria ou frame de vídeo registrado com `bind_view`).
    pub texture: TextureId,
    /// Tamanho da textura em texels.
    pub size: (u32, u32),
    /// Região da textura que cobre a viewport (cover): `[u0, v0, u1, v1]`.
    pub uv_rect: [f32; 4],
    /// Texels da fonte por px de tela (1 / fator de cover), para converter o blur.
    pub src_per_screen_px: f32,
    /// Identidade do conteúdo (hash do caminho/asset ou do vídeo).
    pub content_hash: u64,
    /// `true` para vídeo/cena viva: o blur é refeito a cada `frame_token` novo.
    pub live: bool,
}

/// Parâmetros ópticos já em **px físicos** (escala de UI aplicada). Equivalentes diretos de
/// `OpticalGlassRenderParams` do TS.
#[derive(Clone, Copy, Debug)]
pub struct GlassParams {
    pub blur_px: f32,
    pub refraction_px: f32,
    pub bevel_px: f32,
    pub radius_px: f32,
    pub chroma_px: f32,
    pub frost: f32,
    pub tint: [f32; 3],
    pub noise: f32,
    pub specular: f32,
    pub border: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub opacity: f32,
    /// Direção da luz em graus no espaço top-down (225° ≈ luz no canto superior esquerdo).
    pub light_angle_deg: f32,
    /// Pigmento do texto (rgb + alpha), só no modo glifo.
    pub pigment: [f32; 4],
}

impl Default for GlassParams {
    fn default() -> Self {
        Self {
            blur_px: 0.0,
            refraction_px: 0.0,
            bevel_px: 12.0,
            radius_px: 0.0,
            chroma_px: 0.0,
            frost: 0.0,
            tint: [1.0; 3],
            noise: 0.0,
            specular: 0.0,
            border: 0.0,
            brightness: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            opacity: 1.0,
            light_angle_deg: 229.0,
            pigment: [1.0, 1.0, 1.0, 0.0],
        }
    }
}

/// Máscara do texto para o modo glifo.
#[derive(Clone, Debug)]
pub struct GlyphSource {
    /// Chave do slot de texto (a mesma do `TextSystem`), compartilhada sem alocar por quadro.
    pub key: Arc<str>,
    /// Muda quando o texto/fonte/tamanho/textura muda (invalida o mapa de normais).
    pub token: u64,
    /// Região útil da máscara em px (tamanho do quad do texto).
    pub size: (u32, u32),
    pub bevel_px: f32,
}

#[derive(Clone, Debug)]
pub struct GlassDraw {
    pub mode: GlassMode,
    pub backdrop: GlassBackdrop,
    pub params: GlassParams,
    /// Centro em px de tela e meio tamanho local (px).
    pub center: [f32; 2],
    pub half: [f32; 2],
    /// Base afim `[m00, m10, m01, m11]` da camada (rotação/escala).
    pub basis: [f32; 4],
    pub glyph: Option<GlyphSource>,
}

/// Instância GPU (144 B). Layout idêntico ao `struct Inst` de `glass.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct GlassInstance {
    pub(super) xf: [f32; 4],
    pub(super) geo: [f32; 4],
    pub(super) bg: [f32; 4],
    pub(super) p0: [f32; 4],
    pub(super) p1: [f32; 4],
    pub(super) p2: [f32; 4],
    pub(super) p3: [f32; 4],
    pub(super) tint: [f32; 4],
    pub(super) pigment: [f32; 4],
}

impl GlassInstance {
    pub(super) fn from_draw(d: &GlassDraw) -> Self {
        let p = &d.params;
        let light = fin(p.light_angle_deg, 229.0).to_radians();
        let glyph = d.mode == GlassMode::Glyph;
        Self {
            xf: d.basis.map(|v| fin(v, 0.0)),
            geo: [
                fin(d.center[0], 0.0),
                fin(d.center[1], 0.0),
                fin(d.half[0], 0.0).max(0.0),
                fin(d.half[1], 0.0).max(0.0),
            ],
            bg: d.backdrop.uv_rect,
            p0: [
                blur_mix(p.blur_px),
                fin(p.refraction_px, 0.0),
                fin(p.bevel_px, 12.0),
                fin(p.radius_px, 0.0).max(0.0),
            ],
            p1: [
                fin(p.chroma_px, 0.0).max(0.0),
                fin(p.frost, 0.0),
                fin(p.noise, 0.0),
                fin(p.specular, 0.0),
            ],
            p2: [
                fin(p.border, 0.0),
                fin(p.brightness, 1.0),
                fin(p.contrast, 1.0),
                fin(p.saturation, 1.0),
            ],
            p3: [
                fin(p.opacity, 1.0).clamp(0.0, 1.0),
                light.cos(),
                light.sin(),
                if glyph { 1.0 } else { 0.0 },
            ],
            tint: [
                fin(p.tint[0], 1.0).clamp(0.0, 1.0),
                fin(p.tint[1], 1.0).clamp(0.0, 1.0),
                fin(p.tint[2], 1.0).clamp(0.0, 1.0),
                1.0,
            ],
            pigment: [
                fin(p.pigment[0], 1.0).clamp(0.0, 1.0),
                fin(p.pigment[1], 1.0).clamp(0.0, 1.0),
                fin(p.pigment[2], 1.0).clamp(0.0, 1.0),
                fin(p.pigment[3], 0.0).clamp(0.0, 1.0),
            ],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GlassGlobals {
    pub(super) viewport: [f32; 2],
    pub(super) _pad: [f32; 2],
}

/// Chave exata do bind group de um vidro (textura nítida + blur + mapa de glifo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct GroupKey {
    pub(super) tex_raw: u32,
    pub(super) tex_version: u64,
    /// `(epoch << 8) | (slot + 1)`; 0 = sem blur.
    pub(super) blur: u64,
    /// uid do mapa de glifo; 0 = nenhum.
    pub(super) glyph: u64,
}

pub(super) const INITIAL_CAPACITY: usize = 16;

/// Lista de vidros de uma saída (monitor). Criada por `GlassRenderer::create_layer`.
pub struct GlassLayer {
    pub(super) viewport: (u32, u32),
    pub(super) globals: wgpu::Buffer,
    pub(super) instances: wgpu::Buffer,
    pub(super) capacity: usize,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) draws: Vec<GlassDraw>,
    pub(super) groups: Vec<Option<GroupKey>>,
    pub(super) gpu: Vec<GlassInstance>,
}

impl GlassLayer {
    /// Inicia uma nova lista para um alvo de `width`×`height` px.
    pub fn begin(&mut self, width: u32, height: u32) {
        self.viewport = (width.max(1), height.max(1));
        self.draws.clear();
        self.groups.clear();
    }

    /// Adiciona um vidro e devolve o id do marcador (`Canvas::push_marker`).
    pub fn push(&mut self, draw: GlassDraw) -> u32 {
        let id = u32::try_from(self.draws.len()).unwrap_or(u32::MAX - 1);
        self.draws.push(draw);
        self.groups.push(None);
        id
    }

    pub fn len(&self) -> usize {
        self.draws.len()
    }

    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }
}

/// Contadores de depuração/aceite (blur em cache = `blur_passes` parado em frames estáticos).
#[derive(Clone, Copy, Debug, Default)]
pub struct GlassStats {
    pub blur_hits: u64,
    pub blur_misses: u64,
    pub blur_passes: u64,
    pub glyph_builds: u64,
}
