//! Instância de um modelo na cena (1 por `Scene3dObject` GLB): pose animada, matrizes de joint,
//! pesos de morph e o buffer de vértices deformados.
//!
//! Tudo que o quadro usa é dimensionado em `ModelInstance::new`:
//! - `world`, `joint_staging` e a pose têm tamanho fixo e só são reescritos;
//! - os parâmetros de cada job de deformação ficam num uniform escrito uma única vez (offset
//!   dinâmico por job), então o quadro só sobe joints + pesos e despacha.
//!
//! A deformação só roda quando a pose muda (`deform_dirty`): modelo parado = zero compute.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::animation::{sample_clip, AdditiveLayer, ClipPlayer, Pose};
use super::config::Scene3dPlayback;
use super::gpu_model::{GpuModel, GpuPrimitive, DEFORM_STRIDE};
use crate::engine_core::assets::Aabb;

/// Lado do cubo de normalização do Estúdio (`2.4 / maxDim`, centralizado).
const NORMALIZED_SIZE: f32 = 2.4;
const JOB_ALIGN: u64 = 256;

// ───────────────────────── Kernel de deformação ─────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct JobUniform {
    src_first: u32,
    out_first: u32,
    count: u32,
    joint_base: u32,
    joint_count: u32,
    morph_base: u32,
    morph_count: u32,
    weight_base: u32,
    flags: u32,
    _pad: [u32; 3],
}

/// Pipeline de `deform.wgsl` (1 por device).
pub struct DeformKernel {
    pub layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::ComputePipeline,
}

impl DeformKernel {
    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_deform"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<JobUniform>() as u64),
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, true),
                storage(5, true),
                storage(6, false),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_deform"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/deform.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_deform"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("prismview_deform"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_deform"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        Self { layout, pipeline }
    }
}

// ───────────────────────── Instância ─────────────────────────

/// Uma primitiva desenhada por um nó.
#[derive(Clone, Copy, Debug)]
pub struct DrawRef {
    pub node: u32,
    /// Índice em `GpuModel::primitives`.
    pub prim: u32,
    /// Primeiro vértice na saída deformada da instância (`None` = geometria original).
    pub deformed: Option<u32>,
    /// Vértices já em espaço do modelo (skin): a matriz do nó não se aplica.
    pub skinned: bool,
}

#[derive(Clone, Copy, Debug)]
struct DeformJob {
    offset: u32,
    groups: u32,
}

#[derive(Clone, Copy, Debug)]
struct SkinBinding {
    skin: u32,
    joint_base: u32,
}

struct DeformState {
    output: wgpu::Buffer,
    joints: wgpu::Buffer,
    weights: wgpu::Buffer,
    _jobs_uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    jobs: Vec<DeformJob>,
}

/// Clipe tocado por um binding `PLAY_CLIP` por cima do clipe base.
#[derive(Clone, Copy, Debug)]
struct ClipOverride {
    /// `true` = segura em loop enquanto o sinal está ativo (hover); `false` = toca 1× (clique).
    hold: bool,
    /// Binding dono (só ele devolve ao clipe base ao soltar).
    owner: Option<u32>,
}

pub struct ModelInstance {
    pub model: Arc<GpuModel>,
    pose: Pose,
    /// Matrizes de mundo dos nós (espaço do modelo).
    world: Vec<Mat4>,
    /// `S(2.4/maxDim) · T(-centro)` como o Estúdio.
    pub normalize: Mat4,
    pub draws: Vec<DrawRef>,
    /// AABB conservadora (espaço do modelo) para primitivas com skin.
    pub skinned_bounds: Aabb,
    player: ClipPlayer,
    /// Clipe/loop do `playback` do objeto (volta a ele quando um override de binding termina).
    base_clip: Option<usize>,
    base_looping: bool,
    clip_override: Option<ClipOverride>,
    /// `CLIP_SCRUB`: o tempo do clipe vem de um binding (fórmula/sinal), não do relógio.
    scrub: bool,
    /// Offsets dos bindings reativos (Fase 4), refeitos a cada quadro sobre a pose animada.
    additive: AdditiveLayer,
    /// Só os bindings mudaram: desfaz (restaura a pose animada) e reaplica, sem reamostrar.
    binding_dirty: bool,
    /// A camada tinha offsets de nó/morph no quadro anterior.
    pose_layer_active: bool,
    skin_bindings: Vec<SkinBinding>,
    joint_staging: Vec<[f32; 16]>,
    deform: Option<DeformState>,
    pose_dirty: bool,
    deform_dirty: bool,
}

