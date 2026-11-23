//! Renderer 3D nativo (Fase 3): cena multi-objeto do `.prism` (`Scene3dConfig`) desenhada em wgpu
//! sem Three.js.
//!
//! Por quadro (`update` + `render`):
//! 1. animação de clipes, auto-rotação, câmera orbital amortecida, ciclo dia/noite;
//! 2. matrizes de objeto pelo grafo `parentId` e pose dos nós (só se mudou);
//! 3. compute de skinning/morph (só instâncias com pose nova);
//! 4. frustum culling por AABB, ordenação (opacos por estado, transparentes de trás p/ frente);
//! 5. passe PBR HDR (MSAA opcional, reverse-Z) + passe de tone mapping ACES → `output_view()`
//!    (RGBA8 pré-multiplicado, pronto para o compositor 2D).
//!
//! Regras PLANO_NATIVO: nada de `unwrap/expect`; zero alocação no quadro (listas de draw, staging
//! e uniforms pré-alocados e reaproveitados com `.clear()`); render targets recriados só em
//! resize; ambiente (PMREM) só em troca de preset/HDRI; `needs_frame()` falso = loop pode dormir.
//!
//! Fase 4: `set_signals` entrega o `SignalSnapshot` do quadro; `update` roda o `BindingSolver3d`
//! depois de avançar os clipes (desfaz → anima → reaplica, ver `core::bindings`). Hover/clique em
//! malha usam `pick` (raio × AABB de mundo de cada draw, sem alocar).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, UNIX_EPOCH};

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};

use crate::engine_core::assets::Aabb;
use crate::engine_core::bindings::{BindingSolver3d, InstanceAccess, PickHit};
use crate::engine_core::signals::{SignalKind, SignalSnapshot};

use super::config::{
    Scene3dBackgroundMode, Scene3dConfig, Scene3dDynamicTexture, Scene3dDynamicTextureSource, Scene3dEnvPreset,
    Scene3dObject, Scene3dObjectKind,
};
use super::culling::Frustum;
use super::environment::{load_hdri, EnvBaker, EnvSource, EnvironmentMaps, EquirectImage};
use super::gpu_model::{vertex_layouts, GpuMaterial, GpuModel, ModelUploadContext, ATTR_STRIDE};
use super::graph::SceneGraph;
use super::instance::{DeformKernel, ModelInstance};
use super::lighting::{
    day_night_background, linear_to_srgb, parse_hex_srgb, preset_background, LightRig, ProceduralEnv, MAX_DIR_LIGHTS,
    MAX_POINT_LIGHTS,
};
use super::sculptures::{build_showcase_asset, ShowcaseVariant};
use crate::engine_core::assets::{AssetHash, AssetLoader, LoadEvent, LoaderConfig, ModelAsset, Ticket};
use crate::engine_core::gpu::GpuContext;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Saída para o compositor 2D: valores sRGB em formato unorm, alpha pré-multiplicado.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const NEAR_PLANE: f32 = 0.05;
/// O ciclo dia/noite muda devagar: reavaliar a cada 30 s basta.
const DAY_NIGHT_PERIOD: f32 = 30.0;
const DRAW_STRIDE: u64 = std::mem::size_of::<DrawGpu>() as u64;
/// Limite de tamanho em disco e resolução máxima para texturas dinâmicas (`MEDIA_COVER`/`CUSTOM_IMAGE`).
const MAX_DYNAMIC_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DYNAMIC_IMAGE_DIM: u32 = 2048;

// ───────────────────────── Uniforms ─────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [f32; 16],
    camera_pos: [f32; 4],
    ambient: [f32; 4],
    env: [f32; 4],
    dir: [[f32; 8]; MAX_DIR_LIGHTS],
    point: [[f32; 8]; MAX_POINT_LIGHTS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawGpu {
    model: [f32; 16],
    normal: [f32; 16],
    /// x = brilho emissivo extra dos bindings (`EMISSIVE_GLOW`); yzw livres.
    glow: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TonemapUniform {
    info: [f32; 4],
    background: [f32; 4],
}

// ───────────────────────── Recursos por device ─────────────────────────

/// Pipelines, layouts, LUT do BRDF e cache de modelos na GPU. 1 por device, compartilhado entre
/// renderers (um por monitor/camada) via `Arc`.
pub struct Scene3dShared {
    pub upload: ModelUploadContext,
    baker: EnvBaker,
    _brdf_lut: wgpu::Texture,
    brdf_lut_view: wgpu::TextureView,
    env_sampler: wgpu::Sampler,
    frame_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    deform: DeformKernel,
    pbr_module: wgpu::ShaderModule,
    tonemap_module: wgpu::ShaderModule,
    models: Mutex<HashMap<AssetHash, Weak<GpuModel>>>,
}

impl Scene3dShared {
    pub fn new(gpu: &GpuContext) -> Arc<Self> {
        let device = gpu.device();
        let queue = gpu.queue();
        let baker = EnvBaker::new(device);
        let (brdf_lut, brdf_lut_view) = baker.bake_brdf_lut(device, queue);

        let frag = wgpu::ShaderStages::FRAGMENT;
        let tex = |binding: u32, dim: wgpu::TextureViewDimension, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_pbr_frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<FrameUniform>() as u64),
                    },
                    count: None,
                },
                tex(1, wgpu::TextureViewDimension::Cube, true),
                tex(2, wgpu::TextureViewDimension::Cube, true),
                tex(3, wgpu::TextureViewDimension::D2, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_pbr_draws"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(DRAW_STRIDE),
                },
                count: None,
            }],
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_tonemap"),
            entries: &[
                tex(0, wgpu::TextureViewDimension::D2, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<TonemapUniform>() as u64),
                    },
                    count: None,
                },
            ],
        });

        Arc::new(Self {
            upload: ModelUploadContext::new(device, queue),
            _brdf_lut: brdf_lut,
            brdf_lut_view,
            env_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("prismview_pbr_env"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            frame_layout,
            draw_layout,
            tonemap_layout,
            deform: DeformKernel::new(device),
            pbr_module: device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("prismview_pbr"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/pbr.wgsl").into()),
            }),
            tonemap_module: device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("prismview_tonemap"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/tonemap.wgsl").into()),
            }),
            baker,
            models: Mutex::new(HashMap::new()),
        })
    }

    /// Configuração do `AssetLoader` coerente com o device (BC7 só se suportado).
    pub fn loader_config(&self) -> LoaderConfig {
        LoaderConfig {
            allow_bc7: self.upload.bc_supported(),
            ..LoaderConfig::default()
        }
    }

    /// `GpuModel` do asset: reaproveita o upload de outro objeto/monitor com o mesmo hash.
    pub fn model(&self, gpu: &GpuContext, asset: &ModelAsset) -> Arc<GpuModel> {
        let mut models = self.models.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(model) = models.get(&asset.hash).and_then(Weak::upgrade) {
            return model;
        }
        models.retain(|_, w| w.strong_count() > 0);
        let model = Arc::new(GpuModel::upload(gpu.device(), gpu.queue(), &self.upload, asset));
        models.insert(asset.hash, Arc::downgrade(&model));
        model
    }
}

// ───────────────────────── Estado por objeto ─────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DynamicImageKey {
    path: PathBuf,
    len: u64,
    modified_ns: u128,
}

impl DynamicImageKey {
    fn from_path(path: PathBuf) -> Option<Self> {
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_DYNAMIC_IMAGE_BYTES {
            return None;
        }
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        Some(Self {
            path,
            len: meta.len(),
            modified_ns,
        })
    }
}

struct DecodedDynamicImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

type DynamicTexResult = (DynamicImageKey, Result<DecodedDynamicImage, String>);

enum DynamicTexSlot {
    Loading,
    Ready {
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
        vram_bytes: u64,
    },
    Failed,
}

struct SlotDynamicRule {
    rule: Scene3dDynamicTexture,
    /// Chave resolvida em `set_config` para regras `CUSTOM_IMAGE`.
    custom_key: Option<DynamicImageKey>,
}

