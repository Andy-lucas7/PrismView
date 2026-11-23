//! Pirâmide de blur Dual Kawase do backdrop do vidro (alvos só no resize, cache LRU).

#[allow(unused_imports)]
use super::*;

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
    pub(super) slot: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct CacheKey {
    pub(super) content_hash: u64,
    pub(super) frame_token: u64,
    pub(super) blur_q: u16,
    pub(super) src: (u32, u32),
}

pub(super) struct ResultSlot {
    pub(super) tex: Tex,
    pub(super) key: Option<CacheKey>,
    pub(super) stamp: u64,
}

pub(super) struct Targets {
    pub(super) src_size: (u32, u32),
    pub(super) sizes: [(u32, u32); MAX_LEVELS],
    /// D0..D4: cadeia de descida.
    pub(super) down: Vec<Tex>,
    /// U1..U4 (índice `i - 1`): cadeia de subida intermediária. U0 é o slot de resultado.
    pub(super) up: Vec<Tex>,
    /// Bind groups que LEEM D_i / U_i (reaproveitados por descida e subida).
    pub(super) read_down: Vec<wgpu::BindGroup>,
    pub(super) read_up: Vec<wgpu::BindGroup>,
    /// Bind groups da fonte externa, por `BlurSource::id`.
    pub(super) src_groups: HashMap<u64, wgpu::BindGroup>,
    pub(super) slots: Vec<ResultSlot>,
    /// `(content_hash, frame_token)` da cadeia de descida e quantos níveis já estão calculados.
    pub(super) down_key: Option<(u64, u64)>,
    pub(super) down_depth: usize,
}

pub(super) fn level_divisor(level: usize) -> u32 {
    1u32 << (level + 2)
}

pub(super) fn level_size(src: (u32, u32), level: usize) -> (u32, u32) {
    let d = level_divisor(level);
    (
        src.0.div_ceil(d).max(MIN_LEVEL_DIM),
        src.1.div_ceil(d).max(MIN_LEVEL_DIM),
    )
}

pub(super) fn quantize_blur(src_px: f32) -> u16 {
    let q = (src_px.clamp(0.5, MAX_BLUR_SRC_PX) * 2.0).round();
    if q.is_finite() {
        q as u16
    } else {
        1
    }
}

/// Converte o blur quantizado em (nível mais profundo, offset da subida). Escolhe o menor nível
/// capaz de atingir o sigma com `offset <= MAX_OFFSET` (menos pixels processados).
pub(super) fn plan_blur(blur_q: u16) -> (usize, f32) {
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

pub(super) struct BlurPyramid {
    pub(super) down_first: wgpu::RenderPipeline,
    pub(super) down: wgpu::RenderPipeline,
    pub(super) up: wgpu::RenderPipeline,
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) sampler: wgpu::Sampler,
    pub(super) targets: Option<Targets>,
    /// Muda a cada recriação dos alvos (invalida bind groups que apontam para eles).
    pub(super) epoch: u32,
    pub(super) clock: u64,
    /// Slots com `stamp >= frame_floor` já foram usados neste frame e não podem ser reciclados.
    pub(super) frame_floor: u64,
}

impl BlurPyramid {
    pub(super) fn new(device: &wgpu::Device) -> Self {
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/glass_blur.wgsl").into()),
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

    pub(super) fn begin_frame(&mut self) {
        self.frame_floor = self.clock + 1;
    }

    pub(super) fn group(
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
    pub(super) fn ensure_targets(&mut self, device: &wgpu::Device, ring: &ParamRing, src: (u32, u32)) {
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
        // Slots de resultado são alocados sob demanda até `RESULT_SLOTS` (cena com 1 vidro = 1 alvo).
        let slots = Vec::with_capacity(RESULT_SLOTS);
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

    pub(super) fn release(&mut self) {
        self.targets = None;
        self.epoch = self.epoch.wrapping_add(1);
    }

    pub(super) fn invalidate_content(&mut self, content_hash: u64) {
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

    pub(super) fn view(&self, handle: BlurHandle) -> Option<&wgpu::TextureView> {
        self.targets
            .as_ref()
            .and_then(|t| t.slots.get(handle.slot))
            .map(|s| &s.tex.view)
    }

    /// Garante o blur de `req` para `src` e devolve o slot. Grava os passes em `encoder`.
    /// `None`: sem blur necessário, fonte inválida ou recursos do frame esgotados (o chamador
    /// amostra o backdrop nítido).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn blur(
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

        // Vítima: reaproveita slot ainda não usado neste frame (LRU); se todos estiverem em uso
        // neste frame (múltiplos blurs distintos), aloca mais um sob demanda até `RESULT_SLOTS`.
        let victim = match t
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.stamp < floor)
            .min_by_key(|(_, s)| s.stamp)
            .map(|(i, _)| i)
        {
            Some(i) => i,
            None if t.slots.len() < RESULT_SLOTS => {
                let idx = t.slots.len();
                t.slots.push(ResultSlot {
                    tex: make_target(device, "prismview_glass_result", t.sizes[0], PYRAMID_FORMAT),
                    key: None,
                    stamp: 0,
                });
                idx
            }
            None => return None,
        };

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
