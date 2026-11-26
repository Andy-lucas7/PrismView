//! Modo Espectro: camadas `SHADER` (efeito WGSL do usuário).
//!
//! O Estúdio manda o WGSL completo (prelúdio + `PrismParams` + código + `prism_vs`/`prism_fs` e,
//! se o efeito define `simulate`, `prism_sim`; ver `apps/prismview-studio/src/types/prismEspectro.ts`)
//! e o uniform `params` já empacotado. Aqui: valida com naga antes de criar o módulo (WGSL inválido
//! nunca chega ao driver), renderiza cada efeito num alvo próprio do tamanho da camada e entrega o
//! alvo ao `Compositor2D` como textura pré-multiplicada.
//!
//! Por quadro, por efeito: 1 `write_buffer` de 144 B, [passe de estado + cópia] + passe do efeito
//! + [cópia para o quadro anterior]. Alvos e bind groups só são recriados quando o tamanho da
//! camada ou as texturas mudam (o fundo de vídeo gira num anel pequeno de bind groups).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use prism_core::schema::ShaderEffect;

use super::compositor::{Compositor2D, TextureId};
use super::signals::SignalSnapshot;

/// Floats de `PrismGlobals` (mesmo número que `ESPECTRO_GLOBALS_FLOATS` no Estúdio).
pub const GLOBALS_FLOATS: usize = 36;
/// Slots de textura do usuário (`prism_tex0..3`).
pub const MAX_TEXTURES: usize = 4;
/// Formato dos alvos (o compositor amostra; não-sRGB como o resto do 2D).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Formato do buffer de estado (`ESPECTRO_STATE_FORMAT` no Estúdio).
const STATE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Maior lado de um alvo de efeito.
const MAX_TARGET_DIM: u32 = 4096;
/// Bind groups guardados por efeito (fundo de vídeo alterna entre os slots do anel do decoder).
const BIND_CACHE: usize = 6;
/// Limite do uniform `params` (32 parâmetros de até 16 B = 512 B).
const MAX_PARAMS_BYTES: usize = 512;
/// `click_age` quando ainda não houve clique.
const NO_CLICK_AGE: f32 = 1.0e6;

/// Onde a camada está no quadro e de onde vêm as texturas (preenchido na construção da lista 2D).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EspectroPlacement {
    /// x, y, w, h da camada em 0..1 do quadro.
    pub layer_rect: [f32; 4],
    /// Área (0..1 do quadro) onde a textura do fundo é desenhada: `sample_backdrop` mapeia o
    /// uv do quadro para o uv da textura com ela.
    pub backdrop_rect: [f32; 4],
    pub backdrop: Option<TextureId>,
    /// Fundo sem alpha confiável (vídeo/imagem opaca): o prelúdio força a = 1.
    pub backdrop_opaque: bool,
    /// Imagens do usuário já carregadas no compositor (`None` = branco).
    pub textures: [Option<TextureId>; MAX_TEXTURES],
}