struct ObjectSlot {
    id: String,
    src: String,
    kind: Scene3dObjectKind,
    ticket: Option<Ticket>,
    instance: Option<ModelInstance>,
    error: Option<String>,
    /// Giro acumulado de `autoRotateSpeed` (rad).
    spin: f32,
    /// Regras tipadas de `dynamicTextures` deste objeto.
    dynamic_rules: Vec<SlotDynamicRule>,
    /// Overrides de material na GPU (indexados pelo índice de material do `GpuModel`).
    /// Vazio quando nenhuma textura dinâmica está ativa no objeto.
    material_overrides: Vec<Option<GpuMaterial>>,
}

impl ObjectSlot {
    #[inline]
    fn material<'a>(&'a self, instance: &'a ModelInstance, material_idx: u32) -> Option<&'a GpuMaterial> {
        let idx = material_idx as usize;
        self.material_overrides
            .get(idx)
            .and_then(Option::as_ref)
            .or_else(|| instance.model.materials.get(idx))
    }
}

fn decode_dynamic_image(path: &Path, max_dim: u32) -> Result<DecodedDynamicImage, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("metadata: {e}"))?;
    if meta.len() == 0 {
        return Err("arquivo vazio".to_owned());
    }
    if meta.len() > MAX_DYNAMIC_IMAGE_BYTES {
        return Err("imagem excede o limite de 16 MiB".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("leitura: {e}"))?;
    let mut img = image::load_from_memory(&bytes).map_err(|e| format!("decodificação: {e}"))?;
    if img.width() == 0 || img.height() == 0 {
        return Err("dimensões inválidas".to_owned());
    }
    if img.width() > max_dim || img.height() > max_dim {
        img = img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Ok(DecodedDynamicImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn rebuild_slot_overrides(
    device: &wgpu::Device,
    upload: &ModelUploadContext,
    slot: &mut ObjectSlot,
    media_cover_key: Option<&DynamicImageKey>,
    cache: &HashMap<DynamicImageKey, DynamicTexSlot>,
) -> bool {
    let had_overrides = !slot.material_overrides.is_empty();
    slot.material_overrides.clear();
    if slot.dynamic_rules.is_empty() {
        return had_overrides;
    }
    let Some(instance) = slot.instance.as_ref() else {
        return had_overrides;
    };
    let model = &instance.model;
    let mat_count = model.materials.len();
    if mat_count == 0 {
        return had_overrides;
    }

    let mut matched: Vec<Option<(&wgpu::TextureView, bool)>> = vec![None; mat_count];
    let mut any_match = false;
    // Wildcard ("", "*") primeiro; nomes específicos depois para sempre terem prioridade.
    for pass_named in [false, true] {
        for entry in &slot.dynamic_rules {
            if entry.rule.is_wildcard() == pass_named {
                continue;
            }
            let key = match entry.rule.source {
                Scene3dDynamicTextureSource::MediaCover => media_cover_key,
                Scene3dDynamicTextureSource::CustomImage => entry.custom_key.as_ref(),
            };
            let Some(DynamicTexSlot::Ready { view, .. }) = key.and_then(|k| cache.get(k)) else {
                continue;
            };
            for (mi, target) in matched.iter_mut().enumerate() {
                let mat_name = model.rig.material_names.get(mi).and_then(Option::as_deref);
                if entry.rule.matches_material(mat_name) {
                    *target = Some((view, entry.rule.emissive_map));
                    any_match = true;
                }
            }
        }
    }

    if !any_match {
        return had_overrides;
    }

    slot.material_overrides = matched
        .into_iter()
        .enumerate()
        .map(|(mi, m)| {
            let (view, emissive_map) = m?;
            let base = model.material_data.get(mi)?;
            Some(upload.build_material_override(device, base, &model.texture_views, view, emissive_map))
        })
        .collect();
    true
}

#[derive(Clone, Copy, Debug)]
struct DrawCmd {
    key: u64,
    depth: f32,
    object: u32,
    draw: u32,
    slot: u32,
    variant: u8,
}

struct Targets {
    size: (u32, u32),
    _msaa: Option<wgpu::Texture>,
    msaa_view: Option<wgpu::TextureView>,
    _hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    tonemap_bind: wgpu::BindGroup,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct EnvKey {
    preset: Option<Scene3dEnvPreset>,
    hdri: Option<PathBuf>,
}

type HdriResult = (PathBuf, Result<EquirectImage, String>);

#[derive(Clone, Copy, Debug)]
pub struct Scene3dOptions {
    /// 1 ou 4 (MSAA do passe HDR).
    pub msaa: u32,
}

impl Default for Scene3dOptions {
    fn default() -> Self {
        Self { msaa: 4 }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Scene3dStats {
    pub drawn: u32,
    pub culled: u32,
    pub instances: u32,
    pub vram_bytes: u64,
}

// ───────────────────────── Renderer ─────────────────────────

/// Acima disto (px de render) o MSAA 4× é desligado mesmo no modo Normal.
const MSAA_MAX_PIXELS: u64 = 2560 * 1440;

pub struct Scene3dRenderer {
    shared: Arc<Scene3dShared>,
    sample_count: u32,
    /// [opaco, opaco double-sided, blend, blend double-sided]
    pipelines: [wgpu::RenderPipeline; 4],
    /// Pipelines do outro `sample_count` (1 ↔ 4), guardados ao trocar: voltar não recompila.
    spare_pipelines: Option<(u32, [wgpu::RenderPipeline; 4])>,
    /// Teto de MSAA do PowerGovernor (`PowerCaps::msaa`).
    msaa_cap: u32,
    tonemap_pipeline: wgpu::RenderPipeline,

    config: Scene3dConfig,
    objects: Vec<Scene3dObject>,
    graph: SceneGraph,
    visible: Vec<bool>,
    object_world: Vec<Mat4>,
    slots: Vec<ObjectSlot>,
    asset_root: Option<PathBuf>,

    env: EnvironmentMaps,
    env_key: EnvKey,
    hdri_rx: Option<Receiver<HdriResult>>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
    lights: LightRig,
    day_night_timer: f32,
    /// Fundo linear do modo ENVIRONMENT (cor do preset ou céu do dia/noite).
    sky_background: Vec3,

    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    draw_bind: wgpu::BindGroup,
    draw_capacity: usize,
    tonemap_buffer: wgpu::Buffer,
    targets: Option<Targets>,
    /// Incrementa a cada (re)criação dos alvos: o compositor refaz o bind da `output_view`.
    output_generation: u64,
    viewport: (u32, u32),
    render_scale_cap: f32,

    // Scratch do quadro (capacidade fixa entre trocas de cena).
    draw_staging: Vec<DrawGpu>,
    opaque: Vec<DrawCmd>,
    blended: Vec<DrawCmd>,

    orbit: [f32; 2],
    pointer: Option<[f32; 2]>,
    /// Cursor para olhar/órbita (widget: cursor da tela inteira mapeado; `None` = usa `pointer`).
    look_pointer: Option<[f32; 2]>,
    dirty: bool,
    animating: bool,
    stats: Scene3dStats,

    // Fase 4: bindings reativos.
    solver: BindingSolver3d,
    signals: SignalSnapshot,
    /// Cursor da última consulta de hover (re-pick só se mudou).
    hover_pointer: Option<[f32; 2]>,

    // Texturas dinâmicas (`MEDIA_COVER` e `CUSTOM_IMAGE`).
    media_cover_raw: String,
    media_cover_key: Option<DynamicImageKey>,
    dynamic_textures: HashMap<DynamicImageKey, DynamicTexSlot>,
    dynamic_tex_tx: Sender<DynamicTexResult>,
    dynamic_tex_rx: Receiver<DynamicTexResult>,
    max_dynamic_tex_dim: u32,
}

/// Visão das instâncias para o `BindingSolver3d` (campos disjuntos do renderer).
struct SlotAccess<'a> {
    slots: &'a mut [ObjectSlot],
    objects: &'a [Scene3dObject],
}

impl InstanceAccess for SlotAccess<'_> {
    fn object_count(&self) -> usize {
        self.slots.len()
    }
    fn object_index(&self, object_id: &str) -> Option<usize> {
        self.objects.iter().position(|o| o.id == object_id)
    }
    fn instance(&self, object: usize) -> Option<&ModelInstance> {
        self.slots.get(object).and_then(|s| s.instance.as_ref())
    }
    fn instance_mut(&mut self, object: usize) -> Option<&mut ModelInstance> {
        self.slots.get_mut(object).and_then(|s| s.instance.as_mut())
    }
}

/// Raio × AABB (slabs). Distância de entrada (≥ 0) ou `None`.
fn ray_aabb(origin: Vec3, inv_dir: Vec3, b: &Aabb) -> Option<f32> {
    if b.is_empty() {
        return None;
    }
    let t1 = (b.min - origin) * inv_dir;
    let t2 = (b.max - origin) * inv_dir;
    let t_near = t1.min(t2).max_element().max(0.0);
    let t_far = t1.max(t2).min_element();
    (t_far.is_finite() && t_far >= t_near).then_some(t_near)
}

impl Scene3dRenderer {
    pub fn new(shared: Arc<Scene3dShared>, gpu: &GpuContext, options: Scene3dOptions) -> Self {
        let device = gpu.device();
        let sample_count = if options.msaa >= 4 { 4 } else { 1 };
        let pipelines = build_pbr_pipelines(&shared, device, sample_count);
        let tonemap_pipeline = build_tonemap_pipeline(&shared, device);

        let preset = Scene3dEnvPreset::default();
        let env = shared
            .baker
            .bake(device, gpu.queue(), EnvSource::Procedural(&ProceduralEnv::from_preset(preset)));
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_pbr_frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind = create_frame_bind(&shared, device, &frame_buffer, &env);
        let (draw_buffer, draw_bind) = create_draw_buffer(&shared, device, 16);
        let tonemap_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_tonemap"),
            size: std::mem::size_of::<TonemapUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (dynamic_tex_tx, dynamic_tex_rx) = mpsc::channel();
        let max_dynamic_tex_dim = device.limits().max_texture_dimension_2d.min(MAX_DYNAMIC_IMAGE_DIM);

        let this = Self {
            shared,
            sample_count,
            pipelines,
            spare_pipelines: None,
            msaa_cap: sample_count,
            tonemap_pipeline,
            config: Scene3dConfig::default(),
            objects: Vec::new(),
            graph: SceneGraph::default(),
            visible: Vec::new(),
            object_world: Vec::new(),
            slots: Vec::new(),
            asset_root: None,
            env,
            env_key: EnvKey {
                preset: Some(preset),
                hdri: None,
            },
            hdri_rx: None,
            waker: None,
            lights: LightRig::from_preset(preset),
            day_night_timer: 0.0,
            sky_background: preset_background(preset),
            frame_buffer,
            frame_bind,
            draw_buffer,
            draw_bind,
            draw_capacity: 16,
            tonemap_buffer,
            targets: None,
            output_generation: 0,
            viewport: (0, 0),
            render_scale_cap: 1.0,
            draw_staging: Vec::with_capacity(16),
            opaque: Vec::with_capacity(16),
            blended: Vec::with_capacity(16),
            orbit: [0.0; 2],
            pointer: None,
            look_pointer: None,
            dirty: true,
            animating: false,
            stats: Scene3dStats::default(),
            solver: BindingSolver3d::new(),
            signals: SignalSnapshot::default(),
            hover_pointer: None,
            media_cover_raw: String::new(),
            media_cover_key: None,
            dynamic_textures: HashMap::new(),
            dynamic_tex_tx,
            dynamic_tex_rx,
            max_dynamic_tex_dim,
        };
        this.write_tonemap(gpu.queue());
        this
    }

    // ── Configuração ──

    /// Pasta base para `modelSrc`/HDRI relativos (pacote `.prism` extraído).
    pub fn set_asset_root(&mut self, root: Option<PathBuf>) {
        self.asset_root = root;
    }

    /// Chamado de um thread de trabalho quando um HDRI termina de carregar (ex.: `LoopProxy::wake`).
    pub fn set_waker(&mut self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.waker = waker;
    }

    /// Tamanho da área da camada em pixels físicos. Os alvos são refeitos no próximo `render`.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        if self.viewport != (width, height) {
            self.viewport = (width, height);
            self.dirty = true;
        }
    }

    /// Teto de MSAA vindo do PowerGovernor (Normal = 4; Eco/Ultra = 1). O efetivo também cai para
    /// 1 acima de 2560×1440 de render (alvo MSAA HDR em 4K custa ~265 MB). Aplicado no `render`.
    pub fn set_msaa_cap(&mut self, samples: u32) {
        let samples = if samples >= 4 { 4 } else { 1 };
        if samples != self.msaa_cap {
            self.msaa_cap = samples;
            self.dirty = true;
        }
    }

    /// Troca o `sample_count` (pipelines + alvos) só quando o efetivo muda.
    fn apply_msaa(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        let wanted = if self.msaa_cap >= 4 && u64::from(size.0) * u64::from(size.1) <= MSAA_MAX_PIXELS {
            4
        } else {
            1
        };
        if wanted == self.sample_count {
            return;
        }
        let next = match self.spare_pipelines.take() {
            Some((count, pipelines)) if count == wanted => pipelines,
            _ => build_pbr_pipelines(&self.shared, device, wanted),
        };
        let previous = std::mem::replace(&mut self.pipelines, next);
        self.spare_pipelines = Some((self.sample_count, previous));
        self.sample_count = wanted;
        self.targets = None; // MSAA/profundidade com o novo sample_count
    }

    /// Teto de escala de render vindo do PowerGovernor (`renderScaleCap`).
    pub fn set_render_scale_cap(&mut self, cap: f32) {
        let cap = if cap.is_finite() { cap.clamp(0.25, 2.0) } else { 1.0 };
        if (cap - self.render_scale_cap).abs() > 1e-4 {
            self.render_scale_cap = cap;
            self.dirty = true;
        }
    }

    /// Cursor normalizado (0..1) para a órbita por parallax; `None` = sem cursor (volta ao centro).
    pub fn set_pointer(&mut self, pointer: Option<[f32; 2]>) {
        self.pointer = pointer.filter(|p| p[0].is_finite() && p[1].is_finite());
        self.look_pointer = None;
    }

    /// Widgets: `hover` = cursor local (só dentro da caixa, para hover/pick); `look` = cursor da
    /// tela inteira mapeado em 0..1 (o personagem olha para o mouse mesmo fora do widget).
    pub fn set_pointers(&mut self, hover: Option<[f32; 2]>, look: Option<[f32; 2]>) {
        let ok = |p: &[f32; 2]| p[0].is_finite() && p[1].is_finite();
        self.pointer = hover.filter(ok);
        self.look_pointer = look.filter(ok);
    }

    /// Há bindings `FORMULA` (o host reavalia quando o contexto FX muda).
    pub fn has_formulas(&self) -> bool {
        self.solver.has_formulas()
    }

    /// Reavalia as fórmulas dos bindings (`eval`: expressão → número). `true` = pede quadro.
    pub fn update_formulas(&mut self, eval: impl FnMut(&str) -> Option<f32>) -> bool {
        let changed = self.solver.update_formulas(eval);
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Aplica um novo `Scene3dConfig`. Objetos com o mesmo `id` + `modelSrc` mantêm a instância
    /// (e a animação); modelos novos são pedidos ao `loader` (o resultado chega por
    /// `handle_load_event`).
    pub fn set_config(&mut self, gpu: &GpuContext, config: Scene3dConfig, loader: &mut AssetLoader) {
        // Schema ≤2 (modelo único) já vira `obj-primary` aqui, como a migração do Estúdio.
        let objects = config.effective_objects();

        let mut previous: HashMap<String, ObjectSlot> = HashMap::with_capacity(self.slots.len());
        for slot in self.slots.drain(..) {
            previous.entry(slot.id.clone()).or_insert(slot);
        }
        let mut slots = Vec::with_capacity(objects.len());
        for obj in &objects {
            let slot_key = if obj.kind == Scene3dObjectKind::Showcase {
                ShowcaseVariant::parse(obj.showcase_variant.as_deref()).key()
            } else {
                obj.model_src.as_str()
            };
            let reused = previous
                .remove(&obj.id)
                .filter(|s| s.src == slot_key && s.kind == obj.kind);
            let slot = match reused {
                Some(mut slot) => {
                    if let Some(instance) = &mut slot.instance {
                        instance.set_playback(&obj.playback);
                    }
                    slot.dynamic_rules = self.resolve_dynamic_rules(obj);
                    slot
                }
                None => self.new_slot(gpu, obj, loader),
            };
            slots.push(slot);
        }
        self.slots = slots;

        self.graph = SceneGraph::resolve(&objects);
        self.visible = self.graph.effective_visibility(&objects);
        self.object_world = vec![Mat4::IDENTITY; objects.len()];
        self.objects = objects;

        self.solver.set_specs(&config.bindings);
        self.hover_pointer = None;

        self.config = config;
        self.sync_dynamic_textures(gpu);
        if self.config.auto_day_night {
            // Avalia já (hora fixa do Estúdio ou relógio local); o `update` reavalia a cada 30 s.
            let hour = self.day_night_hour();
            self.lights = LightRig::day_night(hour);
            self.sky_background = day_night_background(hour);
            self.day_night_timer = DAY_NIGHT_PERIOD;
        } else {
            self.lights = LightRig::from_preset(self.config.env_preset);
            self.sky_background = preset_background(self.config.env_preset);
            self.day_night_timer = 0.0;
        }
        self.update_environment(gpu);
        self.write_tonemap(gpu.queue());
        self.ensure_draw_capacity(gpu.device());
        self.dirty = true;
    }

    fn resolve_dynamic_rules(&self, obj: &Scene3dObject) -> Vec<SlotDynamicRule> {
        obj.parsed_dynamic_textures()
            .into_iter()
            .map(|rule| {
                let custom_key = if rule.source == Scene3dDynamicTextureSource::CustomImage {
                    rule.custom_src
                        .as_deref()
                        .and_then(|raw| self.resolve_path(raw))
                        .and_then(DynamicImageKey::from_path)
                } else {
                    None
                };
                SlotDynamicRule { rule, custom_key }
            })
            .collect()
    }

    fn new_slot(&self, gpu: &GpuContext, obj: &Scene3dObject, loader: &mut AssetLoader) -> ObjectSlot {
        let dynamic_rules = self.resolve_dynamic_rules(obj);
        if obj.kind == Scene3dObjectKind::Showcase {
            let variant = ShowcaseVariant::parse(obj.showcase_variant.as_deref());
            let asset = build_showcase_asset(variant);
            let model = self.shared.model(gpu, &asset);
            let instance = ModelInstance::new(gpu.device(), &self.shared.deform, model, &obj.playback);
            return ObjectSlot {
                id: obj.id.clone(),
                src: variant.key().to_owned(),
                kind: obj.kind,
                ticket: None,
                instance: Some(instance),
                error: None,
                spin: 0.0,
                dynamic_rules,
                material_overrides: Vec::new(),
            };
        }
        let mut slot = ObjectSlot {
            id: obj.id.clone(),
            src: obj.model_src.clone(),
            kind: obj.kind,
            ticket: None,
            instance: None,
            error: None,
            spin: 0.0,
            dynamic_rules,
            material_overrides: Vec::new(),
        };
        match self.resolve_path(&obj.model_src) {
            Some(path) => slot.ticket = Some(loader.request(path)),
            None => slot.error = Some(format!("modelSrc inválido: `{}`", obj.model_src)),
        }
        slot
    }

    fn resolve_path(&self, raw: &str) -> Option<PathBuf> {
        let raw = raw.trim();
        let remote = raw.contains("://") && !raw.starts_with("file://");
        if raw.is_empty() || raw.starts_with("blob:") || raw.starts_with("data:") || remote {
            return None;
        }
        let stripped = raw
            .strip_prefix("file:///")
            .or_else(|| raw.strip_prefix("file://"))
            .unwrap_or(raw);
        let path = Path::new(stripped);
        match &self.asset_root {
            Some(root) if !path.is_absolute() => Some(root.join(path)),
            _ => Some(path.to_path_buf()),
        }
    }

    /// Entrega um evento do `AssetLoader`. `true` se pertencia a esta cena.
    pub fn handle_load_event(&mut self, gpu: &GpuContext, event: &LoadEvent) -> bool {
        let shared = Arc::clone(&self.shared);
        match event {
            LoadEvent::Ready { ticket, asset, .. } => {
                let mut matched = false;
                for (index, slot) in self.slots.iter_mut().enumerate() {
                    if slot.ticket != Some(*ticket) {
                        continue;
                    }
                    let Some(obj) = self.objects.get(index) else {
                        continue;
                    };
                    let model = shared.model(gpu, asset);
                    slot.instance = Some(ModelInstance::new(gpu.device(), &shared.deform, model, &obj.playback));
                    slot.ticket = None;
                    slot.error = None;
                    rebuild_slot_overrides(
                        gpu.device(),
                        &shared.upload,
                        slot,
                        self.media_cover_key.as_ref(),
                        &self.dynamic_textures,
                    );
                    matched = true;
                }
                if matched {
                    self.ensure_draw_capacity(gpu.device());
                    self.solver.invalidate();
                    self.hover_pointer = None;
                    self.dirty = true;
                }
                matched
            }
            LoadEvent::Failed { ticket, error, .. } => {
                let mut matched = false;
                for slot in self.slots.iter_mut().filter(|s| s.ticket == Some(*ticket)) {
                    slot.ticket = None;
                    slot.error = Some(error.to_string());
                    matched = true;
                }
                matched
            }
        }
    }

    /// Erros por objeto (`id`, mensagem): modelo que falhou, SHOWCASE, caminho inválido.
    #[allow(dead_code)] // API reservada (ainda sem chamador)
    pub fn object_errors(&self) -> impl Iterator<Item = (&str, &str)> {
        self.slots
            .iter()
            .filter_map(|s| s.error.as_deref().map(|e| (s.id.as_str(), e)))
    }

    /// Algum modelo ainda carregando.
    #[allow(dead_code)] // API reservada (ainda sem chamador)
    pub fn is_loading(&self) -> bool {
        self.slots.iter().any(|s| s.ticket.is_some())
    }

    fn ensure_draw_capacity(&mut self, device: &wgpu::Device) {
        let total: usize = self
            .slots
            .iter()
            .filter_map(|s| s.instance.as_ref())
            .map(|i| i.draws.len())
            .sum();
        if total > self.draw_capacity {
            let capacity = total.next_power_of_two();
            let (buffer, bind) = create_draw_buffer(&self.shared, device, capacity);
            self.draw_buffer = buffer;
            self.draw_bind = bind;
            self.draw_capacity = capacity;
        }
        let cap = self.draw_capacity;
        for v in [&mut self.opaque, &mut self.blended] {
            if v.capacity() < cap {
                v.reserve_exact(cap - v.len());
            }
        }
        if self.draw_staging.capacity() < cap {
            self.draw_staging.reserve_exact(cap - self.draw_staging.len());
        }
    }

    // ── Ambiente ──

    fn update_environment(&mut self, gpu: &GpuContext) {
        let hdri = self
            .config
            .env_map_src
            .as_deref()
            .and_then(|raw| self.resolve_path(raw));
        let key = EnvKey {
            preset: Some(self.config.env_preset),
            hdri,
        };
        if key == self.env_key {
            return;
        }
        let preset_changed = key.preset != self.env_key.preset;
        let had_hdri = self.env_key.hdri.is_some();
        self.env_key = key.clone();
        self.hdri_rx = None;

        // Procedural já (também é o fallback enquanto o HDRI carrega ou se ele falhar).
        if preset_changed || had_hdri {
            let env = ProceduralEnv::from_preset(self.config.env_preset);
            self.install_environment(gpu, EnvSource::Procedural(&env));
        }
        if let Some(path) = key.hdri {
            let (tx, rx) = mpsc::channel::<HdriResult>();
            let waker = self.waker.clone();
            let spawned = std::thread::Builder::new()
                .name("prismview-hdri".into())
                .spawn(move || {
                    let result = load_hdri(&path);
                    let _ = tx.send((path, result));
                    if let Some(wake) = waker {
                        wake();
                    }
                });
            if spawned.is_ok() {
                self.hdri_rx = Some(rx);
            }
        }
    }

    fn install_environment(&mut self, gpu: &GpuContext, source: EnvSource<'_>) {
        self.env = self.shared.baker.bake(gpu.device(), gpu.queue(), source);
        self.frame_bind = create_frame_bind(&self.shared, gpu.device(), &self.frame_buffer, &self.env);
        self.dirty = true;
    }

    /// Atualiza a capa da mídia ativa (`MEDIA_COVER`). Só decodifica/sobe na GPU se o arquivo
    /// ou sua assinatura `(len, mtime)` mudou; devolve `true` se a cena precisa redesenhar já.
    pub fn set_media_cover(&mut self, gpu: &GpuContext, cover_path: &str) -> bool {
        let trimmed = cover_path.trim();
        let next_key = if trimmed.is_empty() {
            None
        } else {
            self.resolve_path(trimmed).and_then(DynamicImageKey::from_path)
        };
        if next_key == self.media_cover_key && trimmed == self.media_cover_raw {
            return false;
        }
        self.media_cover_raw.clear();
        self.media_cover_raw.push_str(trimmed);
        self.media_cover_key = next_key;
        if !self.config.uses_media_cover() {
            return false;
        }
        let changed = self.sync_dynamic_textures(gpu);
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Garante que todas as texturas dinâmicas referenciadas estão carregando ou prontas, descarta
    /// capas/imagens antigas do cache (mantendo a VRAM constante) e reconstrói overrides.
    fn sync_dynamic_textures(&mut self, gpu: &GpuContext) -> bool {
        let mut wanted: HashSet<DynamicImageKey> = HashSet::new();
        for slot in &self.slots {
            for entry in &slot.dynamic_rules {
                let key = match entry.rule.source {
                    Scene3dDynamicTextureSource::MediaCover => self.media_cover_key.clone(),
                    Scene3dDynamicTextureSource::CustomImage => entry.custom_key.clone(),
                };
                if let Some(k) = key {
                    wanted.insert(k);
                }
            }
        }
        self.dynamic_textures.retain(|k, _| wanted.contains(k));

        for key in wanted {
            if self.dynamic_textures.contains_key(&key) {
                continue;
            }
            self.dynamic_textures.insert(key.clone(), DynamicTexSlot::Loading);
            let thread_key = key.clone();
            let tx = self.dynamic_tex_tx.clone();
            let waker = self.waker.clone();
            let max_dim = self.max_dynamic_tex_dim;
            let spawned = std::thread::Builder::new()
                .name("prismview-dyntex".into())
                .spawn(move || {
                    let result = decode_dynamic_image(&thread_key.path, max_dim);
                    let _ = tx.send((thread_key, result));
                    if let Some(wake) = waker {
                        wake();
                    }
                });
            if spawned.is_err() {
                self.dynamic_textures.remove(&key);
            }
        }

        let mut any_changed = false;
        let device = gpu.device();
        for slot in &mut self.slots {
            any_changed |= rebuild_slot_overrides(
                device,
                &self.shared.upload,
                slot,
                self.media_cover_key.as_ref(),
                &self.dynamic_textures,
            );
        }
        any_changed
    }

    fn poll_dynamic_textures(&mut self, gpu: &GpuContext) {
        let mut any_ready = false;
        loop {
            match self.dynamic_tex_rx.try_recv() {
                Ok((key, result)) => {
                    if !self.dynamic_textures.contains_key(&key) {
                        continue; // capa/cena antiga já descartada
                    }
                    match result {
                        Ok(decoded) => {
                            if let Some((texture, view, vram_bytes)) = self.shared.upload.upload_rgba_srgb(
                                gpu.device(),
                                gpu.queue(),
                                decoded.width,
                                decoded.height,
                                &decoded.rgba,
                            ) {
                                self.dynamic_textures.insert(
                                    key,
                                    DynamicTexSlot::Ready {
                                        _texture: texture,
                                        view,
                                        vram_bytes,
                                    },
                                );
                                any_ready = true;
                            } else {
                                self.dynamic_textures.insert(key, DynamicTexSlot::Failed);
                            }
                        }
                        Err(err) => {
                            eprintln!("[prismview-engine] textura dinâmica {}: {err}", key.path.display());
                            self.dynamic_textures.insert(key, DynamicTexSlot::Failed);
                        }
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if any_ready {
            let device = gpu.device();
            for slot in &mut self.slots {
                rebuild_slot_overrides(
                    device,
                    &self.shared.upload,
                    slot,
                    self.media_cover_key.as_ref(),
                    &self.dynamic_textures,
                );
            }
            self.dirty = true;
        }
    }

    fn poll_hdri(&mut self, gpu: &GpuContext) {
        let Some(rx) = &self.hdri_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((path, result)) => {
                self.hdri_rx = None;
                if self.env_key.hdri.as_deref() != Some(path.as_path()) {
                    return; // pedido antigo
                }
                match result {
                    Ok(image) => self.install_environment(gpu, EnvSource::Equirect(&image, 1.0)),
                    Err(e) => eprintln!("[prismview-engine] HDRI {}: {e}", path.display()),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.hdri_rx = None,
        }
    }

    fn write_tonemap(&self, queue: &wgpu::Queue) {
        let exposure = if self.config.exposure.is_finite() && self.config.exposure > 0.0 {
            self.config.exposure
        } else {
            1.05
        };
        // Fundo opaco composto por baixo (sem tone mapping, como `scene.background = Color`);
        // TRANSPARENT = alpha 0 fora dos modelos. A IBL não depende do modo.
        let bg = match self.config.background_mode {
            Scene3dBackgroundMode::Transparent => None,
            Scene3dBackgroundMode::Solid => Some(
                self.config
                    .background_color
                    .as_deref()
                    .filter(|c| !c.trim().is_empty())
                    .and_then(parse_hex_srgb)
                    .unwrap_or_else(|| parse_hex_srgb("#07080b").unwrap_or([0.0, 0.0, 0.0, 1.0])),
            ),
            Scene3dBackgroundMode::Environment => {
                let sky = self.sky_background;
                Some([linear_to_srgb(sky.x), linear_to_srgb(sky.y), linear_to_srgb(sky.z), 1.0])
            }
        };
        let bg_rgb = bg.unwrap_or([0.0; 4]);
        let uniform = TonemapUniform {
            info: [exposure, if bg.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0],
            background: [bg_rgb[0], bg_rgb[1], bg_rgb[2], 1.0],
        };
        queue.write_buffer(&self.tonemap_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// Hora do ciclo dia/noite: `dayNightHourOverride` do Estúdio ou o relógio local.
    fn day_night_hour(&self) -> f32 {
        self.config.hour_override().unwrap_or_else(local_hour)
    }

    // ── Quadro ──

    /// Avança o tempo da cena. Devolve `true` se um quadro precisa ser desenhado.
    pub fn update(&mut self, gpu: &GpuContext, dt: f32) -> bool {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        self.poll_hdri(gpu);
        self.poll_dynamic_textures(gpu);

        if self.config.auto_day_night {
            self.day_night_timer -= dt;
            if self.day_night_timer <= 0.0 {
                self.day_night_timer = DAY_NIGHT_PERIOD;
                let hour = self.day_night_hour();
                let rig = LightRig::day_night(hour);
                if rig != self.lights {
                    self.lights = rig;
                    self.dirty = true;
                }
                let sky = day_night_background(hour);
                if sky != self.sky_background {
                    self.sky_background = sky;
                    if self.config.background_mode == Scene3dBackgroundMode::Environment {
                        self.write_tonemap(gpu.queue());
                        self.dirty = true;
                    }
                }
            }
        }

        let mut animating = false;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            // Oculto (ele ou um ancestral): o tempo anda, mas não pede quadro.
            let shown = self.visible.get(index).copied().unwrap_or(false);
            let speed = self.objects.get(index).map_or(0.0, |o| o.auto_rotate_speed);
            if speed.is_finite() && speed.abs() > 1e-4 {
                slot.spin = (slot.spin + speed * dt).rem_euclid(std::f32::consts::TAU);
                animating |= shown;
            } else if slot.spin != 0.0 {
                slot.spin = 0.0;
                self.dirty |= shown;
            }
            if let Some(instance) = &mut slot.instance {
                if instance.advance(dt) {
                    self.dirty |= shown;
                }
                animating |= shown && instance.is_animating();
            }
        }

        // Órbita por cursor com amortecimento exponencial (mesma fórmula do Estúdio).
        let cam = &self.config.camera;
        let use_cursor = cam.orbit_parallax_x.abs() > 1e-3 || cam.orbit_parallax_y.abs() > 1e-3;
        let look = self.look_pointer.or(self.pointer);
        let (cx, cy) = match (use_cursor, look) {
            (true, Some(p)) => ((p[0] - 0.5) * 2.0, (p[1] - 0.5) * 2.0),
            _ => (0.0, 0.0),
        };
        let target = [cx * cam.orbit_parallax_x, -cy * cam.orbit_parallax_y];
        let damp = if cam.damping > 0.0 { cam.damping.clamp(0.02, 0.4) } else { 0.08 };
        let alpha = 1.0 - (-damp * 60.0 * dt).exp();
        let mut settled = true;
        for axis in 0..2 {
            let delta = target[axis] - self.orbit[axis];
            if delta.abs() >= 0.01 {
                settled = false;
                self.orbit[axis] += delta * alpha;
                self.dirty = true;
            } else if delta != 0.0 {
                self.orbit[axis] = target[axis];
                self.dirty = true;
            }
        }
        if !settled {
            animating = true;
        }

        // Bindings reativos: desfaz o delta anterior → (clipes já avançados acima) → reaplica.
        if !self.solver.is_empty() {
            if self.solver.wants_hover() && (self.pointer != self.hover_pointer || !settled) {
                self.hover_pointer = self.pointer;
                let hit = self.pointer.and_then(|p| self.pick(p));
                self.solver.set_hover(hit);
            }
            let mut access = SlotAccess {
                slots: &mut self.slots,
                objects: &self.objects,
            };
            if self.solver.solve(&mut access, &self.signals, look, dt) {
                self.dirty = true;
            }
            animating |= self.solver.is_animating();
            // Override de clipe (PLAY_CLIP) iniciado agora também pede quadros.
            animating |= self
                .slots
                .iter()
                .any(|s| s.instance.as_ref().is_some_and(ModelInstance::is_animating));
        }

        self.animating = animating;
        self.dirty || animating
    }

    /// Snapshot de sinais do quadro (copiado; ~400 B). Chamar antes de `update`.
    pub fn set_signals(&mut self, snap: &SignalSnapshot) {
        self.signals = *snap;
    }

    /// Sinais que os bindings e texturas dinâmicas desta cena consomem (para `SignalBus::set_demand`).
    pub fn signal_demand(&self) -> SignalKind {
        let media = if self.config.uses_media_cover() {
            SignalKind::MEDIA
        } else {
            SignalKind::NONE
        };
        self.solver.demand() | media
    }

    /// Clique normalizado (0..1 sobre a camada) para bindings `CLICK_MESH`. `true` se disparou.
    pub fn click(&mut self, pointer: [f32; 2]) -> bool {
        if !self.solver.wants_click() {
            return false;
        }
        let Some(hit) = self.pick(pointer) else {
            return false;
        };
        let mut access = SlotAccess {
            slots: &mut self.slots,
            objects: &self.objects,
        };
        let fired = self.solver.click(&mut access, hit);
        if fired {
            self.dirty = true;
        }
        fired
    }

    /// Objeto/nó sob o cursor normalizado (raio da câmera × AABB de mundo de cada draw). Usa as
    /// matrizes do último `render`. Sem alocação.
    pub fn pick(&self, pointer: [f32; 2]) -> Option<PickHit> {
        if !pointer[0].is_finite() || !pointer[1].is_finite() {
            return None;
        }
        let (w, h) = self.viewport;
        if w == 0 || h == 0 {
            return None;
        }
        let aspect = w as f32 / h as f32;
        let (eye, target, fov) = self.camera_view();
        let fwd = (target - eye).normalize_or_zero();
        let right = fwd.cross(camera_up_ref(fwd)).normalize_or_zero();
        if fwd == Vec3::ZERO || right == Vec3::ZERO {
            return None;
        }
        let up = right.cross(fwd);
        let t = (fov.to_radians() * 0.5).tan();
        let nx = pointer[0] * 2.0 - 1.0;
        let ny = 1.0 - pointer[1] * 2.0;
        let dir = (fwd + right * (nx * t * aspect) + up * (ny * t)).normalize_or_zero();
        let safe = |v: f32| if v.abs() < 1e-9 { 1e-9_f32.copysign(v) } else { v };
        let inv = Vec3::new(1.0 / safe(dir.x), 1.0 / safe(dir.y), 1.0 / safe(dir.z));

        let mut best: Option<(f32, PickHit)> = None;
        for (oi, slot) in self.slots.iter().enumerate() {
            let Some(instance) = &slot.instance else {
                continue;
            };
            if !self.visible.get(oi).copied().unwrap_or(false) {
                continue;
            }
            let object = self.object_world.get(oi).copied().unwrap_or(Mat4::IDENTITY);
            let base = object * instance.root_matrix() * instance.normalize;
            for draw in &instance.draws {
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let (model, bounds) = if draw.skinned {
                    (base, instance.skinned_bounds)
                } else {
                    (base * instance.node_world(draw.node), prim.bounds)
                };
                let Some(dist) = ray_aabb(eye, inv, &bounds.transformed(&model)) else {
                    continue;
                };
                if best.map_or(true, |(d, _)| dist < d) {
                    best = Some((dist, PickHit { object: oi, node: draw.node }));
                }
            }
        }
        best.map(|(_, hit)| hit)
    }

    /// `true` se o próximo `render` produziria algo novo.
    pub fn needs_frame(&self) -> bool {
        self.dirty || self.animating
    }

    /// Intervalo entre quadros pelo teto de FPS da cena (`None` = sincronia do monitor).
    pub fn frame_interval(&self) -> Option<Duration> {
        if self.config.fps_mode.eq_ignore_ascii_case("MONITOR") {
            return None;
        }
        let fps = self.config.max_fps.clamp(5, 200);
        Some(Duration::from_secs_f64(1.0 / f64::from(fps)))
    }

    /// Quando acordar se nada estiver animando (ciclo dia/noite). `None` = só por evento.
    pub fn idle_wake(&self) -> Option<Duration> {
        if self.animating {
            return self.frame_interval().or(Some(Duration::from_millis(16)));
        }
        self.config
            .auto_day_night
            .then(|| Duration::from_secs_f32(self.day_night_timer.max(1.0)))
    }

    pub fn stats(&self) -> Scene3dStats {
        self.stats
    }

    /// Textura final (RGBA8, sRGB pré-multiplicado) para o compositor. `None` antes do 1º render.
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.output_view)
    }

    /// Textura de saída (o compositor cria a própria view dela: backdrop do vidro).
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.output)
    }

    /// Muda sempre que `output_view` passa a apontar para uma textura nova.
    pub fn output_generation(&self) -> u64 {
        self.output_generation
    }

    /// Órbita por cursor configurada (o loop precisa do mouse mesmo sem bindings).
    pub fn wants_cursor(&self) -> bool {
        let cam = &self.config.camera;
        cam.orbit_parallax_x.abs() > 1e-3 || cam.orbit_parallax_y.abs() > 1e-3
    }

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| t.size)
    }

    /// Hibernação de VRAM (Fase 5): solta MSAA/HDR/profundidade/saída. O próximo `render`
    /// recria tudo (`dirty`), mantendo cena, modelos e ambiente.
    pub fn release_targets(&mut self) {
        self.targets = None;
        self.dirty = true;
    }

    fn render_size(&self) -> (u32, u32) {
        let (w, h) = self.viewport;
        let configured = if self.config.render_scale.is_finite() && self.config.render_scale > 0.0 {
            self.config.render_scale
        } else {
            0.85
        };
        let scale = configured.min(self.render_scale_cap).max(0.5);
        let max = 8192.0;
        (
            ((w as f32 * scale).round().min(max) as u32).max(1),
            ((h as f32 * scale).round().min(max) as u32).max(1),
        )
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        self.targets = None; // solta os antigos antes de alocar os novos (pico de VRAM menor)
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let make = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let attach = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = attach | wgpu::TextureUsages::TEXTURE_BINDING;
        let msaa = (self.sample_count > 1).then(|| make("prismview_3d_msaa", HDR_FORMAT, self.sample_count, attach));
        let hdr = make("prismview_3d_hdr", HDR_FORMAT, 1, sampled);
        let depth = make("prismview_3d_depth", DEPTH_FORMAT, self.sample_count, attach);
        let output = make("prismview_3d_output", OUTPUT_FORMAT, 1, sampled);
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let hdr_view = view(&hdr);
        let tonemap_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_tonemap"),
            layout: &self.shared.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.tonemap_buffer.as_entire_binding(),
                },
            ],
        });
        self.targets = Some(Targets {
            size,
            msaa_view: msaa.as_ref().map(view),
            _msaa: msaa,
            hdr_view,
            _hdr: hdr,
            depth_view: view(&depth),
            _depth: depth,
            output_view: view(&output),
            output,
            tonemap_bind,
        });
        self.output_generation = self.output_generation.wrapping_add(1);
    }

    /// Olho, alvo e FOV da câmera do wallpaper (órbita aplicada).
    fn camera_view(&self) -> (Vec3, Vec3, f32) {
        let cam = &self.config.camera;
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let target = match cam.target {
            Some(t) => Vec3::new(finite(t[0], 0.0), finite(t[1], finite(cam.target_y, 0.0)), finite(t[2], 0.0)),
            None => Vec3::new(0.0, finite(cam.target_y, 0.0), 0.0),
        };
        let az = (finite(cam.azimuth_deg, 0.0) + self.orbit[0]).to_radians();
        let el = (finite(cam.elevation_deg, 0.0) + self.orbit[1]).clamp(-80.0, 80.0).to_radians();
        let dist = if cam.distance.is_finite() && cam.distance != 0.0 { cam.distance } else { 4.8 }.max(0.3);
        let fov = if cam.fov.is_finite() && cam.fov != 0.0 { cam.fov } else { 42.0 }.clamp(20.0, 85.0);
        let eye = target + Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos()) * dist;
        (eye, target, fov)
    }

    fn frame_uniform(&self, view_proj: &Mat4, eye: Vec3) -> FrameUniform {
        let mut u = FrameUniform::zeroed();
        u.view_proj = view_proj.to_cols_array();
        u.camera_pos = eye.extend(1.0).to_array();
        u.ambient = self.lights.ambient.extend(0.0).to_array();
        let env_intensity = if self.config.env_intensity.is_finite() {
            self.config.env_intensity.clamp(0.0, 3.0)
        } else {
            1.2
        };
        u.env = [
            self.env.max_mip,
            self.lights.dir_count as f32,
            self.lights.point_count as f32,
            env_intensity,
        ];
        for (dst, l) in u.dir.iter_mut().zip(self.lights.dir.iter()).take(self.lights.dir_count) {
            let d = l.direction;
            let c = l.radiance;
            *dst = [d.x, d.y, d.z, 0.0, c.x, c.y, c.z, 0.0];
        }
        for (dst, l) in u.point.iter_mut().zip(self.lights.point.iter()).take(self.lights.point_count) {
            let p = l.position;
            let c = l.radiance;
            *dst = [p.x, p.y, p.z, l.range, c.x, c.y, c.z, l.decay];
        }
        u
    }

    /// Grava o quadro em `encoder` (deformação + PBR + tone mapping). `false` se não havia o que
    /// desenhar (viewport 0). O chamador submete o encoder.
    pub fn render(&mut self, gpu: &GpuContext, encoder: &mut wgpu::CommandEncoder) -> bool {
        if self.viewport.0 == 0 || self.viewport.1 == 0 {
            return false;
        }
        let device = gpu.device();
        let queue = gpu.queue();
        let size = self.render_size();
        self.apply_msaa(device, size);
        self.ensure_targets(device, size);

        // 1. Matrizes de objeto (pai antes do filho).
        for &i in &self.graph.order {
            let (Some(obj), Some(slot)) = (self.objects.get(i), self.slots.get(i)) else {
                continue;
            };
            let local = obj.local_matrix_with_spin(slot.spin);
            let parent = self.graph.parent.get(i).copied().flatten();
            let world = match parent.and_then(|p| self.object_world.get(p)) {
                Some(pw) => *pw * local,
                None => local,
            };
            if let Some(dst) = self.object_world.get_mut(i) {
                *dst = world;
            }
        }

        // 2-3. Pose e deformação na GPU (só o que mudou).
        let mut any_deform = false;
        for slot in &mut self.slots {
            if let Some(instance) = &mut slot.instance {
                instance.refresh_pose();
                any_deform |= instance.needs_deform();
            }
        }
        if any_deform {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("prismview_3d_deform"),
                timestamp_writes: None,
            });
            for slot in &mut self.slots {
                if let Some(instance) = &mut slot.instance {
                    instance.encode_deform(queue, &self.shared.deform, &mut pass);
                }
            }
        }

        // 4. Câmera (reverse-Z infinito).
        let (eye, target, fov) = self.camera_view();
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let proj = Mat4::perspective_infinite_reverse_rh(fov.to_radians(), aspect, NEAR_PLANE);
        let view = Mat4::look_at_rh(eye, target, camera_up_ref((target - eye).normalize_or_zero()));
        let view_proj = proj * view;
        let frustum = Frustum::from_view_proj(&view_proj);

        // 5. Lista de draws com culling (sem alocar: capacidade reservada na troca de cena).
        self.draw_staging.clear();
        self.opaque.clear();
        self.blended.clear();
        let mut culled = 0u32;
        let mut vram: u64 = self
            .dynamic_textures
            .values()
            .map(|entry| match entry {
                DynamicTexSlot::Ready { vram_bytes, .. } => *vram_bytes,
                DynamicTexSlot::Loading | DynamicTexSlot::Failed => 0,
            })
            .sum();
        let mut instances = 0u32;
        let capacity = self.draw_staging.capacity().min(self.draw_capacity);
        for (oi, slot) in self.slots.iter().enumerate() {
            let Some(instance) = &slot.instance else {
                continue;
            };
            instances += 1;
            vram += instance.model.vram_bytes;
            if !self.visible.get(oi).copied().unwrap_or(false) {
                continue;
            }
            let object = self.object_world.get(oi).copied().unwrap_or(Mat4::IDENTITY);
            // `__ROOT__` dos bindings: em torno do centro do modelo normalizado.
            let base = object * instance.root_matrix() * instance.normalize;
            for (di, draw) in instance.draws.iter().enumerate() {
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let (model, local_bounds) = if draw.skinned {
                    (base, instance.skinned_bounds)
                } else {
                    (base * instance.node_world(draw.node), prim.bounds)
                };
                let world_bounds = local_bounds.transformed(&model);
                if !frustum.intersects_aabb(&world_bounds) {
                    culled += 1;
                    continue;
                }
                if self.draw_staging.len() >= capacity {
                    break;
                }
                let Some(material) = slot.material(instance, prim.material) else {
                    continue;
                };
                let m3 = Mat3::from_mat4(model);
                let det = m3.determinant();
                let normal = if det.abs() > 1e-12 && det.is_finite() {
                    Mat4::from_mat3(m3.inverse().transpose())
                } else {
                    model
                };
                let slot_index = u32::try_from(self.draw_staging.len()).unwrap_or(u32::MAX);
                self.draw_staging.push(DrawGpu {
                    model: model.to_cols_array(),
                    normal: normal.to_cols_array(),
                    glow: [instance.glow(prim.material), 0.0, 0.0, 0.0],
                });
                // Espelhado (det < 0) inverte o winding: usa a variante sem culling.
                let double = material.double_sided || det < 0.0;
                let variant = u8::from(material.blend) * 2 + u8::from(double);
                let center = (world_bounds.min + world_bounds.max) * 0.5;
                let depth = view.transform_point3(center).z; // RH: mais longe = mais negativo
                let cmd = DrawCmd {
                    key: (u64::from(variant) << 56)
                        | ((oi as u64 & 0xffff) << 40)
                        | (u64::from(prim.material & 0xffff) << 24)
                        | u64::from(draw.prim & 0xff_ffff),
                    depth,
                    object: u32::try_from(oi).unwrap_or(u32::MAX),
                    draw: u32::try_from(di).unwrap_or(u32::MAX),
                    slot: slot_index,
                    variant,
                };
                if material.blend {
                    self.blended.push(cmd);
                } else {
                    self.opaque.push(cmd);
                }
            }
        }
        // Sort instável = in-place, sem alocação.
        self.opaque.sort_unstable_by_key(|c| c.key);
        self.blended.sort_unstable_by(|a, b| a.depth.total_cmp(&b.depth));

        // 6. Uploads do quadro.
        if !self.draw_staging.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, bytemuck::cast_slice(&self.draw_staging));
        }
        let frame = self.frame_uniform(&view_proj, eye);
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));

        let Some(targets) = &self.targets else {
            return false;
        };

        // 7. Passe PBR (HDR).
        {
            let (color_view, resolve) = match &targets.msaa_view {
                Some(msaa) => (msaa, Some(&targets.hdr_view)),
                None => (&targets.hdr_view, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismview_3d_pbr"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: if resolve.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0), // reverse-Z
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.set_bind_group(1, &self.draw_bind, &[]);
            let mut current_variant = u8::MAX;
            for cmd in self.opaque.iter().chain(self.blended.iter()) {
                let Some(slot) = self.slots.get(cmd.object as usize) else {
                    continue;
                };
                let Some(instance) = slot.instance.as_ref() else {
                    continue;
                };
                let Some(draw) = instance.draws.get(cmd.draw as usize) else {
                    continue;
                };
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let Some(material) = slot.material(instance, prim.material) else {
                    continue;
                };
                if cmd.variant != current_variant {
                    let Some(pipeline) = self.pipelines.get(cmd.variant as usize) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    current_variant = cmd.variant;
                }
                pass.set_bind_group(2, &material.bind_group, &[]);
                pass.set_vertex_buffer(0, instance.position_slice(draw, prim));
                pass.set_vertex_buffer(1, instance.model.attr.slice(u64::from(prim.first_vertex) * ATTR_STRIDE..));
                pass.set_index_buffer(instance.model.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    prim.first_index..prim.first_index + prim.index_count,
                    0,
                    cmd.slot..cmd.slot + 1,
                );
            }
        }

        // 8. Tone mapping → saída RGBA8.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismview_3d_tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.output_view,
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
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &targets.tonemap_bind, &[]);
            pass.draw(0..3, 0..1);
        }

        self.stats = Scene3dStats {
            drawn: u32::try_from(self.draw_staging.len()).unwrap_or(u32::MAX),
            culled,
            instances,
            vram_bytes: vram,
        };
        self.dirty = false;
        true
    }
}

