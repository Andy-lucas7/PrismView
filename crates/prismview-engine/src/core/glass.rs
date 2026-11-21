//! Vidro óptico nativo (Fase 2): pirâmide de blur Dual Kawase, refração/dispersão cromática e
//! mapas de normais dos glifos gerados direto na GPU. Port fiel do pipeline WebGL2 do Estúdio
//! (`opticalGlassWebGL.ts` + `glassLens.ts`) para WGSL, **sem nenhuma passada de CPU por pixel**.
//!
//! Peças:
//! - `BlurPyramid` (`shaders/glass_blur.wgsl`): Dual Kawase em 1/4, 1/8, 1/16, 1/32, 1/64 da
//!   resolução da fonte. Os alvos só são recriados quando o tamanho da fonte muda (resize/troca de
//!   asset). Resultado cacheado por `(content_hash, frame_token, blur quantizado, tamanho)` em 4
//!   slots LRU; a cadeia de descida é reaproveitada entre blurs diferentes do mesmo frame.
//! - `GlyphMaps` (`shaders/glass_glyph.wgsl`): máscara do texto → blur de altura (2 passes
//!   gaussianos) → Sobel empacotado `R=nx G=ny B=inclinação A=alpha`. Só roda quando o texto muda.
//! - `GlassRenderer` (`shaders/glass.wgsl`): passe principal (placa SDF arredondada ou glifo) com
//!   refração de lente, dispersão RGB de 3 canais, véu de tint, grão, especular/Fresnel e borda.
//! - `GlassLayer`: lista de vidros de **uma saída** (monitor), análoga ao `Canvas`. Cada vidro é um
//!   marcador na ordem Z do `Canvas` (`Canvas::push_marker`), desenhado pelo callback de
//!   `Compositor2D::render_with` na posição certa entre as demais camadas 2D.
//!
//! Fluxo por frame (feito por `scene2d::SceneRenderer::render`):
//! 1. `GlassRenderer::flush` (só quando a lista foi refeita) envia as instâncias à GPU.
//! 2. `GlassRenderer::prepare` (com o `CommandEncoder` do frame, antes do passe de render) roda a
//!    pirâmide de blur e os passes de glifo que faltam (cache: normalmente zero passes).
//! 3. Dentro do passe do compositor, o marcador chama `GlassRenderer::draw`.
//!
//! O backdrop é a mídia de fundo (imagem ou frame de vídeo) mapeada em cover sobre a viewport,
//! exatamente como no Estúdio, que refrata apenas o fundo e não as demais camadas.
//!
//! Regras (PLANO_NATIVO): sem `unwrap/expect`; sem alocação por frame no caminho quente (as listas
//! são reaproveitadas; bind groups em cache; só há alocação em resize, troca de asset ou texto novo);
//! nenhum código de SO.

use std::collections::HashMap;
use std::mem;

use bytemuck::{Pod, Zeroable};

use super::compositor::TextureId;

// ───────────────────────── Constantes ─────────────────────────

/// Formato da pirâmide (sem banding no blur; filtrável e renderizável em qualquer GPU DX12).
pub const PYRAMID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Formato do mapa de normais empacotado e dos alvos intermediários de glifo.
const GLYPH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Níveis da pirâmide: 1/4, 1/8, 1/16, 1/32, 1/64 da fonte.
pub const MAX_LEVELS: usize = 5;
/// Slots de resultado em cache (alvos de nível 0 prontos para amostrar).
const RESULT_SLOTS: usize = 4;
/// Passes (slots de uniform) por frame; reiniciado em `GlassRenderer::prepare`.
const MAX_PASS_SLOTS: usize = 64;
const MIN_LEVEL_DIM: u32 = 2;

/// Abaixo disto o blur é dispensado (o shader só mistura nítido/borrado).
const MIN_BLUR_PX: f32 = 0.1;
/// `sigma = blur_px * 0.52`, a mesma conversão do WebGL2 do Estúdio.
const SIGMA_PER_BLUR_PX: f32 = 0.52;
/// Sigma (em texels da fonte) obtido por unidade de `offset` no nível mais profundo. Aproximação
/// analítica do Dual Kawase; calibrar visualmente contra o Estúdio se necessário.
const SIGMA_K: f32 = 0.55;
const MIN_OFFSET: f32 = 0.25;
/// Acima disto o Dual Kawase começa a mostrar padrão de caixa: sobe-se de nível em vez disso.
const MAX_OFFSET: f32 = 1.75;
/// Maior blur (px de fonte) alcançável com `MAX_LEVELS` níveis e `MAX_OFFSET`.
const MAX_BLUR_SRC_PX: f32 = 118.0;