struct Fallback {
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Shared {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    repeat: wgpu::Sampler,
    /// Fundo ausente (cinza-escuro), textura do usuário ausente (branco), memória ausente (transparente).
    dark: Fallback,
    white: Fallback,
    clear: Fallback,
}

struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Target {
    out: Tex,
    /// Cópia do quadro anterior (`sample_previous`), só se o efeito usa.
    prev: Option<Tex>,
    /// Estado (atual, anterior) da simulação, só se o efeito define `simulate`.
    state: Option<(Tex, Tex)>,
    size: (u32, u32),
}

struct Pipelines {
    effect: wgpu::RenderPipeline,
    sim: Option<wgpu::RenderPipeline>,
}

struct Slot {
    hash: u64,
    pipelines: Option<Pipelines>,
    feedback: bool,
    globals_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    params: Vec<f32>,
    params_dirty: bool,
    target: Option<Target>,
    tex_id: Option<TextureId>,
    /// (chave das texturas, bind group do efeito, bind group da simulação).
    binds: Vec<(u64, wgpu::BindGroup, Option<wgpu::BindGroup>)>,
    placement: EspectroPlacement,
    /// Tamanho do quadro da saída (px).
    frame_px: (f32, f32),
    frame: u32,
    epoch: u64,
    /// Saída (monitor) dona do slot: cada saída tem seu alvo no tamanho dela.
    canvas: String,
}

/// Sinais copiados do snapshot do quadro (sem alocação).
#[derive(Clone, Copy)]
struct Inputs {
    audio: [f32; 4],
    beat: f32,
    cursor_px: (i32, i32),
    click_px: Option<(i32, i32)>,
    click_at: Option<Instant>,
    mouse_down: f32,
    key_pulse: f32,
    typing: f32,
    idle: f32,
    media: [f32; 3],
    battery: f32,
}

impl Default for Inputs {
    fn default() -> Self {
        Self {
            audio: [0.0; 4],
            beat: 0.0,
            cursor_px: (0, 0),
            click_px: None,
            click_at: None,
            mouse_down: 0.0,
            key_pulse: 0.0,
            typing: 0.0,
            idle: 0.0,
            media: [0.0; 3],
            battery: 1.0,
        }
    }
}

pub struct EspectroRenderer {
    shared: Option<Shared>,
    slots: HashMap<String, Slot>,
    /// Canto do monitor de cada saída no desktop virtual (px físicos): cursor por monitor.
    origins: HashMap<String, (i32, i32)>,
    epoch: u64,
    t0: Instant,
    last: Option<Instant>,
    globals: [f32; GLOBALS_FLOATS],
    inputs: Inputs,
    /// Erros de compilação ainda não lidos (`take_issues`).
    issues: Vec<(String, String)>,
}

impl Default for EspectroRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_wgsl(wgsl: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    wgsl.hash(&mut h);
    h.finish()
}

/// Valida o WGSL com naga (parse + validação completa). `Err` = mensagem para o log/status.
pub fn validate_wgsl(wgsl: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.emit_to_string(wgsl))?;
    let has_entry = |name: &str, stage: naga::ShaderStage| {
        module.entry_points.iter().any(|e| e.name == name && e.stage == stage)
    };
    if !has_entry("prism_vs", naga::ShaderStage::Vertex) || !has_entry("prism_fs", naga::ShaderStage::Fragment) {
        return Err("efeito sem as entradas prism_vs/prism_fs (WGSL não veio do Estúdio?)".into());
    }
    Ok(())
}

fn has_entry(wgsl: &str, name: &str) -> bool {
    wgsl.contains(&format!("fn {name}("))
}

fn solid(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4], label: &str) -> Fallback {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let view = tex.create_view(&Default::default());
    Fallback { _tex: tex, view }
}

fn make_tex(device: &wgpu::Device, size: (u32, u32), format: wgpu::TextureFormat, label: &str) -> Tex {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Tex { texture, view }
}

fn copy_tex(encoder: &mut wgpu::CommandEncoder, src: &Tex, dst: &Tex, size: (u32, u32)) {
    encoder.copy_texture_to_texture(
        src.texture.as_image_copy(),
        dst.texture.as_image_copy(),
        wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
    );
}

fn draw_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    label: &str,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
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
    pass.set_bind_group(0, bind, &[]);
    pass.draw(0..3, 0..1);
}

impl EspectroRenderer {
    pub fn new() -> Self {
        Self {
            shared: None,
            slots: HashMap::new(),
            origins: HashMap::new(),
            epoch: 0,
            t0: Instant::now(),
            last: None,
            globals: [0.0; GLOBALS_FLOATS],
            inputs: Inputs::default(),
            issues: Vec::new(),
        }
    }

    /// Algum efeito desenhável na última lista: o loop precisa continuar redesenhando.
    pub fn is_active(&self) -> bool {
        self.slots.values().any(|s| s.pipelines.is_some() && s.tex_id.is_some())
    }

