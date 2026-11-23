//! Compositor 2D (Fase 1): camadas ordenadas por Z, transformação afim, opacidade, preenchimento
//! sólido/gradiente, texturas, máscaras e bordas arredondadas por SDF (`shaders/compositor2d.wgsl`).
//!
//! Divisão de responsabilidades:
//! - `Compositor2D` (1 por `GpuContext`): pipelines (um por modo de blend), sampler, layouts e o
//!   registro de texturas (`TextureId` → bind group). Nada aqui depende de uma superfície.
//! - `Canvas` (1 por monitor/saída): lista de desenho ordenada, storage buffer de instâncias e o
//!   uniform de viewport. A lista só é refeita quando a cena muda (`built_*`); frames idênticos
//!   apenas regravam o passe com a mesma lista.
//!
//! Regras (PLANO_NATIVO): zero alocação por frame (vetores pré-alocados + `.clear()`; o buffer de
//! instâncias só cresce quando a cena passa a ter mais elementos, nunca por frame); sem
//! `unwrap/expect`; nenhum código de SO.

use std::mem;

use bytemuck::{Pod, Zeroable};

use super::glass::GlassLayer;

// ───────────────────────── Tipos públicos ─────────────────────────

/// Tipos de quad (`mode.x` no shader).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QuadKind {
    /// Preenchimento sólido ou gradiente.
    Solid = 0,
    /// Textura pré-multiplicada × tint × opacidade (imagens).
    Texture = 1,
    /// Preenchimento (sólido/gradiente) recortado pelo alpha da textura (texto, ícones).
    Mask = 2,
    /// Sombra suave baseada em SDF.
    Shadow = 3,
    /// Textura opaca (frame de vídeo BGRA sem alpha garantido): amostra com a = 1.
    Opaque = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum GradientKind {
    None = 0,
    Linear = 1,
    Radial = 2,
    Sweep = 3,
}

/// Modos de blend suportados em hardware (alpha pré-multiplicado). Os demais modos do schema
/// (Overlay, SoftLight, Hue...) exigem ler o backdrop e entram com o vidro/pós-processo (Fase 2);
/// até lá caem em `Normal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlendKind {
    Normal = 0,
    Plus = 1,
    Multiply = 2,
    Screen = 3,
}

impl BlendKind {
    const COUNT: usize = 4;
    const ALL: [Self; Self::COUNT] = [Self::Normal, Self::Plus, Self::Multiply, Self::Screen];

