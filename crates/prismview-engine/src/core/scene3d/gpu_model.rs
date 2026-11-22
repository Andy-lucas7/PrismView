//! Upload de um `ModelAsset` (CPU) para a GPU: buffers concatenados por modelo, texturas com mips
//! (geradas na GPU para RGBA8; BC7 vem com a cadeia pronta) e bind groups de material.
//!
//! Layout de vértices (2 streams):
//! - stream 0 `DeformVertex` (48 B: posição, normal, tangente em `vec4`): é o que skinning/morph
//!   reescrevem. Também é lido como storage pelo compute de deformação.
//! - stream 1 `AttrVertex` (20 B: uv0, uv1, cor RGBA8): nunca muda.
//!
//! Índices são locais à primitiva (o draw fatia os vertex buffers no início da primitiva), então a
//! mesma lista de índices serve para a geometria original e para a saída deformada da instância.
//!
//! Tangente com `w == 0` = arquivo sem `TANGENT`: o fragment shader monta o TBN por derivadas.
//!
//! O `GpuModel` também guarda o "rig" (pose de repouso, skins, clipes, malha/skin por nó): a
//! instância não segura o `ModelAsset`, então o vault pode liberar a geometria de CPU (`trim`).

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use glam::Vec3;

use super::animation::Pose;
use crate::engine_core::assets::{
    Aabb, AlphaMode, AnimationClip, AssetHash, MaterialData, ModelAsset, SkinData, TexelFormat,
    TextureAsset, TextureSlot,
};

pub const DEFORM_STRIDE: u64 = std::mem::size_of::<DeformVertex>() as u64;
pub const ATTR_STRIDE: u64 = std::mem::size_of::<AttrVertex>() as u64;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct DeformVertex {
    pub position: [f32; 4],
    pub normal: [f32; 4],
    pub tangent: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct AttrVertex {
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub color: [u8; 4],
}

/// Joints (4 × u16 empacotados em 2 × u32) + pesos. Casa com `SkinVertex` de `deform.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SkinVertex {
    pub joints: [u32; 2],
    pub weights: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct MaterialUniform {
    pub base_color: [f32; 4],
    /// rgb = emissivo linear, w = escala do normal map.
    pub emissive: [f32; 4],
    /// metalicidade, rugosidade, força da oclusão, alpha cutoff.
    pub params: [f32; 4],
    /// x = bits de textura presentes, y = bits "usa UV1", z = alpha mode (0/1/2), w = double-sided.
    pub flags: [u32; 4],
}

pub const TEX_BASE: u32 = 1;
pub const TEX_MR: u32 = 2;
pub const TEX_NORMAL: u32 = 4;
pub const TEX_OCCLUSION: u32 = 8;
pub const TEX_EMISSIVE: u32 = 16;

pub fn vertex_layouts() -> [wgpu::VertexBufferLayout<'static>; 2] {
    const DEFORM_ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4
    ];
    const ATTR_ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        3 => Float32x2, 4 => Float32x2, 5 => Unorm8x4
    ];
    [
        wgpu::VertexBufferLayout {
            array_stride: DEFORM_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &DEFORM_ATTRS,
        },
        wgpu::VertexBufferLayout {
            array_stride: ATTR_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTR_ATTRS,
        },
    ]
}

// ───────────────────────── Geração de mips ─────────────────────────

pub struct MipGenerator {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    unorm: wgpu::RenderPipeline,
    srgb: wgpu::RenderPipeline,
}

