//! Modo Espectro: camadas `SHADER` (efeito WGSL do usuário).
//!
//! O Estúdio manda o WGSL completo (prelúdio + `PrismParams` + código + `prism_vs`/`prism_fs`,
//! ver `apps/prismview-studio/src/types/prismEspectro.ts`) e o uniform `params` já empacotado.
//! Aqui: valida com naga antes de criar o módulo (WGSL inválido nunca chega ao driver), renderiza
//! cada efeito num alvo próprio do tamanho da camada e entrega o alvo ao `Compositor2D` como
//! textura pré-multiplicada. O quadro do compositor só desenha o alvo; o efeito é refeito por
//! quadro em `encode` (antes do passe 2D), com `PrismGlobals` num buffer fixo.
//!
//! Custo por quadro: 1 `write_buffer` de 96 B + 1 passe de 1 triângulo por efeito. Alvos e bind
//! groups só são recriados quando o tamanho da camada ou a textura do fundo mudam (o fundo de
//! vídeo gira num anel pequeno de bind groups, sem alocação no regime).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use prism_core::schema::ShaderEffect;

use super::compositor::{Compositor2D, TextureId};
use super::signals::SignalSnapshot;

/// Floats de `PrismGlobals` (mesmo número que `ESPECTRO_GLOBALS_FLOATS` no Estúdio).
pub const GLOBALS_FLOATS: usize = 24;
/// Formato dos alvos (o compositor amostra; não-sRGB como o resto do 2D).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Maior lado de um alvo de efeito.
const MAX_TARGET_DIM: u32 = 4096;
/// Bind groups guardados por efeito (fundo de vídeo alterna entre os slots do anel do decoder).
const BIND_CACHE: usize = 6;
/// Limite do uniform `params` (32 parâmetros de até 16 B = 512 B).
const MAX_PARAMS_BYTES: usize = 512;

/// Onde a camada está no quadro e de onde vem o fundo (preenchido na construção da lista 2D).
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
}

struct Shared {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    fallback_view: wgpu::TextureView,
    _fallback: wgpu::Texture,
}

struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

struct Slot {
    hash: u64,
    pipeline: Option<wgpu::RenderPipeline>,
    globals_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    params: Vec<f32>,
    params_dirty: bool,
    target: Option<Target>,
    tex_id: Option<TextureId>,
    binds: Vec<(u64, wgpu::BindGroup)>,
    placement: EspectroPlacement,
    epoch: u64,
    /// Saída (monitor) dona do slot: cada saída tem seu alvo no tamanho dela.
    canvas: String,
}

/// Sinais copiados do snapshot do quadro (sem alocação).
#[derive(Clone, Copy, Default)]
struct Inputs {
    audio: [f32; 4],
    beat: f32,
    cursor: [f32; 2],
    battery: f32,
}

pub struct EspectroRenderer {
    shared: Option<Shared>,
    slots: HashMap<String, Slot>,
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

impl EspectroRenderer {
    pub fn new() -> Self {
        Self {
            shared: None,
            slots: HashMap::new(),
            epoch: 0,
            t0: Instant::now(),
            last: None,
            globals: [0.0; GLOBALS_FLOATS],
            inputs: Inputs {
                battery: 1.0,
                cursor: [0.5, 0.5],
                ..Inputs::default()
            },
            issues: Vec::new(),
        }
    }

    /// Algum efeito desenhável na última lista: o loop precisa continuar redesenhando.
    pub fn is_active(&self) -> bool {
        self.slots.values().any(|s| s.pipeline.is_some() && s.tex_id.is_some())
    }

    /// Erros de compilação desde a última chamada: `(id da camada, mensagem)`.
    pub fn take_issues(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.issues)
    }