impl ModelInstance {
    pub fn new(
        device: &wgpu::Device,
        kernel: &DeformKernel,
        model: Arc<GpuModel>,
        playback: &Scene3dPlayback,
        normalize_bounds: bool,
    ) -> Self {
        let rig = &model.rig;
        let pose = rig.rest_pose.clone();
        let node_count = pose.node_count();

        // ── Primitivas por nó e saída deformada ──
        let mut draws: Vec<DrawRef> = Vec::new();
        let mut skin_bindings: Vec<SkinBinding> = Vec::new();
        let mut node_joint_base: Vec<Option<(u32, u32)>> = vec![None; node_count];
        let mut joint_total = 0u32;
        let mut out_vertices = 0u32;
        let mut job_data: Vec<JobUniform> = Vec::new();

        for node in 0..node_count {
            if !pose.in_scene.get(node).copied().unwrap_or(false) {
                continue;
            }
            let Some(mesh) = rig.node_mesh.get(node).copied().flatten() else {
                continue;
            };
            let skin = rig
                .node_skin
                .get(node)
                .copied()
                .flatten()
                .filter(|&s| rig.skins.get(s as usize).is_some_and(|s| !s.joints.is_empty()));
            let (weight_base, weight_count) = pose.weight_range.get(node).copied().unwrap_or((0, 0));
            let (start, count) = model.mesh_primitives.get(mesh as usize).copied().unwrap_or((0, 0));
            for prim_index in start..start + count {
                let Some(prim) = model.primitives.get(prim_index as usize) else {
                    continue;
                };
                if prim.index_count == 0 || prim.vertex_count == 0 {
                    continue;
                }
                let skinned = prim.skinned && skin.is_some();
                let morphed = prim.morph_count > 0 && weight_count > 0;
                let mut deformed = None;
                if skinned || morphed {
                    let (joint_base, joint_count) = match (skinned, skin) {
                        (true, Some(s)) => *node_joint_base[node].get_or_insert_with(|| {
                            let n = rig.skins.get(s as usize).map_or(0, |k| k.joints.len());
                            let n = u32::try_from(n).unwrap_or(0);
                            let base = joint_total;
                            joint_total += n;
                            skin_bindings.push(SkinBinding { skin: s, joint_base: base });
                            (base, n)
                        }),
                        _ => (0, 0),
                    };
                    job_data.push(JobUniform {
                        src_first: prim.first_vertex,
                        out_first: out_vertices,
                        count: prim.vertex_count,
                        joint_base,
                        joint_count,
                        morph_base: prim.morph_base,
                        morph_count: if morphed { prim.morph_count.min(weight_count) } else { 0 },
                        weight_base,
                        flags: u32::from(skinned),
                        _pad: [0; 3],
                    });
                    deformed = Some(out_vertices);
                    out_vertices += prim.vertex_count;
                }
                draws.push(DrawRef {
                    node: u32::try_from(node).unwrap_or(u32::MAX),
                    prim: prim_index,
                    deformed,
                    skinned,
                });
            }
        }

        let deform = (!job_data.is_empty()).then(|| {
            Self::create_deform(device, kernel, &model, &job_data, out_vertices, joint_total, pose.weights.len())
        });

        // ── Normalização (Box3 de modelos GLB → 2,4 no maior eixo, centrado; SHOWCASE fica 1:1) ──
        let bounds = rig.bounds;
        let normalize = if !normalize_bounds || bounds.is_empty() {
            Mat4::IDENTITY
        } else {
            let size = bounds.max - bounds.min;
            let max_dim = size.max_element();
            if max_dim.is_finite() && max_dim > 1e-4 {
                let s = NORMALIZED_SIZE / max_dim;
                let center = (bounds.min + bounds.max) * 0.5;
                Mat4::from_scale(Vec3::splat(s)) * Mat4::from_translation(-center)
            } else {
                Mat4::IDENTITY
            }
        };
        let skinned_bounds = if bounds.is_empty() {
            Aabb::EMPTY
        } else {
            // Pose animada pode sair da caixa de repouso: margem de 50% em torno do centro.
            let center = (bounds.min + bounds.max) * 0.5;
            let half = (bounds.max - bounds.min) * 0.75;
            Aabb {
                min: center - half,
                max: center + half,
            }
        };

        let additive = AdditiveLayer::new(node_count, pose.weight_count(), model.materials.len());
        let mut this = Self {
            base_clip: None,
            base_looping: true,
            clip_override: None,
            scrub: false,
            additive,
            binding_dirty: false,
            pose_layer_active: false,
            world: vec![Mat4::IDENTITY; node_count],
            joint_staging: vec![[0.0; 16]; joint_total as usize],
            normalize,
            draws,
            skinned_bounds,
            player: ClipPlayer::default(),
            skin_bindings,
            deform,
            pose,
            model,
            pose_dirty: true,
            deform_dirty: true,
        };
        this.set_playback(playback);
        this
    }