impl MipGenerator {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_mipgen"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/mipgen.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_mipgen"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_mipgen"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let make = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("prismview_mipgen"),
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
        };
        Self {
            unorm: make(wgpu::TextureFormat::Rgba8Unorm),
            srgb: make(wgpu::TextureFormat::Rgba8UnormSrgb),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("prismview_mipgen"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            layout,
        }
    }

    /// Preenche os mips 1.. de `texture` a partir do mip 0 (só RGBA8/RGBA8 sRGB).
    fn generate(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        format: wgpu::TextureFormat,
        mips: u32,
    ) {
        let pipeline = match format {
            wgpu::TextureFormat::Rgba8Unorm => &self.unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb => &self.srgb,
            _ => return,
        };
        let view = |mip: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("prismview_mipgen_level"),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for mip in 1..mips {
            let src = view(mip - 1);
            let dst = view(mip);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("prismview_mipgen"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismview_mipgen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
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
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

// ───────────────────────── Contexto de upload ─────────────────────────

/// Recursos compartilhados por todos os modelos (1 por device).
pub struct ModelUploadContext {
    pub material_layout: wgpu::BindGroupLayout,
    pub material_sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    flat_normal: wgpu::TextureView,
    _fallback: [wgpu::Texture; 2],
    mipgen: MipGenerator,
    bc_supported: bool,
}

fn solid_texture(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4], label: &str) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    )
}

impl ModelUploadContext {
    /// O device aceita BC7 (vale para `LoaderConfig::allow_bc7`).
    pub fn bc_supported(&self) -> bool {
        self.bc_supported
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_pbr_material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<MaterialUniform>() as u64),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                texture_entry(4),
                texture_entry(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("prismview_pbr_material"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let white = solid_texture(device, queue, [255, 255, 255, 255], "prismview_pbr_white");
        let flat = solid_texture(device, queue, [128, 128, 255, 255], "prismview_pbr_flat_normal");
        Self {
            material_layout,
            material_sampler,
            white: white.create_view(&wgpu::TextureViewDescriptor::default()),
            flat_normal: flat.create_view(&wgpu::TextureViewDescriptor::default()),
            _fallback: [white, flat],
            mipgen: MipGenerator::new(device),
            bc_supported: device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
        }
    }
}

// ───────────────────────── Modelo na GPU ─────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct GpuPrimitive {
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub first_index: u32,
    pub index_count: u32,
    /// Índice em `GpuModel::materials` (o último é o material padrão).
    pub material: u32,
    /// Primeira entrada (par posição/normal) de morph desta primitiva em `morph`.
    pub morph_base: u32,
    pub morph_count: u32,
    pub skinned: bool,
    /// AABB no espaço do nó (pose de repouso).
    pub bounds: Aabb,
}

pub struct GpuMaterial {
    pub bind_group: wgpu::BindGroup,
    pub blend: bool,
    pub double_sided: bool,
    _uniform: wgpu::Buffer,
}

pub struct GpuModel {
    pub hash: AssetHash,
    /// Geometria original (stream 0); também storage de entrada da deformação.
    pub vertex: wgpu::Buffer,
    pub attr: wgpu::Buffer,
    pub index: wgpu::Buffer,
    /// `SkinVertex` por vértice (zeros fora de primitivas com skin).
    pub skin: wgpu::Buffer,
    /// Deltas de morph: `vec4` posição + `vec4` normal por (alvo, vértice).
    pub morph: wgpu::Buffer,
    pub primitives: Vec<GpuPrimitive>,
    /// `(primeira primitiva, quantidade)` por malha de `ModelAsset::meshes`.
    pub mesh_primitives: Vec<(u32, u32)>,
    pub materials: Vec<GpuMaterial>,
    pub vertex_count: u32,
    _textures: Vec<wgpu::Texture>,
    /// Estimativa de VRAM (buffers + texturas).
    pub vram_bytes: u64,
    pub rig: ModelRig,
}

