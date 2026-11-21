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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};

use super::config::{Scene3dBackgroundMode, Scene3dConfig, Scene3dEnvPreset, Scene3dObject, Scene3dObjectKind};
use super::culling::Frustum;
use super::environment::{load_hdri, EnvBaker, EnvSource, EnvironmentMaps, EquirectImage};
use super::gpu_model::{vertex_layouts, GpuModel, ModelUploadContext, ATTR_STRIDE};
use super::graph::SceneGraph;
use super::instance::{DeformKernel, ModelInstance};
use super::lighting::{parse_hex_srgb, LightRig, ProceduralEnv, MAX_DIR_LIGHTS, MAX_POINT_LIGHTS};
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

struct ObjectSlot {
    id: String,
    src: String,
    kind: Scene3dObjectKind,
    ticket: Option<Ticket>,
    instance: Option<ModelInstance>,
    error: Option<String>,
    /// Giro acumulado de `autoRotateSpeed` (rad).
    spin: f32,
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
    _output: wgpu::Texture,
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

pub struct Scene3dRenderer {
    shared: Arc<Scene3dShared>,
    sample_count: u32,
    /// [opaco, opaco double-sided, blend, blend double-sided]
    pipelines: [wgpu::RenderPipeline; 4],
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

    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    draw_bind: wgpu::BindGroup,
    draw_capacity: usize,
    tonemap_buffer: wgpu::Buffer,
    targets: Option<Targets>,
    viewport: (u32, u32),
    render_scale_cap: f32,

    // Scratch do quadro (capacidade fixa entre trocas de cena).
    draw_staging: Vec<DrawGpu>,
    opaque: Vec<DrawCmd>,
    blended: Vec<DrawCmd>,

    orbit: [f32; 2],
    pointer: Option<[f32; 2]>,
    dirty: bool,
    animating: bool,
    stats: Scene3dStats,
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

        let mut this = Self {
            shared,
            sample_count,
            pipelines,
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
            frame_buffer,
            frame_bind,
            draw_buffer,
            draw_bind,
            draw_capacity: 16,
            tonemap_buffer,
            targets: None,
            viewport: (0, 0),
            render_scale_cap: 1.0,
            draw_staging: Vec::with_capacity(16),
            opaque: Vec::with_capacity(16),
            blended: Vec::with_capacity(16),
            orbit: [0.0; 2],
            pointer: None,
            dirty: true,
            animating: false,
            stats: Scene3dStats::default(),
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
    }

    /// Aplica um novo `Scene3dConfig`. Objetos com o mesmo `id` + `modelSrc` mantêm a instância
    /// (e a animação); modelos novos são pedidos ao `loader` (o resultado chega por
    /// `handle_load_event`).
    pub fn set_config(&mut self, gpu: &GpuContext, config: Scene3dConfig, loader: &mut AssetLoader) {
        let mut objects = config.effective_objects();
        if config.objects.is_empty() {
            // `modelSrc` legado: escala/offset/rotação/playback globais viram do objeto sintético.
            if let Some(obj) = objects.first_mut() {
                let s = if config.model_scale.is_finite() && config.model_scale > 0.0 {
                    config.model_scale
                } else {
                    1.0
                };
                obj.scale = [s; 3];
                obj.position[1] = config.model_offset_y;
                obj.auto_rotate_speed = config.auto_rotate_speed;
                obj.playback = config.playback.clone();
            }
        }

        let mut previous: HashMap<String, ObjectSlot> = HashMap::with_capacity(self.slots.len());
        for slot in self.slots.drain(..) {
            previous.entry(slot.id.clone()).or_insert(slot);
        }
        let mut slots = Vec::with_capacity(objects.len());
        for obj in &objects {
            let reused = previous
                .remove(&obj.id)
                .filter(|s| s.src == obj.model_src && s.kind == obj.kind);
            let slot = match reused {
                Some(mut slot) => {
                    if let Some(instance) = &mut slot.instance {
                        instance.set_playback(&obj.playback);
                    }
                    slot
                }
                None => self.new_slot(obj, loader),
            };
            slots.push(slot);
        }
        self.slots = slots;

        self.graph = SceneGraph::resolve(&objects);
        self.visible = self.graph.effective_visibility(&objects);
        self.object_world = vec![Mat4::IDENTITY; objects.len()];
        self.objects = objects;

        let preset_changed = self.config.env_preset != config.env_preset;
        let day_night_changed = self.config.auto_day_night != config.auto_day_night;
        self.config = config;
        if preset_changed || day_night_changed || !self.config.auto_day_night {
            self.lights = LightRig::from_preset(self.config.env_preset);
            self.day_night_timer = 0.0; // reavalia o dia/noite no próximo update
        }
        self.update_environment(gpu);
        self.write_tonemap(gpu.queue());
        self.ensure_draw_capacity(gpu.device());
        self.dirty = true;
    }