const GROUP_CACHE_LIMIT: usize = 96;
const SRC_GROUP_CACHE_LIMIT: usize = 16;
const MAX_GLYPH_DIM: u32 = 4096;

// ───────────────────────── Hash ─────────────────────────

/// FNV-1a 64 bits. Para chaves de cache (não criptográfico).
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn hash_str(s: &str) -> u64 {
    hash_bytes(s.as_bytes())
}

pub fn hash_combine(a: u64, b: u64) -> u64 {
    a ^ b
        .wrapping_add(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(a << 6)
        .wrapping_add(a >> 2)
}

fn fin(v: f32, default: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        default
    }
}

/// Mistura nítido→borrado: 0 abaixo de 0,1 px, rampa até 1,5 px (igual ao `blurMix` do TS).
pub fn blur_mix(blur_px: f32) -> f32 {
    if !blur_px.is_finite() || blur_px <= MIN_BLUR_PX {
        0.0
    } else {
        (blur_px / 1.5).min(1.0)
    }
}

// ───────────────────────── Anel de parâmetros ─────────────────────────

/// Conteúdo de um slot de 32 B. Cada shader decodifica `a`/`b` à sua maneira.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PassParams {
    pub a: [f32; 4],
    pub b: [f32; 4],
}

const PARAM_SIZE: u64 = mem::size_of::<PassParams>() as u64;

/// Buffer uniform com offset dinâmico: 1 slot por passe, sem criar buffers/bind groups por frame.
/// As escritas (`queue.write_buffer`) acontecem antes do submit, por isso cada passe do frame usa
/// um slot distinto; `reset` roda uma vez por frame.
struct ParamRing {
    buffer: wgpu::Buffer,
    stride: u64,
    cursor: usize,
}

impl ParamRing {
    fn new(device: &wgpu::Device) -> Self {
        let stride = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(PARAM_SIZE);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_glass_params"),
            size: stride * MAX_PASS_SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            stride,
            cursor: 0,
        }
    }

    fn reset(&mut self) {
        self.cursor = 0;
    }

    fn remaining(&self) -> usize {
        MAX_PASS_SLOTS.saturating_sub(self.cursor)
    }

    /// Grava `params` no próximo slot e devolve o offset dinâmico. `None` se o anel acabou.
    fn push(&mut self, queue: &wgpu::Queue, params: &PassParams) -> Option<u32> {
        if self.cursor >= MAX_PASS_SLOTS {
            return None;
        }
        let offset = self.cursor as u64 * self.stride;
        queue.write_buffer(&self.buffer, offset, bytemuck::bytes_of(params));
        self.cursor += 1;
        u32::try_from(offset).ok()
    }

    fn binding(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: 0,
            size: wgpu::BufferSize::new(PARAM_SIZE),
        })
    }
}

fn dynamic_uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(PARAM_SIZE),
        },
        count: None,
    }
}

fn texture_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(vs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

/// Um passe de tela cheia (triângulo) que sobrescreve `target`.
fn record_fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    dynamic_offset: u32,
    target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[dynamic_offset]);
    pass.draw(0..3, 0..1);
}

fn linear_clamp_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("prismview_glass_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    })
}