/// Dados de CPU que as instâncias precisam além da GPU (pequenos comparados à geometria).
pub struct ModelRig {
    pub rest_pose: Pose,
    pub node_mesh: Vec<Option<u32>>,
    pub node_skin: Vec<Option<u32>>,
    pub skins: Vec<SkinData>,
    pub animations: Vec<AnimationClip>,
    /// AABB do modelo inteiro (pose de repouso, espaço do modelo).
    pub bounds: Aabb,
    /// Nomes para resolver alvos de bindings (Fase 4) — só lidos no carregamento da cena.
    pub node_names: Vec<Option<String>>,
    pub mesh_names: Vec<Option<String>>,
    /// Índice = material do `GpuModel` (o último, padrão, não tem nome).
    pub material_names: Vec<Option<String>>,
    /// `mesh.extras.targetNames` por malha (vazio se o arquivo não nomeia os morph targets).
    pub morph_names: Vec<Vec<String>>,
}

impl ModelRig {
    /// Nó por nome (glTF `node.name`). `None` se não existir.
    pub fn node_by_name(&self, name: &str) -> Option<u32> {
        self.node_names
            .iter()
            .position(|n| n.as_deref() == Some(name))
            .and_then(|i| u32::try_from(i).ok())
    }

    /// Nome do nó ou da malha do nó bate com `name` (como `matchesMeshTarget` do Estúdio, que
    /// compara o nome do objeto Three — o nó — e o da malha).
    pub fn node_matches(&self, node: u32, name: &str) -> bool {
        let n = node as usize;
        if self.node_names.get(n).and_then(Option::as_deref) == Some(name) {
            return true;
        }
        self.node_mesh
            .get(n)
            .copied()
            .flatten()
            .and_then(|m| self.mesh_names.get(m as usize))
            .and_then(Option::as_deref)
            == Some(name)
    }
}

fn buffer_or_dummy(device: &wgpu::Device, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    // Bindings exigem buffer não vazio; 64 B zerados servem de placeholder.
    const DUMMY: [u8; 64] = [0; 64];
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: if bytes.is_empty() { &DUMMY } else { bytes },
        usage,
    })
}

#[inline]
fn color_u8(c: [f32; 4]) -> [u8; 4] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

#[inline]
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