    fn new_slot(&self, obj: &Scene3dObject, loader: &mut AssetLoader) -> ObjectSlot {
        let mut slot = ObjectSlot {
            id: obj.id.clone(),
            src: obj.model_src.clone(),
            kind: obj.kind,
            ticket: None,
            instance: None,
            error: None,
            spin: 0.0,
        };
        if obj.kind == Scene3dObjectKind::Showcase {
            slot.error = Some("escultura procedural (SHOWCASE) ainda não existe no motor nativo".into());
            return slot;
        }
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
                    matched = true;
                }
                if matched {
                    self.ensure_draw_capacity(gpu.device());
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
    pub fn object_errors(&self) -> impl Iterator<Item = (&str, &str)> {
        self.slots
            .iter()
            .filter_map(|s| s.error.as_deref().map(|e| (s.id.as_str(), e)))
    }

    /// Algum modelo ainda carregando.
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
        let solid = self.config.background_mode == Scene3dBackgroundMode::Solid;
        let bg = self
            .config
            .background_color
            .as_deref()
            .and_then(parse_hex_srgb)
            .unwrap_or_else(|| parse_hex_srgb("#07080b").unwrap_or([0.0, 0.0, 0.0, 1.0]));
        let uniform = TonemapUniform {
            info: [exposure, if solid { 1.0 } else { 0.0 }, 0.0, 0.0],
            background: [bg[0], bg[1], bg[2], 1.0],
        };
        queue.write_buffer(&self.tonemap_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    // ── Quadro ──

    /// Avança o tempo da cena. Devolve `true` se um quadro precisa ser desenhado.
    pub fn update(&mut self, gpu: &GpuContext, dt: f32) -> bool {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        self.poll_hdri(gpu);

        if self.config.auto_day_night {
            self.day_night_timer -= dt;
            if self.day_night_timer <= 0.0 {
                self.day_night_timer = DAY_NIGHT_PERIOD;
                let rig = LightRig::day_night(local_hour());
                if rig != self.lights {
                    self.lights = rig;
                    self.dirty = true;
                }
            }
        }

        let mut animating = false;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let speed = self.objects.get(index).map_or(0.0, |o| o.auto_rotate_speed);
            if speed.is_finite() && speed.abs() > 1e-4 {
                slot.spin = (slot.spin + speed * dt).rem_euclid(std::f32::consts::TAU);
                animating = true;
            } else if slot.spin != 0.0 {
                slot.spin = 0.0;
                self.dirty = true;
            }
            if let Some(instance) = &mut slot.instance {
                if instance.advance(dt) {
                    self.dirty = true;
                }
                animating |= instance.is_animating();
            }
        }

        // Órbita por cursor com amortecimento exponencial (mesma fórmula do Estúdio).
        let cam = &self.config.camera;
        let use_cursor = cam.orbit_parallax_x.abs() > 1e-3 || cam.orbit_parallax_y.abs() > 1e-3;
        let (cx, cy) = match (use_cursor, self.pointer) {
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

        self.animating = animating;
        self.dirty || animating
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

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| t.size)
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
            _output: output,
            tonemap_bind,
        });
    }

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
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        let view_proj = proj * view;
        let frustum = Frustum::from_view_proj(&view_proj);

        // 5. Lista de draws com culling (sem alocar: capacidade reservada na troca de cena).
        self.draw_staging.clear();
        self.opaque.clear();
        self.blended.clear();
        let mut culled = 0u32;
        let mut vram = 0u64;
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
            let base = object * instance.normalize;
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
                let Some(material) = instance.model.materials.get(prim.material as usize) else {
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
                let Some(instance) = self.slots.get(cmd.object as usize).and_then(|s| s.instance.as_ref()) else {
                    continue;
                };
                let Some(draw) = instance.draws.get(cmd.draw as usize) else {
                    continue;
                };
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let Some(material) = instance.model.materials.get(prim.material as usize) else {
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