    /// Copia os sinais do quadro usados por `PrismGlobals`.
    pub fn observe(&mut self, snap: &SignalSnapshot) {
        self.inputs.audio = [snap.level, snap.bass, snap.mid, snap.treble];
        self.inputs.beat = snap.beat;
        self.inputs.cursor = snap.cursor_norm;
        // NaN = sem bateria (desktop): fica em 1.
        self.inputs.battery = if snap.battery_percent.is_finite() {
            (snap.battery_percent / 100.0).clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    fn shared(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> &Shared {
        self.shared.get_or_insert_with(|| {
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
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("espectro-bgl"),
                entries: &[
                    uniform(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: stage,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: stage,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    uniform(3),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("espectro-pl"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("espectro-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let fallback = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("espectro-fallback"),
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
                    texture: &fallback,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[6, 6, 8, 255],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            let fallback_view = fallback.create_view(&Default::default());
            Shared {
                layout,
                pipeline_layout,
                sampler,
                fallback_view,
                _fallback: fallback,
            }
        })
    }

    fn compile(device: &wgpu::Device, shared: &Shared, wgsl: &str) -> Result<wgpu::RenderPipeline, String> {
        validate_wgsl(wgsl)?;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("espectro"),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                entry_point: Some("prism_fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        match pollster::block_on(device.pop_error_scope()) {
            Some(err) => Err(err.to_string()),
            None => Ok(pipeline),
        }
    }

    /// Início da construção de uma lista 2D (efeitos não vistos até `end_build` são liberados).
    pub fn begin_build(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// Registra/atualiza o efeito da camada `layer_id` na saída `canvas_key` com alvo de `px` pixels. Devolve a textura
    /// a desenhar no compositor (`None` = efeito com erro ou caixa vazia: nada é desenhado).
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
                    pipeline: None,
                    globals_buf: buf("espectro-globals", (GLOBALS_FLOATS * 4) as u64),
                    params_buf: buf("espectro-params", MAX_PARAMS_BYTES as u64),
                    params: Vec::new(),
                    params_dirty: true,
                    target: None,
                    tex_id: None,
                    binds: Vec::new(),
                    placement,
                    epoch,
                    canvas: canvas_key.to_string(),
                },
            );
        }
        let slot = self.slots.get_mut(&key)?;
        slot.epoch = epoch;
        slot.placement = placement;
        if slot.hash != hash {
            slot.hash = hash;
            slot.binds.clear();
            match Self::compile(device, shared, &effect.wgsl) {
                Ok(p) => slot.pipeline = Some(p),
                Err(msg) => {
                    slot.pipeline = None;
                    eprintln!("[espectro] efeito \"{}\" ({layer_id}) com erro: {msg}", effect.name);
                    self.issues.push((layer_id.to_string(), msg));
                }
            }
        }
        let n = effect.params.len().min(MAX_PARAMS_BYTES / 4);
        if slot.params.as_slice() != &effect.params[..n] {
            slot.params.clear();
            slot.params.extend_from_slice(&effect.params[..n]);
            slot.params_dirty = true;
        }
        slot.pipeline.as_ref()?;
        if slot.target.as_ref().map(|t| t.size) != Some(px) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("espectro-target"),
                size: wgpu::Extent3d { width: px.0, height: px.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            slot.tex_id = Some(compositor.bind_owned_view(device, slot.tex_id, view.clone()));
            slot.target = Some(Target { _texture: texture, view, size: px });
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
        let g = &mut self.globals;
        for slot in self.slots.values_mut() {
            if slot.canvas != canvas_key {
                continue;
            }
            let (Some(pipeline), Some(target)) = (slot.pipeline.as_ref(), slot.target.as_ref()) else {
                continue;
            };
            let p = slot.placement;
            let (view, key) = match p.backdrop.and_then(|id| compositor.texture_view(id).map(|v| (v, id))) {
                Some((v, id)) => (v, (u64::from(id.raw()) << 32) ^ compositor.texture_version(id)),
                None => (&shared.fallback_view, 0),
            };
            let bind = match slot.binds.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    if slot.binds.len() >= BIND_CACHE {
                        slot.binds.remove(0);
                    }
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("espectro-bg"),
                        layout: &shared.layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: slot.globals_buf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shared.sampler) },
                            wgpu::BindGroupEntry { binding: 3, resource: slot.params_buf.as_entire_binding() },
                        ],
                    });
                    slot.binds.push((key, bg));
                    slot.binds.len() - 1
                }
            };

            let (w, h) = target.size;
            g[0] = w as f32;
            g[1] = h as f32;
            g[2] = time;
            g[3] = delta;
            let lr = p.layer_rect;
            g[4] = ((inputs.cursor[0] - lr[0]) / lr[2].max(1e-5)).clamp(-1.0, 2.0);
            g[5] = ((inputs.cursor[1] - lr[1]) / lr[3].max(1e-5)).clamp(-1.0, 2.0);
            g[6] = w as f32 / lr[2].max(1e-5);
            g[7] = h as f32 / lr[3].max(1e-5);
            g[8..12].copy_from_slice(&lr);
            g[12..16].copy_from_slice(&p.backdrop_rect);
            g[16..20].copy_from_slice(&inputs.audio);
            g[20] = inputs.beat;
            g[21] = inputs.battery;
            g[22] = if p.backdrop.is_some() { 1.0 } else { 0.0 };
            g[23] = if p.backdrop_opaque { 1.0 } else { 0.0 };
            queue.write_buffer(&slot.globals_buf, 0, bytemuck::cast_slice(&g[..]));
            if slot.params_dirty {
                slot.params_dirty = false;
                if !slot.params.is_empty() {
                    queue.write_buffer(&slot.params_buf, 0, bytemuck::cast_slice(&slot.params));
                }
            }

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("espectro"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
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
            pass.set_bind_group(0, &slot.binds[bind].1, &[]);
            pass.draw(0..3, 0..1);
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

    /// Os modelos do Estúdio (fixture gerado por `tests/espectro.test.ts`) passam no naga, o
    /// mesmo validador do wgpu: o WGSL que o Estúdio monta roda no motor.
    #[test]
    fn studio_templates_validate() {
        let raw = include_str!("../../tests/fixtures/espectro_templates.json");
        let cases: Vec<Case> = serde_json::from_str(raw).expect("fixture");
        assert!(!cases.is_empty());
        for c in cases {
            validate_wgsl(&c.wgsl).unwrap_or_else(|e| panic!("{}: {e}", c.id));
            assert!(c.params.len() * 4 <= super::MAX_PARAMS_BYTES, "{}", c.id);
            assert_eq!(c.params.len() % 4, 0, "{}: params em múltiplos de 16 B", c.id);
        }
    }

    #[test]
    fn broken_wgsl_is_rejected() {
        assert!(validate_wgsl("fn effect(uv: vec2f) -> vec4f { return nope; }").is_err());
        assert!(validate_wgsl("@fragment fn main() -> @location(0) vec4f { return vec4f(1.0); }").is_err());
    }
}