impl GpuModel {
    /// Sobe o modelo inteiro. Chamado no evento de carregamento (nunca no quadro).
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, ctx: &ModelUploadContext, asset: &ModelAsset) -> Self {
        // ── Geometria ──
        let total_vertices: usize = asset
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .map(|p| p.positions.len())
            .sum();
        let total_indices: usize = asset
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .map(|p| p.indices.len())
            .sum();
        let any_skin = asset
            .meshes
            .iter()
            .flat_map(|m| m.primitives.iter())
            .any(|p| !p.joints.is_empty());

        let mut deform: Vec<DeformVertex> = Vec::with_capacity(total_vertices);
        let mut attrs: Vec<AttrVertex> = Vec::with_capacity(total_vertices);
        let mut skin: Vec<SkinVertex> = Vec::with_capacity(if any_skin { total_vertices } else { 0 });
        let mut morph: Vec<[f32; 4]> = Vec::new();
        let mut indices: Vec<u32> = Vec::with_capacity(total_indices);
        let mut primitives: Vec<GpuPrimitive> = Vec::new();
        let mut mesh_primitives: Vec<(u32, u32)> = Vec::with_capacity(asset.meshes.len());
        let default_material = u32::try_from(asset.materials.len()).unwrap_or(u32::MAX);

        for mesh in &asset.meshes {
            let first_prim = u32::try_from(primitives.len()).unwrap_or(u32::MAX);
            for prim in &mesh.primitives {
                let vcount = prim.positions.len();
                let first_vertex = u32::try_from(deform.len()).unwrap_or(u32::MAX);
                let first_index = u32::try_from(indices.len()).unwrap_or(u32::MAX);
                for v in 0..vcount {
                    let p = prim.positions[v];
                    let n = prim.normals.get(v).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let t = prim.tangents.get(v).copied().unwrap_or([0.0; 4]);
                    // w ∈ {-1, +1} com tangente; 0 sinaliza "sem tangente".
                    let tw = if prim.tangents.is_empty() {
                        0.0
                    } else if t[3] < 0.0 {
                        -1.0
                    } else {
                        1.0
                    };
                    deform.push(DeformVertex {
                        position: [p[0], p[1], p[2], 1.0],
                        normal: [n[0], n[1], n[2], 0.0],
                        tangent: [t[0], t[1], t[2], tw],
                    });
                    attrs.push(AttrVertex {
                        uv0: prim.uv0.get(v).copied().unwrap_or([0.0; 2]),
                        uv1: prim.uv1.get(v).copied().unwrap_or([0.0; 2]),
                        color: prim.colors.get(v).copied().map_or([255; 4], color_u8),
                    });
                    if any_skin {
                        let j = prim.joints.get(v).copied().unwrap_or([0; 4]);
                        let w = prim.weights.get(v).copied().unwrap_or([0.0; 4]);
                        skin.push(SkinVertex {
                            joints: [
                                u32::from(j[0]) | (u32::from(j[1]) << 16),
                                u32::from(j[2]) | (u32::from(j[3]) << 16),
                            ],
                            weights: w.map(|x| finite_or(x, 0.0)),
                        });
                    }
                }
                let morph_base = u32::try_from(morph.len() / 2).unwrap_or(u32::MAX);
                for target in &prim.morph_targets {
                    for v in 0..vcount {
                        let dp = target.positions.get(v).copied().unwrap_or([0.0; 3]);
                        let dn = target.normals.get(v).copied().unwrap_or([0.0; 3]);
                        morph.push([dp[0], dp[1], dp[2], 0.0]);
                        morph.push([dn[0], dn[1], dn[2], 0.0]);
                    }
                }
                indices.extend_from_slice(&prim.indices);
                // Bounds conservadores com morph: soma dos maiores deslocamentos de cada alvo.
                let mut bounds = prim.bounds.unwrap_or(Aabb::EMPTY);
                if !bounds.is_empty() && !prim.morph_targets.is_empty() {
                    let reach: f32 = prim
                        .morph_targets
                        .iter()
                        .map(|t| {
                            t.positions
                                .iter()
                                .map(|d| d[0].abs().max(d[1].abs()).max(d[2].abs()))
                                .filter(|v| v.is_finite())
                                .fold(0.0f32, f32::max)
                        })
                        .sum();
                    bounds.min -= Vec3::splat(reach);
                    bounds.max += Vec3::splat(reach);
                }
                primitives.push(GpuPrimitive {
                    first_vertex,
                    vertex_count: u32::try_from(vcount).unwrap_or(0),
                    first_index,
                    index_count: u32::try_from(prim.indices.len()).unwrap_or(0),
                    material: prim.material.unwrap_or(default_material),
                    morph_base,
                    morph_count: u32::try_from(prim.morph_targets.len()).unwrap_or(0),
                    skinned: !prim.joints.is_empty(),
                    bounds,
                });
            }
            let count = u32::try_from(primitives.len()).unwrap_or(u32::MAX) - first_prim;
            mesh_primitives.push((first_prim, count));
        }

        let storage_vertex = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE;
        let vertex = buffer_or_dummy(device, "prismview_model_vertex", bytemuck::cast_slice(&deform), storage_vertex);
        let attr = buffer_or_dummy(device, "prismview_model_attr", bytemuck::cast_slice(&attrs), wgpu::BufferUsages::VERTEX);
        let index = buffer_or_dummy(device, "prismview_model_index", bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX);
        let skin_buf = buffer_or_dummy(device, "prismview_model_skin", bytemuck::cast_slice(&skin), wgpu::BufferUsages::STORAGE);
        let morph_buf = buffer_or_dummy(device, "prismview_model_morph", bytemuck::cast_slice(&morph), wgpu::BufferUsages::STORAGE);
        let mut vram = (deform.len() as u64 * DEFORM_STRIDE)
            + (attrs.len() as u64 * ATTR_STRIDE)
            + (indices.len() as u64 * 4)
            + (skin.len() as u64 * 24)
            + (morph.len() as u64 * 16);

        // ── Texturas ──
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("prismview_model_upload"),
        });
        let mut textures: Vec<Option<wgpu::Texture>> = Vec::with_capacity(asset.textures.len());
        for tex in &asset.textures {
            let uploaded = upload_texture(device, queue, &mut encoder, ctx, tex);
            if let Some((_, bytes)) = &uploaded {
                vram += *bytes;
            }
            textures.push(uploaded.map(|(t, _)| t));
        }
        queue.submit(std::iter::once(encoder.finish()));
        let views: Vec<Option<wgpu::TextureView>> = textures
            .iter()
            .map(|t| t.as_ref().map(|t| t.create_view(&wgpu::TextureViewDescriptor::default())))
            .collect();

        // ── Materiais (+ padrão no fim) ──
        let default = MaterialData::default();
        let materials: Vec<GpuMaterial> = asset
            .materials
            .iter()
            .chain(std::iter::once(&default))
            .map(|m| build_material(device, ctx, m, &views))
            .collect();

        Self {
            hash: asset.hash,
            vertex,
            attr,
            index,
            skin: skin_buf,
            morph: morph_buf,
            primitives,
            mesh_primitives,
            materials,
            vertex_count: u32::try_from(deform.len()).unwrap_or(u32::MAX),
            _textures: textures.into_iter().flatten().collect(),
            vram_bytes: vram,
            rig: ModelRig {
                rest_pose: Pose::new(asset),
                node_mesh: asset.nodes.iter().map(|n| n.mesh).collect(),
                node_skin: asset.nodes.iter().map(|n| n.skin).collect(),
                skins: asset.skins.clone(),
                animations: asset.animations.clone(),
                bounds: asset.bounds,
                node_names: asset.nodes.iter().map(|n| n.name.clone()).collect(),
                mesh_names: asset.meshes.iter().map(|m| m.name.clone()).collect(),
                material_names: asset.materials.iter().map(|m| m.name.clone()).collect(),
                morph_names: asset.meshes.iter().map(|m| m.target_names.clone()).collect(),
            },
        }
    }

    pub fn mesh_primitives(&self, mesh: u32) -> &[GpuPrimitive] {
        match self.mesh_primitives.get(mesh as usize) {
            Some(&(start, count)) => self
                .primitives
                .get(start as usize..(start + count) as usize)
                .unwrap_or(&[]),
            None => &[],
        }
    }
}