struct Tex {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn make_target(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> Tex {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Tex {
        _texture: texture,
        view,
    }
}

// ───────────────────────── Pirâmide de blur ─────────────────────────

/// Fonte do blur: view do backdrop + identidade estável do par (textura, versão) para cache de
/// bind group.
#[derive(Clone, Copy)]
pub struct BlurSource<'a> {
    pub view: &'a wgpu::TextureView,
    pub id: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct BlurRequest {
    /// Identidade do conteúdo do backdrop (muda ao trocar de asset).
    pub content_hash: u64,
    /// 0 para conteúdo estático; contador por frame novo para vídeo/cena viva.
    pub frame_token: u64,
    /// Blur em **px da fonte** (texels do backdrop). Converter de px de tela antes.
    pub blur_px: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlurHandle {
    slot: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct CacheKey {
    content_hash: u64,
    frame_token: u64,
    blur_q: u16,
    src: (u32, u32),
}

struct ResultSlot {
    tex: Tex,
    key: Option<CacheKey>,
    stamp: u64,
}

struct Targets {
    src_size: (u32, u32),
    sizes: [(u32, u32); MAX_LEVELS],
    /// D0..D4: cadeia de descida.
    down: Vec<Tex>,
    /// U1..U4 (índice `i - 1`): cadeia de subida intermediária. U0 é o slot de resultado.
    up: Vec<Tex>,
    /// Bind groups que LEEM D_i / U_i (reaproveitados por descida e subida).
    read_down: Vec<wgpu::BindGroup>,
    read_up: Vec<wgpu::BindGroup>,
    /// Bind groups da fonte externa, por `BlurSource::id`.
    src_groups: HashMap<u64, wgpu::BindGroup>,
    slots: Vec<ResultSlot>,
    /// `(content_hash, frame_token)` da cadeia de descida e quantos níveis já estão calculados.
    down_key: Option<(u64, u64)>,
    down_depth: usize,
}

fn level_divisor(level: usize) -> u32 {
    1u32 << (level + 2)
}

fn level_size(src: (u32, u32), level: usize) -> (u32, u32) {
    let d = level_divisor(level);
    (
        src.0.div_ceil(d).max(MIN_LEVEL_DIM),
        src.1.div_ceil(d).max(MIN_LEVEL_DIM),
    )
}

fn quantize_blur(src_px: f32) -> u16 {
    let q = (src_px.clamp(0.5, MAX_BLUR_SRC_PX) * 2.0).round();
    if q.is_finite() {
        q as u16
    } else {
        1
    }
}

/// Converte o blur quantizado em (nível mais profundo, offset da subida). Escolhe o menor nível
/// capaz de atingir o sigma com `offset <= MAX_OFFSET` (menos pixels processados).
fn plan_blur(blur_q: u16) -> (usize, f32) {
    let blur_src_px = f32::from(blur_q) * 0.5;
    let sigma = blur_src_px * SIGMA_PER_BLUR_PX;
    for level in 0..MAX_LEVELS {
        let per_offset = SIGMA_K * level_divisor(level) as f32;
        let offset = sigma / per_offset;
        if offset <= MAX_OFFSET || level == MAX_LEVELS - 1 {
            return (level, offset.clamp(MIN_OFFSET, MAX_OFFSET));
        }
    }
    (MAX_LEVELS - 1, MAX_OFFSET)
}

struct BlurPyramid {
    down_first: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    targets: Option<Targets>,
    /// Muda a cada recriação dos alvos (invalida bind groups que apontam para eles).
    epoch: u32,
    clock: u64,
    /// Slots com `stamp >= frame_floor` já foram usados neste frame e não podem ser reciclados.
    frame_floor: u64,
}

impl BlurPyramid {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_glass_blur_layout"),
            entries: &[
                dynamic_uniform_entry(0),
                texture_entry(1, wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_glass_blur_pipeline_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_glass_blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glass_blur.wgsl").into()),
        });
        let make = |label: &str, fs: &str| {
            fullscreen_pipeline(
                device,
                label,
                &pipeline_layout,
                &module,
                "vs_main",
                fs,
                PYRAMID_FORMAT,
            )
        };
        Self {
            down_first: make("prismview_glass_down_first", "fs_down_first"),
            down: make("prismview_glass_down", "fs_down"),
            up: make("prismview_glass_up", "fs_up"),
            layout,
            sampler: linear_clamp_sampler(device),
            targets: None,
            epoch: 0,
            clock: 1,
            frame_floor: 1,
        }
    }

    fn begin_frame(&mut self) {
        self.frame_floor = self.clock + 1;
    }

    fn group(
        &self,
        device: &wgpu::Device,
        ring: &ParamRing,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_glass_blur_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ring.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// (Re)cria os alvos **somente** se o tamanho da fonte mudou.
    fn ensure_targets(&mut self, device: &wgpu::Device, ring: &ParamRing, src: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.src_size == src) {
            return;
        }
        let mut sizes = [(0, 0); MAX_LEVELS];
        for (level, size) in sizes.iter_mut().enumerate() {
            *size = level_size(src, level);
        }
        let down: Vec<Tex> = sizes
            .iter()
            .map(|&s| make_target(device, "prismview_glass_down", s, PYRAMID_FORMAT))
            .collect();
        let up: Vec<Tex> = sizes
            .iter()
            .skip(1)
            .map(|&s| make_target(device, "prismview_glass_up", s, PYRAMID_FORMAT))
            .collect();
        let read_down: Vec<wgpu::BindGroup> =
            down.iter().map(|t| self.group(device, ring, &t.view)).collect();
        let read_up: Vec<wgpu::BindGroup> =
            up.iter().map(|t| self.group(device, ring, &t.view)).collect();
        let slots: Vec<ResultSlot> = (0..RESULT_SLOTS)
            .map(|_| ResultSlot {
                tex: make_target(device, "prismview_glass_result", sizes[0], PYRAMID_FORMAT),
                key: None,
                stamp: 0,
            })
            .collect();
        self.epoch = self.epoch.wrapping_add(1);
        self.targets = Some(Targets {
            src_size: src,
            sizes,
            down,
            up,
            read_down,
            read_up,
            src_groups: HashMap::new(),
            slots,
            down_key: None,
            down_depth: 0,
        });
    }

    fn release(&mut self) {
        self.targets = None;
        self.epoch = self.epoch.wrapping_add(1);
    }

    fn invalidate_content(&mut self, content_hash: u64) {
        if let Some(t) = self.targets.as_mut() {
            for slot in &mut t.slots {
                if slot.key.is_some_and(|k| k.content_hash == content_hash) {
                    slot.key = None;
                    slot.stamp = 0;
                }
            }
            if t.down_key.is_some_and(|(h, _)| h == content_hash) {
                t.down_key = None;
                t.down_depth = 0;
            }
        }
    }

    fn view(&self, handle: BlurHandle) -> Option<&wgpu::TextureView> {
        self.targets
            .as_ref()
            .and_then(|t| t.slots.get(handle.slot))
            .map(|s| &s.tex.view)
    }

    /// Garante o blur de `req` para `src` e devolve o slot. Grava os passes em `encoder`.
    /// `None`: sem blur necessário, fonte inválida ou recursos do frame esgotados (o chamador
    /// amostra o backdrop nítido).
    #[allow(clippy::too_many_arguments)]
    fn blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        ring: &mut ParamRing,
        stats: &mut GlassStats,
        src: BlurSource<'_>,
        req: BlurRequest,
    ) -> Option<BlurHandle> {
        if src.width == 0
            || src.height == 0
            || !req.blur_px.is_finite()
            || req.blur_px <= MIN_BLUR_PX
        {
            return None;
        }
        self.ensure_targets(device, ring, (src.width, src.height));

        let blur_q = quantize_blur(req.blur_px);
        let key = CacheKey {
            content_hash: req.content_hash,
            frame_token: req.frame_token,
            blur_q,
            src: (src.width, src.height),
        };

        self.clock += 1;
        let now = self.clock;
        let floor = self.frame_floor;

        // Bind group da fonte externa (criado só se faltar; precisa de `self.layout`).
        let need_ext = {
            let t = self.targets.as_ref()?;
            !t.src_groups.contains_key(&src.id)
        };
        let ext_group = if need_ext {
            Some(self.group(device, ring, src.view))
        } else {
            None
        };

        let t = self.targets.as_mut()?;

        // Acerto de cache.
        if let Some(i) = t.slots.iter().position(|s| s.key == Some(key)) {
            t.slots[i].stamp = now;
            stats.blur_hits += 1;
            return Some(BlurHandle { slot: i });
        }

        // Vítima: menor `stamp` entre os slots ainda não usados neste frame.
        let victim = t
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.stamp < floor)
            .min_by_key(|(_, s)| s.stamp)
            .map(|(i, _)| i)?;

        // Cadeia de descida reaproveitada entre blurs do mesmo conteúdo/frame.
        let chain = (req.content_hash, req.frame_token);
        if t.down_key != Some(chain) {
            t.down_key = Some(chain);
            t.down_depth = 0;
        }
        let (level, offset) = plan_blur(blur_q);
        let first = t.down_depth;
        let down_passes = (level + 1).saturating_sub(first);
        if ring.remaining() < down_passes + level + 1 {
            return None;
        }

        if first == 0 {
            if let Some(group) = ext_group {
                if t.src_groups.len() >= SRC_GROUP_CACHE_LIMIT {
                    t.src_groups.clear();
                }
                t.src_groups.insert(src.id, group);
            }
        }

        // Descida: fonte → D0 (1/4, caixa 4×4) → D1 → ... → D_level (Kawase 5 taps, offset 1).
        for i in first..=level {
            let (group, tex_size, pipeline) = if i == 0 {
                (t.src_groups.get(&src.id)?, (src.width, src.height), &self.down_first)
            } else {
                (&t.read_down[i - 1], t.sizes[i - 1], &self.down)
            };
            let params = PassParams {
                a: [1.0 / tex_size.0 as f32, 1.0 / tex_size.1 as f32, 1.0, 0.0],
                b: [0.0; 4],
            };
            let dyn_off = ring.push(queue, &params)?;
            record_fullscreen(
                encoder,
                "prismview_glass_down_pass",
                pipeline,
                group,
                dyn_off,
                &t.down[i].view,
            );
            stats.blur_passes += 1;
        }
        t.down_depth = t.down_depth.max(level + 1);

        // Subida: D_level → U_level → ... → U1 → slot de resultado (U0), Kawase 8 taps.
        for i in (0..=level).rev() {
            let (group, tex_size) = if i == level {
                (&t.read_down[level], t.sizes[level])
            } else {
                (&t.read_up[i], t.sizes[i + 1])
            };
            let params = PassParams {
                a: [
                    1.0 / tex_size.0 as f32,
                    1.0 / tex_size.1 as f32,
                    offset,
                    0.0,
                ],
                b: [0.0; 4],
            };
            let dyn_off = ring.push(queue, &params)?;
            let target = if i == 0 {
                &t.slots[victim].tex.view
            } else {
                &t.up[i - 1].view
            };
            record_fullscreen(
                encoder,
                "prismview_glass_up_pass",
                &self.up,
                group,
                dyn_off,
                target,
            );
            stats.blur_passes += 1;
        }

        let slot = &mut t.slots[victim];
        slot.key = Some(key);
        slot.stamp = now;
        stats.blur_misses += 1;
        Some(BlurHandle { slot: victim })
    }
}

// ───────────────────────── Mapas de glifo ─────────────────────────

struct GlyphEntry {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    token: u64,
    bevel_q: u32,
    uid: u64,
    used: u64,
}

struct GlyphMaps {
    gauss: wgpu::RenderPipeline,
    pack: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    entries: HashMap<String, GlyphEntry>,
    serial: u64,
    next_uid: u64,
}

impl GlyphMaps {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_glass_glyph_layout"),
            entries: &[
                dynamic_uniform_entry(0),
                texture_entry(1, wgpu::ShaderStages::FRAGMENT),
                texture_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_glass_glyph_pipeline_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_glass_glyph"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glass_glyph.wgsl").into()),
        });
        Self {
            gauss: fullscreen_pipeline(
                device,
                "prismview_glass_glyph_gauss",
                &pipeline_layout,
                &module,
                "vs_full",
                "fs_gauss",
                GLYPH_FORMAT,
            ),
            pack: fullscreen_pipeline(
                device,
                "prismview_glass_glyph_pack",
                &pipeline_layout,
                &module,
                "vs_full",
                "fs_pack",
                GLYPH_FORMAT,
            ),
            layout,
            entries: HashMap::new(),
            serial: 0,
            next_uid: 1,
        }
    }

    fn begin_build(&mut self) {
        self.serial += 1;
    }

    /// Libera mapas que a lista atual não referencia (chamado após o primeiro `prepare`).
    fn sweep(&mut self) {
        let serial = self.serial;
        self.entries.retain(|_, e| e.used == serial);
    }

    fn group(
        &self,
        device: &wgpu::Device,
        ring: &ParamRing,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_glass_glyph_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ring.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(b),
                },
            ],
        })
    }

    fn view(&self, key: &str) -> Option<(u64, &wgpu::TextureView)> {
        self.entries.get(key).map(|e| (e.uid, &e.view))
    }

    /// Garante o mapa de normais de `key` para este `token`. Gera: máscara → altura (blur H, V) →
    /// Sobel empacotado, tudo na GPU. `false` se faltou slot de parâmetros neste frame (tenta de
    /// novo no próximo) ou o tamanho é inválido.
    #[allow(clippy::too_many_arguments)]
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        ring: &mut ParamRing,
        stats: &mut GlassStats,
        source: &GlyphSource,
        mask: &wgpu::TextureView,
    ) -> bool {
        let size = (
            source.size.0.clamp(1, MAX_GLYPH_DIM),
            source.size.1.clamp(1, MAX_GLYPH_DIM),
        );
        // Mesma conversão do TS: bevel = clamp(round(bevelPx * 0.48), 2, 32); σ = max(2, bevel * 0.55).
        let bevel = (fin(source.bevel_px, 0.0) * 0.48).round().clamp(2.0, 32.0);
        let bevel_q = bevel as u32;

        if let Some(e) = self.entries.get_mut(&source.key) {
            if e.token == source.token && e.size == size && e.bevel_q == bevel_q {
                e.used = self.serial;
                return true;
            }
        }
        if ring.remaining() < 3 {
            return false;
        }

        let sigma = (bevel * 0.55).max(2.0);
        let radius = (sigma * 3.0).ceil().clamp(1.0, 64.0);
        let region = [size.0 as f32, size.1 as f32, 0.0, 0.0];

        let tmp = make_target(device, "prismview_glyph_tmp", size, GLYPH_FORMAT);
        let height = make_target(device, "prismview_glyph_height", size, GLYPH_FORMAT);
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prismview_glyph_normals"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GLYPH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let out_view = out.create_view(&wgpu::TextureViewDescriptor::default());

        // Passe 1: blur horizontal máscara → tmp. Passe 2: vertical tmp → altura.
        let g_h = self.group(device, ring, mask, mask);
        let g_v = self.group(device, ring, &tmp.view, &tmp.view);
        let g_pack = self.group(device, ring, mask, &height.view);

        let params_h = PassParams {
            a: [1.0, 0.0, sigma, radius],
            b: region,
        };
        let params_v = PassParams {
            a: [0.0, 1.0, sigma, radius],
            b: region,
        };
        let params_pack = PassParams {
            a: [0.0; 4],
            b: region,
        };
        let (Some(off_h), Some(off_v), Some(off_pack)) = (
            ring.push(queue, &params_h),
            ring.push(queue, &params_v),
            ring.push(queue, &params_pack),
        ) else {
            return false;
        };
        record_fullscreen(encoder, "prismview_glyph_blur_h", &self.gauss, &g_h, off_h, &tmp.view);
        record_fullscreen(encoder, "prismview_glyph_blur_v", &self.gauss, &g_v, off_v, &height.view);
        record_fullscreen(encoder, "prismview_glyph_pack", &self.pack, &g_pack, off_pack, &out_view);
        stats.glyph_builds += 1;

        let uid = self.next_uid;
        self.next_uid += 1;
        self.entries.insert(
            source.key.clone(),
            GlyphEntry {
                _texture: out,
                view: out_view,
                size,
                token: source.token,
                bevel_q,
                uid,
                used: self.serial,
            },
        );
        true
    }
}

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
    /// Chave do slot de texto (a mesma do `TextSystem`).
    pub key: String,
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
struct GlassInstance {
    xf: [f32; 4],
    geo: [f32; 4],
    bg: [f32; 4],
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
    p3: [f32; 4],
    tint: [f32; 4],
    pigment: [f32; 4],
}