    fn state(self) -> wgpu::BlendState {
        use wgpu::BlendFactor as F;
        let alpha = wgpu::BlendComponent {
            src_factor: F::One,
            dst_factor: F::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let color = match self {
            Self::Normal => alpha,
            Self::Plus => wgpu::BlendComponent {
                src_factor: F::One,
                dst_factor: F::One,
                operation: wgpu::BlendOperation::Add,
            },
            Self::Multiply => wgpu::BlendComponent {
                src_factor: F::Dst,
                dst_factor: F::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            Self::Screen => wgpu::BlendComponent {
                src_factor: F::One,
                dst_factor: F::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
        };
        wgpu::BlendState { color, alpha }
    }
}

/// Handle de textura registrada no `Compositor2D`. `TextureId::WHITE` é 1×1 branco (usado pelos
/// quads sem textura).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(u32);

impl TextureId {
    pub const WHITE: Self = Self(0);

    /// Índice bruto (para chaves de cache de outros módulos).
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// Instância de um quad (160 bytes). Layout idêntico ao `struct Inst` do WGSL.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct QuadInstance {
    pub center_half: [f32; 4],
    pub basis: [f32; 4],
    pub color: [f32; 4],
    pub color2: [f32; 4],
    pub border: [f32; 4],
    pub uv_rect: [f32; 4],
    pub clip: [f32; 4],
    pub shape: [f32; 4],
    pub extra: [f32; 4],
    pub mode: [u32; 4],
}

impl QuadInstance {
    /// Quad centrado em `center` (px de tela) com meio tamanho local `half`, transformado por
    /// `basis` = [m00, m10, m01, m11] (colunas X e Y). `scale` é o fator de escala uniforme da
    /// matriz (usado para manter ~1 px de antialiasing na tela).
    pub fn new(center: [f32; 2], half: [f32; 2], basis: [f32; 4], scale: f32) -> Self {
        let aa = 1.0 / scale.max(1e-3);
        Self {
            center_half: [center[0], center[1], half[0].max(0.0), half[1].max(0.0)],
            basis,
            color: [1.0, 1.0, 1.0, 1.0],
            color2: [1.0, 1.0, 1.0, 1.0],
            border: [0.0; 4],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            clip: [0.0, 0.0, -1.0, -1.0],
            // raio, borda, margem do quad (1 px de AA), raio do clip
            shape: [0.0, 0.0, aa, 0.0],
            // dir do gradiente (x, y), AA local, blur local
            extra: [1.0, 0.0, aa, 0.0],
            mode: [QuadKind::Solid as u32, GradientKind::None as u32, 0, 0],
        }
    }

    #[must_use]
    pub fn kind(mut self, kind: QuadKind) -> Self {
        self.mode[0] = kind as u32;
        self
    }

    /// Cor de preenchimento / tint, alpha reto (já com a opacidade aplicada).
    #[must_use]
    pub fn color(mut self, rgba: [f32; 4]) -> Self {
        self.color = rgba;
        self
    }

    /// Gradiente de `color` até `color2`. `dir` = direção local unitária (linear) ou ângulo
    /// de partida (sweep).
    #[must_use]
    pub fn gradient(mut self, kind: GradientKind, color2: [f32; 4], dir: [f32; 2]) -> Self {
        self.mode[1] = kind as u32;
        self.color2 = color2;
        self.extra[0] = dir[0];
        self.extra[1] = dir[1];
        self
    }

    /// Raio dos cantos em px locais (limitado ao menor meio tamanho no shader).
    #[must_use]
    pub fn radius(mut self, radius: f32) -> Self {
        self.shape[0] = radius.max(0.0);
        self
    }

    /// Borda interna de `width` px locais.
    #[must_use]
    pub fn border(mut self, width: f32, rgba: [f32; 4]) -> Self {
        self.shape[1] = width.max(0.0);
        self.border = rgba;
        self
    }

    /// Região da textura (u0, v0, u1, v1).
    #[must_use]
    pub fn uv(mut self, uv_rect: [f32; 4]) -> Self {
        self.uv_rect = uv_rect;
        self
    }

    /// Máscara retangular arredondada em espaço de tela: centro, meio tamanho e raio (px).
    #[must_use]
    pub fn clip(mut self, center: [f32; 2], half: [f32; 2], radius: f32) -> Self {
        self.clip = [center[0], center[1], half[0].max(0.0), half[1].max(0.0)];
        self.shape[3] = radius.max(0.0);
        self
    }

    /// Sombra: meia-largura da transição em px locais; amplia o quad para caber o halo.
    #[must_use]
    pub fn shadow(mut self, blur_local: f32) -> Self {
        let blur = blur_local.max(0.0);
        self.mode[0] = QuadKind::Shadow as u32;
        self.extra[3] = blur;
        self.shape[2] = self.extra[2] + blur;
        self
    }
}

// ───────────────────────── Compositor2D ─────────────────────────

struct TextureEntry {
    bind_group: Option<wgpu::BindGroup>,
    /// Textura própria (imagens). `None` para views emprestadas (slots de texto).
    _owned: Option<wgpu::Texture>,
    /// View da textura própria (o vidro óptico a usa como backdrop). `None` para views emprestadas:
    /// o dono (texto, vídeo) é quem entrega a view nesse caso.
    view: Option<wgpu::TextureView>,
    /// Muda a cada (re)associação do id a uma textura/view: chave de caches de bind group.
    version: u64,
    /// Id estável que aponta para outra textura (frame atual de um vídeo): trocar o alvo não
    /// muda a lista de desenho nem cria bind group. Resolvido em 1 nível.
    alias: Option<TextureId>,
}

pub struct Compositor2D {
    pipelines: [wgpu::RenderPipeline; BlendKind::COUNT],
    canvas_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: Vec<TextureEntry>,
    free: Vec<u32>,
    max_texture_dim: u32,
    next_version: u64,
    /// Gerador de mips (criado no 1º upload de imagem com mips).
    mipgen: Option<crate::engine_core::scene3d::gpu_model::MipGenerator>,
}

impl Compositor2D {
    /// `format` = formato da swapchain (a mesma para todos os `Canvas` deste compositor).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let canvas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_c2d_canvas_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_c2d_texture_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("prismview_c2d_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Imagens têm mips (sem cintilar ao reduzir em parallax/escala); texto/vídeo têm 1.
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_compositor2d"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/compositor2d.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_c2d_pipeline_layout"),
            bind_group_layouts: &[&canvas_layout, &texture_layout],
            push_constant_ranges: &[],
        });
        let make = |blend: BlendKind| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("prismview_c2d_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend.state()),
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
            })
        };
        let pipelines = BlendKind::ALL.map(make);

        let mut this = Self {
            pipelines,
            canvas_layout,
            texture_layout,
            sampler,
            textures: Vec::new(),
            free: Vec::new(),
            max_texture_dim: device.limits().max_texture_dimension_2d,
            mipgen: None,
            next_version: 1,
        };
        this.install_white(device, queue);
        this
    }

    fn install_white(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.textures.clear();
        self.free.clear();
        // Índice 0 reservado: `TextureId::WHITE`. Se a criação falhar, fica sem bind group e os
        // quads sólidos simplesmente não desenham (nunca chega a panicar).
        let id = self.create_texture_rgba(device, queue, 1, 1, &[255, 255, 255, 255]);
        if id.is_none() {
            self.textures.push(TextureEntry {
                bind_group: None,
                _owned: None,
                view: None,
                version: 0,
                alias: None,
            });
        }
    }

    fn bump_version(&mut self) -> u64 {
        self.next_version = self.next_version.wrapping_add(1);
        self.next_version
    }

    /// View da textura **própria** de `id` (imagens). `None` para ids emprestados via `bind_view`
    /// (o dono da textura entrega a view) e para ids liberados.
    pub fn texture_view(&self, id: TextureId) -> Option<&wgpu::TextureView> {
        self.entry(id).and_then(|t| t.view.as_ref())
    }

    /// Versão atual da associação de `id` (muda quando o id passa a apontar para outra
    /// textura/view). `0` se desconhecido.
    pub fn texture_version(&self, id: TextureId) -> u64 {
        self.entry(id).map_or(0, |t| t.version)
    }

    /// Entrada de `id`, seguindo um alias (1 nível).
    fn entry(&self, id: TextureId) -> Option<&TextureEntry> {
        let entry = self.textures.get(id.0 as usize)?;
        match entry.alias {
            Some(target) => self.textures.get(target.0 as usize),
            None => Some(entry),
        }
    }

    /// Cria um id estável que desenha `target` (ver `set_alias`). Liberar com `release_texture`.
    pub fn create_alias(&mut self, target: TextureId) -> TextureId {
        let version = self.bump_version();
        self.insert_entry(TextureEntry {
            bind_group: None,
            _owned: None,
            view: None,
            version,
            alias: Some(target),
        })
    }

    /// Redireciona o alias para outra textura. Sem alocação: a lista já gravada passa a desenhar
    /// o novo alvo no próximo passe (frame de vídeo sem rebuild 2D).
    pub fn set_alias(&mut self, alias: TextureId, target: TextureId) {
        if let Some(slot) = self.textures.get_mut(alias.0 as usize) {
            if slot.alias.is_some() {
                slot.alias = Some(target);
            }
        }
    }

    /// Maior lado de textura aceito pelo device.
    pub fn max_texture_dim(&self) -> u32 {
        self.max_texture_dim
    }

    fn bind_view_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_c2d_texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn insert_entry(&mut self, entry: TextureEntry) -> TextureId {
        if let Some(index) = self.free.pop() {
            if let Some(slot) = self.textures.get_mut(index as usize) {
                *slot = entry;
                return TextureId(index);
            }
        }
        let index = u32::try_from(self.textures.len()).unwrap_or(u32::MAX);
        self.textures.push(entry);
        TextureId(index)
    }

    /// Cria uma textura RGBA8 (`Rgba8Unorm`, alpha **pré-multiplicado**) a partir de pixels da
    /// CPU. Só para assets estáticos (imagens); vídeo e texto nunca passam por aqui por frame.
    pub fn create_texture_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        premultiplied_rgba: &[u8],
    ) -> Option<TextureId> {
        self.create_texture_rgba_ext(device, queue, width, height, premultiplied_rgba, false)
    }

    /// Como `create_texture_rgba`, com cadeia de mips gerada na GPU (imagens estáticas: evita
    /// cintilar quando reduzidas por escala/parallax).
    pub fn create_texture_rgba_mipped(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        premultiplied_rgba: &[u8],
    ) -> Option<TextureId> {
        self.create_texture_rgba_ext(device, queue, width, height, premultiplied_rgba, true)
    }

    fn create_texture_rgba_ext(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        premultiplied_rgba: &[u8],
        mipped: bool,
    ) -> Option<TextureId> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if width == 0
            || height == 0
            || width > self.max_texture_dim
            || height > self.max_texture_dim
            || premultiplied_rgba.len() != expected
        {
            return None;
        }
        let mips = if mipped { 32 - width.max(height).leading_zeros() } else { 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prismview_c2d_image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: if mips > 1 {
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
            } else {
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST
            },
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            premultiplied_rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        if mips > 1 {
            let mipgen = self
                .mipgen
                .get_or_insert_with(|| crate::engine_core::scene3d::gpu_model::MipGenerator::new(device));
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismview_c2d_image_mips"),
            });
            mipgen.generate(device, &mut encoder, &texture, wgpu::TextureFormat::Rgba8Unorm, mips);
            queue.submit(std::iter::once(encoder.finish()));
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.bind_view_group(device, &view);
        let version = self.bump_version();
        Some(self.insert_entry(TextureEntry {
            bind_group: Some(bind_group),
            _owned: Some(texture),
            view: Some(view),
            version,
            alias: None,
        }))
    }

    /// Registra (ou atualiza, se `existing` for `Some`) uma view pertencente a outro módulo
    /// (ex.: textura de um slot de texto). O dono deve manter a textura viva enquanto o id for
    /// usado; ao realocar a textura, chame de novo para refazer o bind group.
    /// Como `bind_view`, mas guarda a view: `texture_view(id)` passa a devolvê-la (o vidro óptico
    /// a usa como backdrop — saída 3D, frames de vídeo).
    pub fn bind_owned_view(
        &mut self,
        device: &wgpu::Device,
        existing: Option<TextureId>,
        view: wgpu::TextureView,
    ) -> TextureId {
        let id = self.bind_view(device, existing, &view);
        if let Some(slot) = self.textures.get_mut(id.0 as usize) {
            slot.view = Some(view);
        }
        id
    }

    pub fn bind_view(
        &mut self,
        device: &wgpu::Device,
        existing: Option<TextureId>,
        view: &wgpu::TextureView,
    ) -> TextureId {
        let bind_group = self.bind_view_group(device, view);
        let version = self.bump_version();
        if let Some(id) = existing {
            if id != TextureId::WHITE {
                if let Some(slot) = self.textures.get_mut(id.0 as usize) {
                    slot.bind_group = Some(bind_group);
                    slot._owned = None;
                    slot.view = None;
                    slot.version = version;
                    slot.alias = None;
                    return id;
                }
            }
        }
        self.insert_entry(TextureEntry {
            bind_group: Some(bind_group),
            _owned: None,
            view: None,
            version,
            alias: None,
        })
    }

    /// Libera a textura (o id pode ser reaproveitado). `WHITE` é ignorado.
    pub fn release_texture(&mut self, id: TextureId) {
        if id == TextureId::WHITE {
            return;
        }
        if let Some(slot) = self.textures.get_mut(id.0 as usize) {
            if slot.bind_group.is_some() || slot._owned.is_some() || slot.alias.is_some() {
                slot.bind_group = None;
                slot._owned = None;
                slot.view = None;
                slot.version = 0;
                slot.alias = None;
                self.free.push(id.0);
            }
        }
    }

    /// Cria o `Canvas` de uma saída. Chamar de novo após `Resized` não é necessário: o `Canvas`
    /// só precisa do novo tamanho em `Canvas::begin`.
    pub fn create_canvas(&self, device: &wgpu::Device) -> Canvas {
        Canvas::new(device, &self.canvas_layout)
    }

    /// Grava as instâncias de `canvas` (já enviadas por `Canvas::flush`) no passe.
    pub fn draw(&self, canvas: &Canvas, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_with(canvas, pass, |_, _| {});
    }

    /// Como `draw`, mas chama `on_marker(pass, id)` no ponto exato (ordem Z) de cada marcador
    /// enfileirado com `Canvas::push_marker`. O callback pode trocar pipeline/bind groups à
    /// vontade (vidro óptico); o estado do compositor é restaurado depois.
    pub fn draw_with<F>(&self, canvas: &Canvas, pass: &mut wgpu::RenderPass<'_>, mut on_marker: F)
    where
        F: FnMut(&mut wgpu::RenderPass<'_>, u32),
    {
        if canvas.batches.is_empty() {
            return;
        }
        pass.set_bind_group(0, &canvas.bind_group, &[]);
        let mut current: Option<BlendKind> = None;
        for batch in &canvas.batches {
            if batch.marker != NO_MARKER {
                on_marker(pass, batch.marker);
                pass.set_bind_group(0, &canvas.bind_group, &[]);
                current = None;
                continue;
            }
            if current != Some(batch.blend) {
                pass.set_pipeline(&self.pipelines[batch.blend as usize]);
                current = Some(batch.blend);
            }
            let Some(group) = self.entry(batch.tex).and_then(|t| t.bind_group.as_ref()) else {
                continue;
            };
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..4, batch.start..batch.end);
        }
    }

    /// Passe completo: limpa `view` com `clear` e desenha `canvas`.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: [f32; 4],
        canvas: &Canvas,
    ) {
        self.render_with(encoder, view, clear, canvas, |_, _| {});
    }

    /// `render` com callback de marcadores (ver `draw_with`).
    pub fn render_with<F>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: [f32; 4],
        canvas: &Canvas,
        on_marker: F,
    ) where
        F: FnMut(&mut wgpu::RenderPass<'_>, u32),
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("prismview_c2d_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: f64::from(clear[0]),
                        g: f64::from(clear[1]),
                        b: f64::from(clear[2]),
                        a: f64::from(clear[3]),
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        self.draw_with(canvas, &mut pass, on_marker);
    }
}