/// Sobe uma textura. `None` se o formato não é suportado pelo device (BC7 sem a feature) ou os
/// dados estão incoerentes: o material cai na textura neutra.
fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    ctx: &ModelUploadContext,
    tex: &Arc<TextureAsset>,
) -> Option<(wgpu::Texture, u64)> {
    let format = tex.wgpu_format();
    let max_dim = device.limits().max_texture_dimension_2d;
    if tex.width == 0 || tex.height == 0 || tex.width > max_dim || tex.height > max_dim {
        return None;
    }
    let (block, block_bytes) = match tex.format {
        TexelFormat::Bc7 => {
            if !ctx.bc_supported || tex.width % 4 != 0 || tex.height % 4 != 0 {
                return None;
            }
            (4u32, 16u32)
        }
        TexelFormat::Rgba8 => (1, 4),
    };
    let first = tex.levels.first()?;
    let full_chain = 32 - tex.width.max(tex.height).leading_zeros();
    let generate = tex.format == TexelFormat::Rgba8 && tex.generate_mips && tex.levels.len() == 1;
    let mip_count = if generate {
        full_chain
    } else {
        u32::try_from(tex.levels.len()).unwrap_or(1).clamp(1, full_chain)
    };
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    if generate {
        usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("prismview_model_texture"),
        size: wgpu::Extent3d {
            width: first.width.max(1),
            height: first.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });

    let upload_levels = if generate { 1 } else { mip_count };
    let mut bytes_total = 0u64;
    for (mip, level) in tex.levels.iter().take(upload_levels as usize).enumerate() {
        let data = tex.level_bytes(level)?;
        let blocks_w = level.width.max(1).div_ceil(block);
        let blocks_h = level.height.max(1).div_ceil(block);
        let bytes_per_row = blocks_w * block_bytes;
        let needed = bytes_per_row as usize * blocks_h as usize;
        if data.len() < needed {
            break; // nível truncado: mips restantes ficam indefinidos, mas não derruba nada
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: u32::try_from(mip).unwrap_or(0),
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data[..needed],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(blocks_h),
            },
            wgpu::Extent3d {
                width: blocks_w * block,
                height: blocks_h * block,
                depth_or_array_layers: 1,
            },
        );
        bytes_total += needed as u64;
    }
    if generate {
        ctx.mipgen.generate(device, encoder, &texture, format, mip_count);
        bytes_total += bytes_total / 3; // cadeia de mips ≈ +1/3
    }
    Some((texture, bytes_total))
}

