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
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

use super::compositor::TextureId;

// ───────────────────────── Constantes ─────────────────────────

/// Formato da pirâmide (4 B/px; o shader opera em [0, 1] na codificação da tela, sem HDR).
pub const PYRAMID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Formato do mapa de normais empacotado e dos alvos intermediários de glifo.
const GLYPH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Níveis da pirâmide: base ≤ 1080p, 1/2, 1/4, 1/8, 1/16 (paridade 1:1 com `opticalGlassWebGL.ts`).
pub const MAX_LEVELS: usize = 5;
/// Slots de resultado em cache (alvos de saída prontos para amostrar).
const RESULT_SLOTS: usize = 4;
/// Passes (slots de uniform) por frame; reiniciado em `GlassRenderer::prepare`.
const MAX_PASS_SLOTS: usize = 64;

/// Abaixo disto o blur é dispensado (o shader só mistura nítido/borrado).
const MIN_BLUR_PX: f32 = 0.1;
/// `sigma = blur_px * 0.52`, a mesma conversão do WebGL2 do Estúdio.
const SIGMA_PER_BLUR_PX: f32 = 0.52;
/// Maior blur (px de fonte) quantizado na pirâmide (cobre 80 px em 4K).
const MAX_BLUR_SRC_PX: f32 = 240.0;

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

mod blur;
#[allow(unused_imports)]
use blur::*;

mod glyph;
#[allow(unused_imports)]
use glyph::*;

mod groups;
#[allow(unused_imports)]
pub use groups::*;

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
    /// `false` (modo Eco): sem pirâmide de blur; o vidro refrata o backdrop nítido.
    blur_enabled: bool,
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/glass.wgsl").into()),
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
            blur_enabled: true,
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

    /// Liga/desliga a pirâmide de blur (PowerGovernor). `true` se mudou (refazer o quadro).
    pub fn set_blur_enabled(&mut self, enabled: bool) -> bool {
        if self.blur_enabled == enabled {
            return false;
        }
        self.blur_enabled = enabled;
        if !enabled {
            self.blur.release();
        }
        true
    }

    /// Qualidade atual (fração da resolução da saída): também dimensiona a captura do fundo.
    pub fn resolution_scale(&self) -> f32 {
        self.blur.res_scale
    }

    /// Qualidade dos reflexos: fração da resolução da saída na base da pirâmide (1,0 = cheia).
    /// `true` se mudou (os alvos são refeitos no próximo quadro; refazer o quadro).
    pub fn set_resolution_scale(&mut self, scale: f32) -> bool {
        let scale = if scale.is_finite() { scale.clamp(0.1, 1.0) } else { 1.0 };
        if (self.blur.res_scale - scale).abs() < 1e-4 {
            return false;
        }
        self.blur.res_scale = scale;
        self.blur.release();
        true
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
            base_gpu: Vec::with_capacity(INITIAL_CAPACITY),
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
        layer.base_gpu.clear();
        layer.base_gpu.extend_from_slice(&layer.gpu);
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

    /// Inicia um frame de preparação de vidros (zera o anel de uniforms 1× por encoder).
    pub fn begin_prepare(&mut self) {
        self.ring.reset();
        self.blur.begin_frame();
        if self.groups_epoch != self.blur.epoch {
            self.groups.clear();
            self.groups_epoch = self.blur.epoch;
        }
    }

    /// Encerra o frame de preparação de vidros (varre mapas de glifo não usados).
    pub fn finish_prepare(&mut self) {
        self.glyphs.sweep();
    }

    /// Prepara apenas os vidros indicados em `markers` contra o `backdrop` atual. Permite múltiplas
    /// etapas progressivas no mesmo `CommandEncoder` (ex.: Vidro A → 3D → Vidro B).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_markers<'v, B, M>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer: &mut GlassLayer,
        markers: &[u32],
        frame_token: u64,
        backdrop: B,
        mask: M,
    ) where
        B: Fn(TextureId) -> Option<(&'v wgpu::TextureView, u64)>,
        M: Fn(&str) -> Option<&'v wgpu::TextureView>,
    {
        self.blur.begin_frame();
        if self.groups_epoch != self.blur.epoch {
            self.groups.clear();
            self.groups_epoch = self.blur.epoch;
        }

        for &marker in markers {
            let index = marker as usize;
            if index >= layer.draws.len() {
                continue;
            }
            self.prepare_one(device, queue, encoder, layer, index, frame_token, &backdrop, &mask);
        }
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
        self.begin_prepare();
        for index in 0..layer.draws.len() {
            self.prepare_one(device, queue, encoder, layer, index, frame_token, &backdrop, &mask);
        }
        self.finish_prepare();
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_one<'v, B, M>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer: &mut GlassLayer,
        index: usize,
        frame_token: u64,
        backdrop: &B,
        mask: &M,
    ) where
        B: Fn(TextureId) -> Option<(&'v wgpu::TextureView, u64)>,
        M: Fn(&str) -> Option<&'v wgpu::TextureView>,
    {
        let draw = &layer.draws[index];
        layer.groups[index] = None;

        let Some((sharp, version)) = backdrop(draw.backdrop.texture) else {
            return;
        };
        let tex_raw = draw.backdrop.texture.raw();
        let source_id = hash_combine(u64::from(tex_raw), version);

        // Blur (px de tela → px da fonte).
        let mut blur_handle = None;
        if self.blur_enabled && draw.params.blur_px > MIN_BLUR_PX {
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
                    blur_px: (draw.params.blur_px * draw.backdrop.src_per_screen_px).max(0.5),
                    opaque: draw.backdrop.opaque,
                },
            );
        }

        // Mapa de normais do glifo.
        let mut glyph_uid = 0u64;
        if draw.mode == GlassMode::Glyph {
            let Some(source) = draw.glyph.as_ref() else {
                return;
            };
            let Some(mask_view) = mask(&source.key) else {
                return;
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
                return;
            }
            let Some((uid, _)) = self.glyphs.view(&source.key) else {
                return;
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
    fn mip_sizes_follow_quality_and_halve() {
        // Max: base na resolução da saída.
        let full = mip_sizes((1920, 1080), 1.0);
        assert_eq!(full[0], (1920, 1080));
        assert_eq!(full[1], (960, 540));
        // Medium (0,5) em 4K.
        let sizes = mip_sizes((3840, 2160), 0.5);
        assert_eq!(sizes[0], (1920, 1080));
        assert_eq!(sizes[1], (960, 540));
        assert_eq!(sizes[4], (120, 68));

        let tiny = mip_sizes((16, 16), 0.35);
        assert_eq!(tiny[0], (16, 16));
    }

    #[test]
    fn plan_is_monotonic_in_blur() {
        let mut last_level = 0;
        for px in [1.0f32, 4.0, 10.0, 26.0, 60.0, 160.0] {
            let (level, sigma) = plan_blur(quantize_blur(px), 1920, 1080);
            assert!(level >= last_level);
            assert!((0.45..=4.2).contains(&sigma));
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
