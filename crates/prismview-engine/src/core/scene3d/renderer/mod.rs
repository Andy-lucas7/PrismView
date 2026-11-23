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

mod shared;
#[allow(unused_imports)]
pub use shared::*;

mod objects;
#[allow(unused_imports)]
pub use objects::*;

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

    /// Órbita por cursor configurada (o loop precisa do mouse mesmo sem bindings).
    pub fn wants_cursor(&self) -> bool {
        let cam = &self.config.camera;
        cam.orbit_parallax_x.abs() > 1e-3 || cam.orbit_parallax_y.abs() > 1e-3
    }

}

mod build;
mod targets;
mod pick;
mod environment;
mod draw;
#[allow(unused_imports)]
use build::*;