fn resolve_slot(views: &[Option<wgpu::TextureView>], slot: Option<TextureSlot>) -> Option<(&wgpu::TextureView, bool)> {
    let slot = slot?;
    let view = views.get(slot.texture as usize)?.as_ref()?;
    Some((view, slot.uv_set == 1))
}

fn build_material(
    device: &wgpu::Device,
    ctx: &ModelUploadContext,
    m: &MaterialData,
    views: &[Option<wgpu::TextureView>],
) -> GpuMaterial {
    let resolve = |slot: Option<TextureSlot>| resolve_slot(views, slot);
    let slots = [
        (resolve(m.base_color_tex), TEX_BASE, &ctx.white),
        (resolve(m.metallic_roughness_tex), TEX_MR, &ctx.white),
        (resolve(m.normal_tex), TEX_NORMAL, &ctx.flat_normal),
        (resolve(m.occlusion_tex), TEX_OCCLUSION, &ctx.white),
        (resolve(m.emissive_tex), TEX_EMISSIVE, &ctx.white),
    ];
    let mut present = 0u32;
    let mut uv1 = 0u32;
    for (resolved, bit, _) in &slots {
        if let Some((_, use_uv1)) = resolved {
            present |= bit;
            if *use_uv1 {
                uv1 |= bit;
            }
        }
    }
    let alpha_mode = match m.alpha_mode {
        AlphaMode::Opaque => 0u32,
        AlphaMode::Mask => 1,
        AlphaMode::Blend => 2,
    };
    let uniform = MaterialUniform {
        base_color: m.base_color.map(|c| finite_or(c, 1.0)),
        emissive: [
            finite_or(m.emissive[0], 0.0),
            finite_or(m.emissive[1], 0.0),
            finite_or(m.emissive[2], 0.0),
            finite_or(m.normal_scale, 1.0),
        ],
        params: [
            finite_or(m.metallic, 1.0).clamp(0.0, 1.0),
            finite_or(m.roughness, 1.0).clamp(0.0, 1.0),
            finite_or(m.occlusion_strength, 1.0).clamp(0.0, 1.0),
            finite_or(m.alpha_cutoff, 0.5),
        ],
        flags: [present, uv1, alpha_mode, u32::from(m.double_sided)],
    };
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("prismview_pbr_material"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let chosen: [&wgpu::TextureView; 5] = [0usize, 1, 2, 3, 4].map(|i| {
        let (resolved, _, fallback) = slots[i];
        resolved.map_or(fallback, |(v, _)| v)
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("prismview_pbr_material"),
        layout: &ctx.material_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(chosen[0]),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(chosen[1]),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(chosen[2]),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(chosen[3]),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(chosen[4]),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(&ctx.material_sampler),
            },
        ],
    });
    GpuMaterial {
        bind_group,
        blend: m.alpha_mode == AlphaMode::Blend,
        double_sided: m.double_sided,
        _uniform: buffer,
    }
}