// ───────────────────────── Canvas ─────────────────────────

/// `marker` de um item/batch comum (não é um marcador).
const NO_MARKER: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct DrawItem {
    z: u32,
    seq: u32,
    blend: BlendKind,
    tex: TextureId,
    inst: QuadInstance,
    /// `NO_MARKER` ou id de um ponto de callback (não gera instância).
    marker: u32,
}

#[derive(Clone, Copy)]
struct Batch {
    blend: BlendKind,
    tex: TextureId,
    start: u32,
    end: u32,
    marker: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalsRaw {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

const INITIAL_CAPACITY: usize = 256;

/// Caixa orientada da camada 3D nesta saída (px físicos), refeita junto com a lista. Usada para
/// dimensionar o viewport do `Scene3dRenderer` e mapear cliques/cursor para 0..1 da camada.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene3dPlacement {
    pub center: [f32; 2],
    pub half: [f32; 2],
    /// Colunas X/Y da afim (rotação·escala), como `QuadInstance`.
    pub basis: [f32; 4],
}

impl Scene3dPlacement {
    /// Tamanho em px do alvo de render (caixa sem rotação, com a escala da afim).
    pub fn pixel_size(&self) -> (u32, u32) {
        let sx = self.basis[0].hypot(self.basis[1]);
        let sy = self.basis[2].hypot(self.basis[3]);
        let w = (self.half[0] * 2.0 * sx).round();
        let h = (self.half[1] * 2.0 * sy).round();
        let clamp = |v: f32| if v.is_finite() { v.clamp(1.0, 8192.0) as u32 } else { 1 };
        (clamp(w), clamp(h))
    }