// ───────────────────────── Construção de recursos ─────────────────────────

fn build_pbr_pipelines(shared: &Scene3dShared, device: &wgpu::Device, sample_count: u32) -> [wgpu::RenderPipeline; 4] {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("prismview_pbr"),
        bind_group_layouts: &[&shared.frame_layout, &shared.draw_layout, &shared.upload.material_layout],
        push_constant_ranges: &[],
    });
    let buffers = vertex_layouts();
    let make = |variant: usize| {
        let blend = variant >= 2;
        let double = variant % 2 == 1;
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prismview_pbr"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shared.pbr_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shared.pbr_module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: blend.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: if double { None } else { Some(wgpu::Face::Back) },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: !blend,
                depth_compare: wgpu::CompareFunction::GreaterEqual, // reverse-Z
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    };
    [make(0), make(1), make(2), make(3)]
}

fn build_tonemap_pipeline(shared: &Scene3dShared, device: &wgpu::Device) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("prismview_tonemap"),
        bind_group_layouts: &[&shared.tonemap_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("prismview_tonemap"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shared.tonemap_module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shared.tonemap_module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OUTPUT_FORMAT,
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

fn create_frame_bind(
    shared: &Scene3dShared,
    device: &wgpu::Device,
    frame_buffer: &wgpu::Buffer,
    env: &EnvironmentMaps,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("prismview_pbr_frame"),
        layout: &shared.frame_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&env.specular_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&env.irradiance_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&shared.brdf_lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&shared.env_sampler),
            },
        ],
    })
}