    /// Erros de compilação desde a última chamada: `(id da camada, mensagem)`.
    pub fn take_issues(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.issues)
    }

    /// Canto do monitor da saída `canvas_key` no desktop virtual (px físicos). Sem alocação
    /// quando a saída já é conhecida.
    pub fn set_output_origin(&mut self, canvas_key: &str, origin: (i32, i32)) {
        match self.origins.get_mut(canvas_key) {
            Some(o) => *o = origin,
            None => {
                self.origins.insert(canvas_key.to_string(), origin);
            }
        }
    }

    /// Copia os sinais do quadro usados por `PrismGlobals`.
    pub fn observe(&mut self, snap: &SignalSnapshot) {
        let i = &mut self.inputs;
        i.audio = [snap.level, snap.bass, snap.mid, snap.treble];
        i.beat = snap.beat;
        i.cursor_px = snap.cursor;
        if snap.clicks > 0 {
            i.click_px = Some(snap.cursor);
            i.click_at = Some(Instant::now());
        }
        i.mouse_down = if snap.buttons != 0 { 1.0 } else { 0.0 };
        i.key_pulse = snap.key_pulse;
        i.typing = snap.typing_energy;
        i.idle = snap.idle_s;
        i.media = [
            if snap.media_playing { 1.0 } else { 0.0 },
            snap.media_progress,
            if snap.media_has_session { 1.0 } else { 0.0 },
        ];
        // NaN = sem bateria (desktop): fica em 1.
        i.battery = if snap.battery_percent.is_finite() {
            (snap.battery_percent / 100.0).clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    fn shared(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.shared.is_some() {
            return;
        }
        let stage = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stage,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stage,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stage,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("espectro-bgl"),
            entries: &[
                uniform(0),
                texture(1),
                sampler(2),
                uniform(3),
                texture(4),
                texture(5),
                texture(6),
                texture(7),
                texture(8),
                sampler(9),
                texture(10),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("espectro-pl"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let linear = |mode| wgpu::SamplerDescriptor {
            label: Some("espectro-sampler"),
            address_mode_u: mode,
            address_mode_v: mode,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        };
        self.shared = Some(Shared {
            sampler: device.create_sampler(&linear(wgpu::AddressMode::ClampToEdge)),
            repeat: device.create_sampler(&linear(wgpu::AddressMode::Repeat)),
            dark: solid(device, queue, [6, 6, 8, 255], "espectro-dark"),
            white: solid(device, queue, [255, 255, 255, 255], "espectro-white"),
            clear: solid(device, queue, [0, 0, 0, 0], "espectro-clear"),
            layout,
            pipeline_layout,
        });
    }

    fn compile(device: &wgpu::Device, shared: &Shared, wgsl: &str) -> Result<Pipelines, String> {
        validate_wgsl(wgsl)?;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("espectro"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let make = |entry: &str, format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("espectro"),
                layout: Some(&shared.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("prism_vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
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
        let effect = make("prism_fs", TARGET_FORMAT);
        let sim = has_entry(wgsl, "prism_sim").then(|| make("prism_sim", STATE_FORMAT));
        match pollster::block_on(device.pop_error_scope()) {
            Some(err) => Err(err.to_string()),
            None => Ok(Pipelines { effect, sim }),
        }
    }

    /// Início da construção de uma lista 2D (efeitos não vistos até `end_build` são liberados).
    pub fn begin_build(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Registra/atualiza o efeito da camada `layer_id` na saída `canvas_key` com alvo de `px` pixels.
    /// Devolve a textura a desenhar no compositor (`None` = efeito com erro: nada é desenhado).
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        compositor: &mut Compositor2D,
        canvas_key: &str,
        layer_id: &str,
        effect: &ShaderEffect,
        px: (u32, u32),
        frame_px: (f32, f32),
        placement: EspectroPlacement,
    ) -> Option<TextureId> {
        let px = (px.0.clamp(1, MAX_TARGET_DIM), px.1.clamp(1, MAX_TARGET_DIM));
        let epoch = self.epoch;
        let hash = hash_wgsl(&effect.wgsl);
        self.shared(device, queue);
        let shared = self.shared.as_ref()?;
        // Chave por saída + camada (só na construção da lista, nunca por quadro).
        let key = format!("{canvas_key}\u{1}{layer_id}");

        if !self.slots.contains_key(&key) {
            let buf = |label, size: u64| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            self.slots.insert(
                key.clone(),
                Slot {
                    hash: hash ^ 1, // força a compilação abaixo
                    pipelines: None,
                    feedback: false,
                    globals_buf: buf("espectro-globals", (GLOBALS_FLOATS * 4) as u64),
                    params_buf: buf("espectro-params", MAX_PARAMS_BYTES as u64),
                    params: Vec::new(),
                    params_dirty: true,
                    target: None,
                    tex_id: None,
                    binds: Vec::new(),
                    placement,
                    frame_px,
                    frame: 0,
                    epoch,
                    canvas: canvas_key.to_string(),
                },
            );
        }
        let slot = self.slots.get_mut(&key)?;
        slot.epoch = epoch;
        slot.placement = placement;
        slot.frame_px = frame_px;
        let mut reshape = false;
        if slot.hash != hash {
            slot.hash = hash;
            slot.binds.clear();
            slot.frame = 0;
            reshape = true;
            match Self::compile(device, shared, &effect.wgsl) {
                Ok(p) => slot.pipelines = Some(p),
                Err(msg) => {
                    slot.pipelines = None;
                    eprintln!("[espectro] efeito \"{}\" ({layer_id}) com erro: {msg}", effect.name);
                    self.issues.push((layer_id.to_string(), msg));
                }
            }
        }
        if slot.feedback != effect.feedback {
            slot.feedback = effect.feedback;
            reshape = true;
        }
        let n = effect.params.len().min(MAX_PARAMS_BYTES / 4);
        if slot.params.as_slice() != &effect.params[..n] {
            slot.params.clear();
            slot.params.extend_from_slice(&effect.params[..n]);
            slot.params_dirty = true;
        }
        let simulate = slot.pipelines.as_ref()?.sim.is_some();
        if reshape || slot.target.as_ref().map(|t| t.size) != Some(px) {
            let out = make_tex(device, px, TARGET_FORMAT, "espectro-target");
            slot.tex_id = Some(compositor.bind_owned_view(device, slot.tex_id, out.view.clone()));
            slot.target = Some(Target {
                out,
                prev: slot.feedback.then(|| make_tex(device, px, TARGET_FORMAT, "espectro-prev")),
                state: simulate.then(|| {
                    (
                        make_tex(device, px, STATE_FORMAT, "espectro-state"),
                        make_tex(device, px, STATE_FORMAT, "espectro-state-prev"),
                    )
                }),
                size: px,
            });
            slot.binds.clear();
            slot.frame = 0;
        }
        slot.tex_id
    }

    /// Fim da construção da saída `canvas_key`: libera os efeitos dela que saíram da cena.
    pub fn end_build(&mut self, canvas_key: &str, compositor: &mut Compositor2D) {
        let epoch = self.epoch;
        self.slots.retain(|_, s| {
            let keep = s.epoch == epoch || s.canvas != canvas_key;
            if !keep {
                if let Some(id) = s.tex_id {
                    compositor.release_texture(id);
                }
            }
            keep
        });
    }

    /// Solta todas as texturas (hibernação / device perdido / cena nova).
    pub fn release_gpu(&mut self, compositor: &mut Compositor2D) {
        for (_, s) in self.slots.drain() {
            if let Some(id) = s.tex_id {
                compositor.release_texture(id);
            }
        }
    }

    /// Renderiza os efeitos da saída `canvas_key` em `encoder` (antes do passe 2D que os compõe).
    pub fn encode(
        &mut self,
        canvas_key: &str,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        compositor: &Compositor2D,
    ) {
        let Some(shared) = self.shared.as_ref() else {
            return;
        };
        let now = Instant::now();
        let time = now.duration_since(self.t0).as_secs_f32();
        let delta = self.last.map_or(0.0, |l| now.duration_since(l).as_secs_f32()).min(0.25);
        self.last = Some(now);
        let inputs = self.inputs;
        let origin = self.origins.get(canvas_key).copied().unwrap_or((0, 0));
        let g = &mut self.globals;
        for slot in self.slots.values_mut() {
            if slot.canvas != canvas_key {
                continue;
            }
            let (Some(pipes), Some(target)) = (slot.pipelines.as_ref(), slot.target.as_ref()) else {
                continue;
            };
            let p = slot.placement;
            let view_of = |id: Option<TextureId>| id.and_then(|id| compositor.texture_view(id).map(|v| (v, id)));
            let backdrop = view_of(p.backdrop);

            // Chave das texturas ligadas (fundo + imagens do usuário, com versão). FNV-1a, sem alocação.
            let mut key = 0xcbf2_9ce4_8422_2325u64;
            let ids = [backdrop.map(|(_, id)| id), p.textures[0], p.textures[1], p.textures[2], p.textures[3]];
            for id in ids {
                let (a, b) = match id.filter(|id| compositor.texture_view(*id).is_some()) {
                    Some(id) => (u64::from(id.raw()) + 1, compositor.texture_version(id)),
                    None => (0, 0),
                };
                key = (key ^ a).wrapping_mul(0x0100_0000_01b3);
                key = (key ^ b).wrapping_mul(0x0100_0000_01b3);
            }
            let bind = match slot.binds.iter().position(|(k, _, _)| *k == key) {
                Some(i) => i,
                None => {
                    if slot.binds.len() >= BIND_CACHE {
                        slot.binds.remove(0);
                    }
                    let bd_view = backdrop.map_or(&shared.dark.view, |(v, _)| v);
                    let user = |i: usize| view_of(p.textures[i]).map_or(&shared.white.view, |(v, _)| v);
                    let prev = target.prev.as_ref().map_or(&shared.clear.view, |t| &t.view);
                    let globals_buf = &slot.globals_buf;
                    let params_buf = &slot.params_buf;
                    let make = |state: &wgpu::TextureView| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("espectro-bg"),
                            layout: &shared.layout,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(bd_view) },
                                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shared.sampler) },
                                wgpu::BindGroupEntry { binding: 3, resource: params_buf.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(prev) },
                                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(user(0)) },
                                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(user(1)) },
                                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(user(2)) },
                                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(user(3)) },
                                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(&shared.repeat) },
                                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(state) },
                            ],
                        })
                    };
                    // `effect` lê o estado novo; `simulate` lê o anterior.
                    let (effect_bg, sim_bg) = match target.state.as_ref() {
                        Some((cur, old)) => (make(&cur.view), Some(make(&old.view))),
                        None => (make(&shared.clear.view), None),
                    };
                    slot.binds.push((key, effect_bg, sim_bg));
                    slot.binds.len() - 1
                }
            };

            let (w, h) = target.size;
            let lr = p.layer_rect;
            let (fw, fh) = (slot.frame_px.0.max(1.0), slot.frame_px.1.max(1.0));
            // Cursor/clique em uv da camada, no monitor desta saída.
            let to_uv = |pt: (i32, i32)| {
                let fx = (pt.0 - origin.0) as f32 / fw;
                let fy = (pt.1 - origin.1) as f32 / fh;
                [(fx - lr[0]) / lr[2].max(1e-5), (fy - lr[1]) / lr[3].max(1e-5)]
            };
            let cursor = to_uv(inputs.cursor_px);
            let click = inputs.click_px.map_or([0.5, 0.5], to_uv);
            g[0] = w as f32;
            g[1] = h as f32;
            g[2] = time;
            g[3] = delta;
            g[4] = cursor[0].clamp(-4.0, 5.0);
            g[5] = cursor[1].clamp(-4.0, 5.0);
            g[6] = fw;
            g[7] = fh;
            g[8..12].copy_from_slice(&lr);
            g[12..16].copy_from_slice(&p.backdrop_rect);
            g[16..20].copy_from_slice(&inputs.audio);
            g[20] = inputs.beat;
            g[21] = inputs.battery;
            g[22] = if p.backdrop.is_some() { 1.0 } else { 0.0 };
            g[23] = if p.backdrop_opaque { 1.0 } else { 0.0 };
            g[24] = click[0].clamp(-4.0, 5.0);
            g[25] = click[1].clamp(-4.0, 5.0);
            g[26] = inputs.click_at.map_or(NO_CLICK_AGE, |t| now.duration_since(t).as_secs_f32());
            g[27] = slot.frame as f32;
            g[28] = inputs.mouse_down;
            g[29] = inputs.key_pulse;
            g[30] = inputs.typing;
            g[31] = inputs.idle;
            g[32] = inputs.media[0];
            g[33] = inputs.media[1];
            g[34] = inputs.media[2];
            g[35] = 0.0;
            slot.frame = slot.frame.saturating_add(1);
            queue.write_buffer(&slot.globals_buf, 0, bytemuck::cast_slice(&g[..]));
            if slot.params_dirty {
                slot.params_dirty = false;
                if !slot.params.is_empty() {
                    queue.write_buffer(&slot.params_buf, 0, bytemuck::cast_slice(&slot.params));
                }
            }

            let (_, effect_bg, sim_bg) = &slot.binds[bind];
            if let (Some(sim), Some(sim_bg), Some((cur, old))) = (pipes.sim.as_ref(), sim_bg.as_ref(), target.state.as_ref()) {
                draw_pass(encoder, &cur.view, sim, sim_bg, "espectro-sim");
                copy_tex(encoder, cur, old, target.size);
            }
            draw_pass(encoder, &target.out.view, &pipes.effect, effect_bg, "espectro");
            if let Some(prev) = target.prev.as_ref() {
                copy_tex(encoder, &target.out, prev, target.size);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate_wgsl;

    #[derive(serde::Deserialize)]
    struct Case {
        id: String,
        wgsl: String,
        params: Vec<f32>,
    }

    fn cases() -> Vec<Case> {
        serde_json::from_str(include_str!("../../tests/fixtures/espectro_templates.json")).expect("fixture")
    }

    /// Os modelos do Estúdio (fixture gerado por `tests/espectro.test.ts`) passam no naga, o
    /// mesmo validador do wgpu: o WGSL que o Estúdio monta roda no motor.
    #[test]
    fn studio_templates_validate() {
        let cases = cases();
        assert!(cases.len() >= 7);
        for c in cases {
            validate_wgsl(&c.wgsl).unwrap_or_else(|e| panic!("{}: {e}", c.id));
            assert!(c.params.len() * 4 <= super::MAX_PARAMS_BYTES, "{}", c.id);
            assert_eq!(c.params.len() % 4, 0, "{}: params em múltiplos de 16 B", c.id);
        }
    }

    #[test]
    fn simulation_entry_is_detected() {
        let cases = cases();
        let water = cases.iter().find(|c| c.id == "water").expect("modelo água");
        assert!(super::has_entry(&water.wgsl, "prism_sim"));
        let rain = cases.iter().find(|c| c.id == "rain-glass").expect("modelo chuva");
        assert!(!super::has_entry(&rain.wgsl, "prism_sim"));
    }

    #[test]
    fn broken_wgsl_is_rejected() {
        assert!(validate_wgsl("fn effect(uv: vec2f) -> vec4f { return nope; }").is_err());
        assert!(validate_wgsl("@fragment fn main() -> @location(0) vec4f { return vec4f(1.0); }").is_err());
    }
}