    /// Ponto `p` (px locais da saída) → coordenada normalizada 0..1 da camada (`None` = fora).
    pub fn to_local(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let [a, b, c, d] = self.basis; // colunas (a,b) e (c,d)
        let det = a * d - c * b;
        if !det.is_finite() || det.abs() < 1e-8 || self.half[0] <= 0.0 || self.half[1] <= 0.0 {
            return None;
        }
        let (dx, dy) = (p[0] - self.center[0], p[1] - self.center[1]);
        let lx = (d * dx - c * dy) / det;
        let ly = (-b * dx + a * dy) / det;
        let u = lx / (self.half[0] * 2.0) + 0.5;
        let v = ly / (self.half[1] * 2.0) + 0.5;
        ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some([u, v])
    }
}

pub struct Canvas {
    viewport: (u32, u32),
    globals: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    bind_group: wgpu::BindGroup,
    items: Vec<DrawItem>,
    gpu_instances: Vec<QuadInstance>,
    batches: Vec<Batch>,
    seq: u32,
    /// Versão da cena com que a lista foi construída (controlada por `scene2d`).
    pub built_version: u64,
    /// Tamanho (px) e escala de UI da última construção.
    pub built_size: (u32, u32),
    pub built_scale: f32,
    /// Vidros ópticos desta saída (cada um é um marcador na ordem Z; ver `core::glass`).
    pub glass: Option<GlassLayer>,
    /// Regiões tocáveis (controles de mídia) desta saída, refeitas junto com a lista.
    pub hits: super::interaction::HitMap,
    /// Camada 3D desenhada nesta saída (a primeira visível da cena), se houver.
    pub scene3d: Option<Scene3dPlacement>,
    /// Subárvores com binding 2D (pós-ordem: filhos antes dos pais). O caminho rápido de
    /// `scene2d` reaplica só o delta afim nas instâncias, sem refazer a lista.
    pub bound: Vec<BoundSubtree>,
    /// Alguma camada com binding foi pulada no rebuild (invisível/opacidade ~0): sem caminho rápido.
    pub bound_skipped: bool,
    /// Geração dos deltas de binding aplicada a esta lista (controlada por `scene2d`).
    pub built_fx_gen: u64,
    /// Marcadores (vidro) empurrados desde o `begin` (detecta subárvores "rígidas").
    markers: u32,
    /// `seq` de cada instância em `gpu_instances` (a ordem muda no sort do `flush`).
    gpu_seq: Vec<u32>,
    /// Cópia das instâncias do último `flush`: base sem deriva para os deltas.
    base_instances: Vec<QuadInstance>,
}

/// Camada com binding 2D gravada no rebuild (ver `Canvas::bound`).
#[derive(Clone, Debug)]
pub struct BoundSubtree {
    /// Id da camada (consulta ao `LayerBindingSolver`).
    pub key: std::sync::Arc<str>,
    /// `seq` dos itens da camada + descendentes.
    pub seq: std::ops::Range<u32>,
    /// Índices das regiões tocáveis da subárvore.
    pub hits: std::ops::Range<usize>,
    /// Afim do pai (`m` coluna-maior, `t`).
    pub parent_m: [f32; 4],
    pub parent_t: [f32; 2],
    /// Centro no espaço do pai SEM o offset do binding (px), rotação (rad) e escala sem binding.
    pub center_base: [f32; 2],
    pub rot_base: f32,
    pub scale_base: f32,
    /// Escala de UI do rebuild (offset do binding é em px lógicos).
    pub ui: f32,
    /// Delta do binding usado no rebuild.
    pub fx0: super::bindings::LayerFx,
    /// Estado "animando" no rebuild (muda o desenho do texto → exige rebuild ao virar).
    pub moving0: bool,
    /// Contém vidro (marcador) ou a camada 3D: posição gravada fora das instâncias.
    pub rigid: bool,
}

impl Canvas {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_c2d_globals"),
            size: mem::size_of::<GlobalsRaw>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (instances, bind_group) = Self::alloc_instances(device, layout, &globals, INITIAL_CAPACITY);
        Self {
            viewport: (1, 1),
            globals,
            instances,
            capacity: INITIAL_CAPACITY,
            bind_group,
            items: Vec::with_capacity(INITIAL_CAPACITY),
            gpu_instances: Vec::with_capacity(INITIAL_CAPACITY),
            batches: Vec::with_capacity(64),
            seq: 0,
            built_version: u64::MAX,
            built_size: (0, 0),
            built_scale: 0.0,
            glass: None,
            hits: super::interaction::HitMap::default(),
            scene3d: None,
            bound: Vec::new(),
            bound_skipped: false,
            built_fx_gen: u64::MAX,
            markers: 0,
            gpu_seq: Vec::with_capacity(INITIAL_CAPACITY),
            base_instances: Vec::with_capacity(INITIAL_CAPACITY),
        }
    }