fn create_draw_buffer(shared: &Scene3dShared, device: &wgpu::Device, capacity: usize) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("prismview_pbr_draws"),
        size: capacity.max(1) as u64 * DRAW_STRIDE,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("prismview_pbr_draws"),
        layout: &shared.draw_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    (buffer, bind)
}

/// Hora local fracionária (0..24) para o ciclo dia/noite.
fn local_hour() -> f32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() as f32 + now.minute() as f32 / 60.0 + now.second() as f32 / 3600.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_caps_dynamic_image_and_tracks_file_changes() {
        let dir = std::env::temp_dir().join(format!("prismview_dyntex_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cover.png");

        let img = image::RgbaImage::from_pixel(16, 8, image::Rgba([220, 40, 90, 255]));
        img.save(&path).expect("salva png de teste");

        let key1 = DynamicImageKey::from_path(path.clone()).expect("key inicial");
        let decoded = decode_dynamic_image(&path, 8).expect("decodifica e reduz");
        assert_eq!(decoded.width, 8);
        assert_eq!(decoded.height, 4);
        assert_eq!(decoded.rgba.len(), 8 * 4 * 4);

        let img2 = image::RgbaImage::from_pixel(24, 24, image::Rgba([10, 200, 120, 255]));
        img2.save(&path).expect("sobrescreve png");
        let key2 = DynamicImageKey::from_path(path.clone()).expect("key atualizada");
        assert_ne!(key1, key2);

        let empty_path = dir.join("empty.png");
        std::fs::write(&empty_path, b"").expect("arquivo vazio");
        assert!(DynamicImageKey::from_path(empty_path.clone()).is_none());
        assert!(decode_dynamic_image(&empty_path, 64).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// "Up" de referência da câmera: `Y`, ou `Z` quando o olhar é (quase) vertical — senão
/// `fwd × Y` zera e a view/o pick degeneram (câmera a ±90°).
fn camera_up_ref(fwd: Vec3) -> Vec3 {
    if fwd.dot(Vec3::Y).abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

#[cfg(test)]
mod camera_up_tests {
    use super::*;

    #[test]
    fn vertical_view_still_has_a_right_vector() {
        for fwd in [Vec3::NEG_Y, Vec3::Y, Vec3::new(0.3, -0.9, 0.1).normalize()] {
            assert!(fwd.cross(camera_up_ref(fwd)).length() > 1e-3);
        }
    }
}