impl GlassInstance {
    fn from_draw(d: &GlassDraw) -> Self {
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
            tint: [p.tint[0], p.tint[1], p.tint[2], 1.0],
            pigment: p.pigment,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlassGlobals {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// Chave exata do bind group de um vidro (textura nítida + blur + mapa de glifo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GroupKey {
    tex_raw: u32,
    tex_version: u64,
    /// `(epoch << 8) | (slot + 1)`; 0 = sem blur.
    blur: u64,
    /// uid do mapa de glifo; 0 = nenhum.
    glyph: u64,
}

const INITIAL_CAPACITY: usize = 16;

/// Lista de vidros de uma saída (monitor). Criada por `GlassRenderer::create_layer`.
pub struct GlassLayer {
    viewport: (u32, u32),
    globals: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    bind_group: wgpu::BindGroup,
    draws: Vec<GlassDraw>,
    groups: Vec<Option<GroupKey>>,
    gpu: Vec<GlassInstance>,
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

// ───────────────────────── GlassRenderer ─────────────────────────

pub struct GlassRenderer {
    ring: ParamRing,
    blur: BlurPyramid,
    glyphs: GlyphMaps,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    layer_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    groups: HashMap<GroupKey, wgpu::BindGroup>,
    groups_epoch: u32,
    stats: GlassStats,
}

impl GlassRenderer {
    /// `format` = formato da swapchain (o mesmo do `Compositor2D`).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_glass_layer_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: both,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: both,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let frag = wgpu::ShaderStages::FRAGMENT;
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_glass_tex_layout"),
            entries: &[
                texture_entry(0, frag),
                texture_entry(1, frag),
                texture_entry(2, frag),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_glass_pipeline_layout"),
            bind_group_layouts: &[&layer_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_glass"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glass.wgsl").into()),
        });
        // Alpha pré-multiplicado, mesmo blend `Normal` do compositor.
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prismview_glass_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            ring: ParamRing::new(device),
            blur: BlurPyramid::new(device),
            glyphs: GlyphMaps::new(device),
            sampler: linear_clamp_sampler(device),
            pipeline,
            layer_layout,
            tex_layout,
            groups: HashMap::new(),
            groups_epoch: 0,
            stats: GlassStats::default(),
        }
    }

    pub fn stats(&self) -> GlassStats {
        self.stats
    }

    /// Hibernação de VRAM (Fase 5): solta pirâmide, mapas de glifo e bind groups. Tudo é
    /// recriado sob demanda no próximo `prepare`.
    pub fn release(&mut self) {
        self.blur.release();
        self.glyphs.entries.clear();
        self.groups.clear();
    }

    /// Invalida resultados de blur de um backdrop que foi trocado/liberado.
    pub fn invalidate_backdrop(&mut self, content_hash: u64) {
        self.blur.invalidate_content(content_hash);
    }

    fn alloc_instances(
        &self,
        device: &wgpu::Device,
        globals: &wgpu::Buffer,
        capacity: usize,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_glass_instances"),
            size: (capacity * mem::size_of::<GlassInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_glass_layer"),
            layout: &self.layer_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: instances.as_entire_binding(),
                },
            ],
        });
        (instances, bind_group)
    }

    /// Lista de vidros para uma saída (monitor).
    pub fn create_layer(&self, device: &wgpu::Device) -> GlassLayer {
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_glass_globals"),
            size: mem::size_of::<GlassGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (instances, bind_group) = self.alloc_instances(device, &globals, INITIAL_CAPACITY);
        GlassLayer {
            viewport: (1, 1),
            globals,
            instances,
            capacity: INITIAL_CAPACITY,
            bind_group,
            draws: Vec::with_capacity(INITIAL_CAPACITY),
            groups: Vec::with_capacity(INITIAL_CAPACITY),
            gpu: Vec::with_capacity(INITIAL_CAPACITY),
        }
    }

    /// Marca o início de uma nova construção de lista (permite varrer mapas de glifo órfãos).
    pub fn begin_build(&mut self) {
        self.glyphs.begin_build();
    }

    /// Envia as instâncias de `layer` à GPU. Chamar só quando a lista foi refeita.
    pub fn flush(&self, layer: &mut GlassLayer, device: &wgpu::Device, queue: &wgpu::Queue) {
        layer.gpu.clear();
        for draw in &layer.draws {
            layer.gpu.push(GlassInstance::from_draw(draw));
        }
        if layer.gpu.len() > layer.capacity {
            let capacity = layer.gpu.len().next_power_of_two();
            let (buffer, bind_group) = self.alloc_instances(device, &layer.globals, capacity);
            layer.instances = buffer;
            layer.bind_group = bind_group;
            layer.capacity = capacity;
        }
        if !layer.gpu.is_empty() {
            queue.write_buffer(&layer.instances, 0, bytemuck::cast_slice(&layer.gpu));
        }
        let globals = GlassGlobals {
            viewport: [layer.viewport.0 as f32, layer.viewport.1 as f32],
            _pad: [0.0; 2],
        };
        queue.write_buffer(&layer.globals, 0, bytemuck::bytes_of(&globals));
    }

    /// Roda o que falta de GPU para os vidros de `layer` neste frame: pirâmide de blur (cache por
    /// hash + `frame_token`) e mapas de glifo. Chamar com o `CommandEncoder` do frame, **antes** de
    /// abrir o passe de render.
    ///
    /// `backdrop(id)` devolve a view do backdrop e a versão da associação (ver
    /// `Compositor2D::texture_version`); `mask(key)` devolve a view da máscara de texto.
    /// `frame_token` só importa para backdrops `live`.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare<'v, B, M>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer: &mut GlassLayer,
        frame_token: u64,
        backdrop: B,
        mask: M,
    ) where
        B: Fn(TextureId) -> Option<(&'v wgpu::TextureView, u64)>,
        M: Fn(&str) -> Option<&'v wgpu::TextureView>,
    {
        self.ring.reset();
        self.blur.begin_frame();
        if self.groups_epoch != self.blur.epoch {
            self.groups.clear();
            self.groups_epoch = self.blur.epoch;
        }

        for index in 0..layer.draws.len() {
            let draw = &layer.draws[index];
            layer.groups[index] = None;

            let Some((sharp, version)) = backdrop(draw.backdrop.texture) else {
                continue;
            };
            let tex_raw = draw.backdrop.texture.raw();
            let source_id = hash_combine(u64::from(tex_raw), version);

            // Blur (px de tela → px da fonte).
            let mut blur_handle = None;
            if draw.params.blur_px > MIN_BLUR_PX {
                let token = if draw.backdrop.live { frame_token } else { 0 };
                blur_handle = self.blur.blur(
                    device,
                    queue,
                    encoder,
                    &mut self.ring,
                    &mut self.stats,
                    BlurSource {
                        view: sharp,
                        id: source_id,
                        width: draw.backdrop.size.0,
                        height: draw.backdrop.size.1,
                    },
                    BlurRequest {
                        content_hash: draw.backdrop.content_hash,
                        frame_token: token,
                        blur_px: draw.params.blur_px * draw.backdrop.src_per_screen_px,
                    },
                );
            }

            // Mapa de normais do glifo.
            let mut glyph_uid = 0u64;
            if draw.mode == GlassMode::Glyph {
                let Some(source) = draw.glyph.as_ref() else {
                    continue;
                };
                let Some(mask_view) = mask(&source.key) else {
                    continue;
                };
                if !self.glyphs.ensure(
                    device,
                    queue,
                    encoder,
                    &mut self.ring,
                    &mut self.stats,
                    source,
                    mask_view,
                ) {
                    continue;
                }
                let Some((uid, _)) = self.glyphs.view(&source.key) else {
                    continue;
                };
                glyph_uid = uid;
            }

            let blur_key = match (blur_handle, self.blur.targets.as_ref()) {
                (Some(h), Some(_)) => (u64::from(self.blur.epoch) << 8) | (h.slot as u64 + 1),
                _ => 0,
            };
            let key = GroupKey {
                tex_raw,
                tex_version: version,
                blur: blur_key,
                glyph: glyph_uid,
            };
            if !self.groups.contains_key(&key) {
                if self.groups.len() >= GROUP_CACHE_LIMIT {
                    self.groups.clear();
                }
                let blur_view = blur_handle.and_then(|h| self.blur.view(h)).unwrap_or(sharp);
                let glyph_view = if glyph_uid != 0 {
                    draw.glyph
                        .as_ref()
                        .and_then(|g| self.glyphs.view(&g.key))
                        .map_or(sharp, |(_, v)| v)
                } else {
                    sharp
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("prismview_glass_textures"),
                    layout: &self.tex_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(sharp),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(blur_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(glyph_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                self.groups.insert(key, group);
            }
            layer.groups[index] = Some(key);
        }

        self.glyphs.sweep();
    }

    /// Desenha o vidro `marker` dentro do passe de render do compositor.
    pub fn draw(&self, layer: &GlassLayer, pass: &mut wgpu::RenderPass<'_>, marker: u32) {
        let index = marker as usize;
        let Some(Some(key)) = layer.groups.get(index) else {
            return;
        };
        let Some(group) = self.groups.get(key) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &layer.bind_group, &[]);
        pass.set_bind_group(1, group, &[]);
        pass.draw(0..4, marker..marker + 1);
    }
}

// ───────────────────────── Testes (funções puras) ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_sizes_halve_and_clamp() {
        assert_eq!(level_size((3840, 2160), 0), (960, 540));
        assert_eq!(level_size((3840, 2160), 1), (480, 270));
        assert_eq!(level_size((8, 8), 4), (MIN_LEVEL_DIM, MIN_LEVEL_DIM));
    }

    #[test]
    fn plan_is_monotonic_in_blur() {
        let mut last_level = 0;
        for px in [1.0f32, 4.0, 10.0, 26.0, 60.0, 118.0] {
            let (level, offset) = plan_blur(quantize_blur(px));
            assert!(level >= last_level);
            assert!((MIN_OFFSET..=MAX_OFFSET).contains(&offset));
            last_level = level;
        }
    }

    #[test]
    fn blur_mix_ramps() {
        assert_eq!(blur_mix(0.0), 0.0);
        assert!((blur_mix(0.75) - 0.5).abs() < 1e-6);
        assert_eq!(blur_mix(40.0), 1.0);
        assert_eq!(blur_mix(f32::NAN), 0.0);
    }
}