    /// Próximo `seq` (início/fim do intervalo de uma subárvore).
    pub fn next_seq(&self) -> u32 {
        self.seq
    }

    /// Marcadores empurrados desde o `begin`.
    pub fn marker_count(&self) -> u32 {
        self.markers
    }

    /// Volta instâncias e regiões tocáveis ao estado do último rebuild (base dos deltas).
    pub fn reset_to_base(&mut self) {
        if self.base_instances.len() == self.gpu_instances.len() {
            self.gpu_instances.copy_from_slice(&self.base_instances);
        }
        self.hits.reset_to_base();
    }

    /// Aplica `p' = lin·p + t` (espaço de tela) às instâncias com `seq` no intervalo e às regiões
    /// tocáveis. `false` se alguma instância tem recorte (clip) e `lin` não é identidade: o
    /// recorte é alinhado à tela e precisa do rebuild.
    pub fn transform_range(
        &mut self,
        seq: &std::ops::Range<u32>,
        hits: &std::ops::Range<usize>,
        lin: [f32; 4],
        t: [f32; 2],
    ) -> bool {
        let pure_translation = (lin[0] - 1.0).abs() < 1e-6
            && lin[1].abs() < 1e-6
            && lin[2].abs() < 1e-6
            && (lin[3] - 1.0).abs() < 1e-6;
        for (inst, s) in self.gpu_instances.iter_mut().zip(self.gpu_seq.iter()) {
            if !seq.contains(s) {
                continue;
            }
            let c = [inst.center_half[0], inst.center_half[1]];
            inst.center_half[0] = lin[0] * c[0] + lin[2] * c[1] + t[0];
            inst.center_half[1] = lin[1] * c[0] + lin[3] * c[1] + t[1];
            let b = inst.basis;
            inst.basis = [
                lin[0] * b[0] + lin[2] * b[1],
                lin[1] * b[0] + lin[3] * b[1],
                lin[0] * b[2] + lin[2] * b[3],
                lin[1] * b[2] + lin[3] * b[3],
            ];
            if inst.clip[2] >= 0.0 {
                if !pure_translation {
                    return false;
                }
                inst.clip[0] += t[0];
                inst.clip[1] += t[1];
            }
        }
        self.hits.transform_range(hits.clone(), lin, t);
        true
    }