    fn create_deform(
        device: &wgpu::Device,
        kernel: &DeformKernel,
        model: &GpuModel,
        jobs: &[JobUniform],
        out_vertices: u32,
        joint_total: u32,
        weight_total: usize,
    ) -> DeformState {
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_deform_output"),
            size: (u64::from(out_vertices) * DEFORM_STRIDE).max(DEFORM_STRIDE),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        let joints = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_deform_joints"),
            size: (u64::from(joint_total) * 64).max(64),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let weights = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("prismview_deform_weights"),
            size: (weight_total as u64 * 4).max(16).next_multiple_of(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Um slot de 256 B por job (alinhamento de offset dinâmico).
        let mut bytes = vec![0u8; jobs.len() * JOB_ALIGN as usize];
        let mut list = Vec::with_capacity(jobs.len());
        for (i, job) in jobs.iter().enumerate() {
            let at = i * JOB_ALIGN as usize;
            let raw = bytemuck::bytes_of(job);
            bytes[at..at + raw.len()].copy_from_slice(raw);
            list.push(DeformJob {
                offset: u32::try_from(at).unwrap_or(0),
                groups: job.count.div_ceil(64).max(1),
            });
        }
        let jobs_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("prismview_deform_jobs"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_deform"),
            layout: &kernel.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &jobs_uniform,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<JobUniform>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: model.vertex.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: model.skin.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: model.morph.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: joints.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: weights.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        DeformState {
            output,
            joints,
            weights,
            _jobs_uniform: jobs_uniform,
            bind_group,
            jobs: list,
        }
    }

    /// Aplica `playback` (troca de clipe reinicia o tempo). Com um override de binding ativo, só
    /// atualiza o clipe base (aplicado quando o override termina).
    pub fn set_playback(&mut self, playback: &Scene3dPlayback) {
        let clip = ClipPlayer::select(&self.model.rig.animations, &playback.clip_name);
        let speed = if playback.speed.is_finite() { playback.speed } else { 1.0 };
        self.player.speed = speed;
        self.base_clip = clip;
        self.base_looping = playback.looping;
        if self.scrub {
            // Config nova: o binding de scrub (se ainda existir) retoma no próximo `solve`.
            self.scrub = false;
            self.clip_override = None;
            self.player.clip = None;
        }
        if self.clip_override.is_some() {
            return;
        }
        if clip != self.player.clip {
            self.player.clip = clip;
            self.player.time = 0.0;
            self.player.finished = false;
            self.pose_dirty = true;
        }
        if self.player.looping != playback.looping {
            self.player.finished = false;
        }
        self.player.looping = playback.looping;
    }

    /// Índice do clipe por nome exato (bindings `PLAY_CLIP`).
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        self.model
            .rig
            .animations
            .iter()
            .position(|c| c.name.as_deref() == Some(name))
    }

    /// Toca `clip` por cima do clipe base (troca seca, sem crossfade). `hold` = loop enquanto o
    /// binding segurar; senão toca 1× e volta sozinho ao base em `advance`.
    pub fn play_override(&mut self, clip: usize, hold: bool, owner: Option<u32>) {
        if self.model.rig.animations.get(clip).is_none() {
            return;
        }
        let same = self.player.clip == Some(clip) && self.clip_override.is_some() && !self.scrub;
        self.scrub = false;
        self.clip_override = Some(ClipOverride { hold, owner });
        if same && hold {
            return; // já segurando este clipe
        }
        self.player.clip = Some(clip);
        self.player.time = 0.0;
        self.player.finished = false;
        self.player.looping = hold;
        self.pose_dirty = true;
    }

    /// `CLIP_SCRUB`: posiciona `clip` em `fraction` (0..1) da duração e congela o relógio dele
    /// (a pose segue o valor do binding: fórmula de hora/minuto, CPU, áudio...). Sem alocação;
    /// só marca a pose se o tempo mudou de fato.
    pub fn scrub_clip(&mut self, clip: usize, fraction: f32) {
        let Some(duration) = self.model.rig.animations.get(clip).map(|c| c.duration) else {
            return;
        };
        let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
        // `duration` exato reamostraria o 1º quadro em clipes de loop: fica um ε antes do fim.
        let time = (f * duration).min((duration - 1e-4).max(0.0));
        if !self.scrub || self.player.clip != Some(clip) {
            self.scrub = true;
            self.clip_override = Some(ClipOverride { hold: true, owner: None });
            self.player.clip = Some(clip);
            self.player.finished = false;
            self.player.looping = false;
            self.player.time = time;
            self.pose_dirty = true;
        } else if (self.player.time - time).abs() > 1e-5 {
            self.player.time = time;
            self.pose_dirty = true;
        }
    }

    /// Dono do override atual (`None` = sem override ou override de clique).
    pub fn override_owner(&self) -> Option<u32> {
        self.clip_override.and_then(|o| o.owner)
    }

    /// Encerra o override e volta ao clipe base do `playback`.
    pub fn return_to_base(&mut self) {
        self.scrub = false;
        if self.clip_override.take().is_none() {
            return;
        }
        self.player.clip = self.base_clip;
        self.player.time = 0.0;
        self.player.finished = false;
        self.player.looping = self.base_looping;
        self.pose_dirty = true;
    }

    /// Camada aditiva dos bindings (o solver chama `begin`, acumula e depois `commit_additive`).
    #[inline]
    pub fn additive_mut(&mut self) -> &mut AdditiveLayer {
        &mut self.additive
    }

    #[inline]
    pub fn additive(&self) -> &AdditiveLayer {
        &self.additive
    }

    /// Fecha o quadro dos bindings. `true` se algo visível mudou (pose, raiz ou brilho).
    pub fn commit_additive(&mut self) -> bool {
        let changed = self.additive.finish();
        // Brilho/raiz não tocam a pose: só refaz pose/skin se nós/morphs estão (ou estavam) ativos.
        let touches = self.additive.touches_pose();
        if changed && (touches || self.pose_layer_active) {
            self.binding_dirty = true;
        }
        self.pose_layer_active = touches;
        changed
    }

    /// Fator para converter unidades de exibição (modelo normalizado para 2,4) em unidades
    /// locais do nó (escala do pai × normalização), como `addAxisOffset` do Estúdio.
    pub fn display_to_node_units(&self, node: u32) -> f32 {
        let norm = self.normalize.x_axis.truncate().length();
        let parent = self
            .pose
            .parent_of(node as usize)
            .and_then(|p| self.world.get(p as usize))
            .map_or(1.0, |m| m.x_axis.truncate().length());
        let s = norm * parent;
        if s.is_finite() && s > 1e-6 {
            1.0 / s
        } else {
            1.0
        }
    }

    #[inline]
    pub fn parent_of(&self, node: u32) -> Option<u32> {
        self.pose.parent_of(node as usize)
    }

    /// Matriz do modelo inteiro com o extra `__ROOT__` dos bindings (aplicada antes de `normalize`).
    #[inline]
    pub fn root_matrix(&self) -> Mat4 {
        self.additive.root_matrix()
    }

    #[inline]
    pub fn glow(&self, material: u32) -> f32 {
        self.additive.glow(material)
    }

    /// Precisa de quadros contínuos (clipe tocando).
    pub fn is_animating(&self) -> bool {
        !self.scrub && self.player.is_playing()
    }

    /// Avança a animação. `true` se a pose mudou.
    pub fn advance(&mut self, dt: f32) -> bool {
        if self.scrub {
            return self.pose_dirty; // tempo controlado pelo binding
        }
        let duration = self
            .player
            .clip
            .and_then(|c| self.model.rig.animations.get(c))
            .map_or(0.0, |c| c.duration);
        if self.player.advance(dt, duration) {
            self.pose_dirty = true;
        }
        // Override de clique (1×) terminou → volta ao clipe base.
        if self.player.finished && self.clip_override.is_some_and(|o| !o.hold) {
            self.return_to_base();
        }
        self.pose_dirty
    }

    /// Recalcula pose → mundo → joints se algo mudou. Sem alocação.
    ///
    /// Pipeline drift-free dos bindings (PLANO_NATIVO §1.5 / Fase 4):
    /// - clipe avançou: repouso → amostra → guarda pose animada → soma bindings;
    /// - só bindings mudaram: **desfaz** (restaura a pose animada guardada) → soma bindings.
    pub fn refresh_pose(&mut self) {
        if !self.pose_dirty && !self.binding_dirty {
            return;
        }
        if self.pose_dirty {
            self.pose.reset_to_rest();
            if let Some(clip) = self.player.clip.and_then(|c| self.model.rig.animations.get(c)) {
                sample_clip(clip, self.player.time, &mut self.pose);
            }
            self.pose.store_animated();
        } else {
            self.pose.restore_animated();
        }
        self.additive.apply(&mut self.pose);
        self.binding_dirty = false;
        self.pose.world_matrices(&mut self.world);

        for binding in &self.skin_bindings {
            let Some(skin) = self.model.rig.skins.get(binding.skin as usize) else {
                continue;
            };
            for (j, &joint_node) in skin.joints.iter().enumerate() {
                let world = self.world.get(joint_node as usize).copied().unwrap_or(Mat4::IDENTITY);
                let inverse_bind = skin.inverse_bind.get(j).copied().unwrap_or(Mat4::IDENTITY);
                if let Some(slot) = self.joint_staging.get_mut(binding.joint_base as usize + j) {
                    *slot = (world * inverse_bind).to_cols_array();
                }
            }
        }
        self.pose_dirty = false;
        self.deform_dirty = true;
    }

    /// Sobe joints/pesos e despacha a deformação, se a pose mudou desde o último quadro.
    pub fn encode_deform(&mut self, queue: &wgpu::Queue, kernel: &DeformKernel, pass: &mut wgpu::ComputePass<'_>) {
        if !self.deform_dirty {
            return;
        }
        self.deform_dirty = false;
        let Some(deform) = &self.deform else {
            return;
        };
        if !self.joint_staging.is_empty() {
            queue.write_buffer(&deform.joints, 0, bytemuck::cast_slice(&self.joint_staging));
        }
        if !self.pose.weights.is_empty() {
            queue.write_buffer(&deform.weights, 0, bytemuck::cast_slice(&self.pose.weights));
        }
        pass.set_pipeline(&kernel.pipeline);
        for job in &deform.jobs {
            pass.set_bind_group(0, &deform.bind_group, &[job.offset]);
            pass.dispatch_workgroups(job.groups, 1, 1);
        }
    }

    /// Há deformação pendente (o chamador abre o compute pass só se alguma instância precisar).
    pub fn needs_deform(&self) -> bool {
        self.deform_dirty && self.deform.is_some()
    }

    /// Matriz de mundo do nó (espaço do modelo).
    #[inline]
    pub fn node_world(&self, node: u32) -> Mat4 {
        self.world.get(node as usize).copied().unwrap_or(Mat4::IDENTITY)
    }

    #[inline]
    pub fn primitive(&self, draw: &DrawRef) -> Option<&GpuPrimitive> {
        self.model.primitives.get(draw.prim as usize)
    }

    /// Fatia do stream 0 para `draw` (saída deformada ou geometria original).
    pub fn position_slice(&self, draw: &DrawRef, prim: &GpuPrimitive) -> wgpu::BufferSlice<'_> {
        match (draw.deformed, &self.deform) {
            (Some(first), Some(d)) => d.output.slice(u64::from(first) * DEFORM_STRIDE..),
            _ => self.model.vertex.slice(u64::from(prim.first_vertex) * DEFORM_STRIDE..),
        }
    }
}