    /// Envia as instâncias atuais (após `transform_range`) sem refazer lotes.
    pub fn upload_instances(&self, queue: &wgpu::Queue) {
        if !self.gpu_instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.gpu_instances));
        }
    }

    fn alloc_instances(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        capacity: usize,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_c2d_instances"),
            size: (capacity * mem::size_of::<QuadInstance>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_c2d_canvas"),
            layout,
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

    /// Inicia uma nova lista de desenho para um alvo de `width`×`height` px.
    pub fn begin(&mut self, width: u32, height: u32) {
        self.viewport = (width.max(1), height.max(1));
        self.items.clear();
        self.seq = 0;
        self.hits.clear();
        self.scene3d = None;
        self.bound.clear();
        self.bound_skipped = false;
        self.markers = 0;
    }

    /// Enfileira um quad. A ordem final é `(z, ordem de chamada)`: Z maior fica por cima.
    pub fn push(&mut self, z: u32, blend: BlendKind, tex: TextureId, inst: QuadInstance) {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.items.push(DrawItem {
            z,
            seq,
            blend,
            tex,
            inst,
            marker: NO_MARKER,
        });
    }

    /// Enfileira um ponto de callback na ordem Z (`Compositor2D::draw_with`). `id` deve ser
    /// diferente de `u32::MAX`. Usado pelo vidro óptico, que desenha com pipeline próprio.
    pub fn push_marker(&mut self, z: u32, id: u32) {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.markers = self.markers.wrapping_add(1);
        self.items.push(DrawItem {
            z,
            seq,
            blend: BlendKind::Normal,
            tex: TextureId::WHITE,
            inst: QuadInstance::zeroed(),
            marker: id,
        });
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Ordena por Z, agrupa em batches (mesmo blend + textura) e envia à GPU. O buffer só é
    /// recriado se a lista passou a ser maior que a capacidade (mudança de cena, não por frame).
    pub fn flush(&mut self, compositor: &Compositor2D, device: &wgpu::Device, queue: &wgpu::Queue) {
        // (z, seq) é único: o sort instável não alocaria e ainda é determinístico.
        self.items.sort_unstable_by_key(|i| (i.z, i.seq));

        self.gpu_instances.clear();
        self.gpu_seq.clear();
        self.batches.clear();
        for item in &self.items {
            let index = u32::try_from(self.gpu_instances.len()).unwrap_or(u32::MAX);
            if item.marker != NO_MARKER {
                self.batches.push(Batch {
                    blend: item.blend,
                    tex: item.tex,
                    start: index,
                    end: index,
                    marker: item.marker,
                });
                continue;
            }
            self.gpu_instances.push(item.inst);
            self.gpu_seq.push(item.seq);
            match self.batches.last_mut() {
                Some(last)
                    if last.marker == NO_MARKER
                        && last.blend == item.blend
                        && last.tex == item.tex =>
                {
                    last.end = index.saturating_add(1);
                }
                _ => self.batches.push(Batch {
                    blend: item.blend,
                    tex: item.tex,
                    start: index,
                    end: index.saturating_add(1),
                    marker: NO_MARKER,
                }),
            }
        }

        if self.gpu_instances.len() > self.capacity {
            let capacity = self.gpu_instances.len().next_power_of_two();
            let (buffer, bind_group) =
                Self::alloc_instances(device, &compositor.canvas_layout, &self.globals, capacity);
            self.instances = buffer;
            self.bind_group = bind_group;
            self.capacity = capacity;
        }
        if !self.gpu_instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.gpu_instances));
        }
        self.base_instances.clear();
        self.base_instances.extend_from_slice(&self.gpu_instances);
        self.hits.snapshot_base();
        let globals = GlobalsRaw {
            viewport: [self.viewport.0 as f32, self.viewport.1 as f32],
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
    }
}
