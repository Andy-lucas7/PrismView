//! Pirâmide de blur do backdrop do vidro: downsample 2× anti-aliased (base = saída × qualidade, 4 níveis)
//! + Gaussiana separável densa de 15 taps (passo 1,0 texel), em paridade 1:1 com
//! `opticalGlassWebGL.ts` (zero ghosting, zero artefatos de caixa, cache LRU).

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Pirâmide de blur ─────────────────────────

const MIN_MIP0_DIM: u32 = 64;
const MIN_MIP_DIM: u32 = 16;

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
    /// `true` para imagem/vídeo opacos; `false` para preservar alpha pré-multiplicado (3D).
    pub opaque: bool,
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
    pub(super) size: (u32, u32),
    pub(super) key: Option<CacheKey>,
    pub(super) stamp: u64,
}

pub(super) struct Targets {
    pub(super) src_size: (u32, u32),
    pub(super) res_scale: f32,
    pub(super) sizes: [(u32, u32); MAX_LEVELS],
    /// Mips de downsample (níveis 0..MAX_LEVELS).
    pub(super) down: Vec<Tex>,
    /// Alvos temporários do passe horizontal da Gaussiana (1 por nível).
    pub(super) horiz: Vec<Tex>,
    /// Bind groups que LEEM `down[i]` / `horiz[i]`.
    pub(super) read_down: Vec<wgpu::BindGroup>,
    pub(super) read_horiz: Vec<wgpu::BindGroup>,
    /// Bind groups da fonte externa, por `BlurSource::id`.
    pub(super) src_groups: HashMap<u64, wgpu::BindGroup>,
    pub(super) slots: Vec<ResultSlot>,
    /// `(content_hash, frame_token)` da cadeia de descida e quantos níveis já estão calculados.
    pub(super) down_key: Option<(u64, u64)>,
    pub(super) down_depth: usize,
}

/// Tamanhos da pirâmide. `res_scale` = qualidade (1,0 = mip 0 na resolução da saída).
pub(super) fn mip_sizes(src: (u32, u32), res_scale: f32) -> [(u32, u32); MAX_LEVELS] {
    let sw = src.0.max(1);
    let sh = src.1.max(1);
    let scale = if res_scale.is_finite() { res_scale.clamp(0.1, 1.0) } else { 1.0 };
    let mut w = ((sw as f32 * scale).round() as u32).max(MIN_MIP0_DIM.min(sw));
    let mut h = ((sh as f32 * scale).round() as u32).max(MIN_MIP0_DIM.min(sh));
    let mut sizes = [(1, 1); MAX_LEVELS];
    for size in &mut sizes {
        *size = (w.max(1), h.max(1));
        w = ((w as f32 * 0.5).round() as u32).max(MIN_MIP_DIM.min(sw));
        h = ((h as f32 * 0.5).round() as u32).max(MIN_MIP_DIM.min(sh));
    }
    sizes
}

pub(super) fn quantize_blur(src_px: f32) -> u16 {
    let q = (src_px.clamp(0.5, MAX_BLUR_SRC_PX) * 2.0).round();
    if q.is_finite() {
        q as u16
    } else {
        1
    }
}

/// Converte o blur quantizado (em texels da fonte) em `(nível, sigma_no_mip)`.
/// Escolhe o menor nível onde `sigma <= 3.2` (kernel de 15 taps cobre >98,5% da Gaussiana com
/// passo exato de 1,0 texel — zero ghosting!).
pub(super) fn plan_blur(blur_q: u16, src_w: u32, mip0_w: u32) -> (usize, f32) {
    let blur_src_px = f32::from(blur_q) * 0.5;
    let mip0_scale = mip0_w.max(1) as f32 / src_w.max(1) as f32;
    let mut sigma = (blur_src_px * SIGMA_PER_BLUR_PX * mip0_scale).max(0.4);
    let mut level = 0usize;
    while level + 1 < MAX_LEVELS && sigma > 3.2 {
        level += 1;
        sigma *= 0.5;
    }
    (level, sigma.clamp(0.45, 4.2))
}

pub(super) struct BlurPyramid {
    pub(super) down: wgpu::RenderPipeline,
    pub(super) gauss: wgpu::RenderPipeline,
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) sampler: wgpu::Sampler,
    pub(super) targets: Option<Targets>,
    /// Muda a cada recriação dos alvos (invalida bind groups que apontam para eles).
    pub(super) epoch: u32,
    pub(super) clock: u64,
    /// Slots com `stamp >= frame_floor` já foram usados neste frame e não podem ser reciclados.
    pub(super) frame_floor: u64,
    /// Qualidade: fração da resolução da saída no mip 0 (`RenderQuality`).
    pub(super) res_scale: f32,
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
            down: make("prismview_glass_down", "fs_down"),
            gauss: make("prismview_glass_gauss", "fs_gauss"),
            layout,
            sampler: linear_clamp_sampler(device),
            targets: None,
            epoch: 0,
            clock: 1,
            frame_floor: 1,
            res_scale: 1.0,
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
        if self.targets.as_ref().is_some_and(|t| t.src_size == src && t.res_scale == self.res_scale) {
            return;
        }
        let sizes = mip_sizes(src, self.res_scale);
        let down: Vec<Tex> = sizes
            .iter()
            .map(|&s| make_target(device, "prismview_glass_down", s, PYRAMID_FORMAT))
            .collect();
        let horiz: Vec<Tex> = sizes
            .iter()
            .map(|&s| make_target(device, "prismview_glass_horiz", s, PYRAMID_FORMAT))
            .collect();
        let read_down: Vec<wgpu::BindGroup> =
            down.iter().map(|t| self.group(device, ring, &t.view)).collect();
        let read_horiz: Vec<wgpu::BindGroup> =
            horiz.iter().map(|t| self.group(device, ring, &t.view)).collect();
        let slots = Vec::with_capacity(RESULT_SLOTS);
        self.epoch = self.epoch.wrapping_add(1);
        self.targets = Some(Targets {
            src_size: src,
            res_scale: self.res_scale,
            sizes,
            down,
            horiz,
            read_down,
            read_horiz,
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
    /// `None`: sem blur necessário, fonte inválida ou recursos do frame esgotados.
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

        // Bind group da fonte externa (criado só se faltar).
        let need_ext = {
            let t = self.targets.as_ref()?;
            !t.src_groups.contains_key(&src.id)
        };
        let ext_group = if need_ext {
            Some(self.group(device, ring, src.view))
        } else {
            None
        };

        let mut bumped_epoch = false;
        let t = self.targets.as_mut()?;

        // Acerto de cache.
        if let Some(i) = t.slots.iter().position(|s| s.key == Some(key)) {
            t.slots[i].stamp = now;
            stats.blur_hits += 1;
            return Some(BlurHandle { slot: i });
        }

        let (level, sigma) = plan_blur(blur_q, src.width, t.sizes[0].0);
        let mip_size = t.sizes[level];
        let needs_mip0_down = src.width as f32 > t.sizes[0].0 as f32 * 1.15
            || src.height as f32 > t.sizes[0].1 as f32 * 1.15;

        // Vítima LRU para o resultado do nível `level`.
        let victim = match t
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.stamp < floor)
            .min_by_key(|(_, s)| (u8::from(s.size != mip_size), s.stamp))
            .map(|(i, _)| i)
        {
            Some(i) => {
                if t.slots[i].size != mip_size {
                    t.slots[i].tex = make_target(device, "prismview_glass_result", mip_size, PYRAMID_FORMAT);
                    t.slots[i].size = mip_size;
                    t.slots[i].key = None;
                    bumped_epoch = true;
                }
                i
            }
            None if t.slots.len() < RESULT_SLOTS => {
                let idx = t.slots.len();
                t.slots.push(ResultSlot {
                    tex: make_target(device, "prismview_glass_result", mip_size, PYRAMID_FORMAT),
                    size: mip_size,
                    key: None,
                    stamp: 0,
                });
                bumped_epoch = true;
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
        let start_lvl = if needs_mip0_down { 0 } else { 1 };
        let need_down = needs_mip0_down || level > 0;
        let first_down = t.down_depth.max(start_lvl);
        let down_passes = if need_down && level >= first_down {
            level - first_down + 1
        } else {
            0
        };
        if ring.remaining() < down_passes + 2 {
            return None;
        }

        if let Some(group) = ext_group {
            if t.src_groups.len() >= SRC_GROUP_CACHE_LIMIT {
                t.src_groups.clear();
            }
            t.src_groups.insert(src.id, group);
        }

        // Passo 1: Downsample 2× progressivo (anti-aliased, 4 taps bilineares).
        let force_opaque = if req.opaque { 1.0 } else { 0.0 };
        if need_down && level >= first_down {
            for lvl in first_down..=level {
                let (group, src_sz) = if lvl == start_lvl {
                    (t.src_groups.get(&src.id)?, (src.width, src.height))
                } else {
                    (&t.read_down[lvl - 1], t.sizes[lvl - 1])
                };
                let params = PassParams {
                    a: [
                        0.5 / src_sz.0.max(1) as f32,
                        0.5 / src_sz.1.max(1) as f32,
                        0.0,
                        if lvl == start_lvl { force_opaque } else { 0.0 },
                    ],
                    b: [0.0; 4],
                };
                let dyn_off = ring.push(queue, &params)?;
                record_fullscreen(
                    encoder,
                    "prismview_glass_down_pass",
                    &self.down,
                    group,
                    dyn_off,
                    &t.down[lvl].view,
                );
                stats.blur_passes += 1;
            }
            t.down_depth = t.down_depth.max(level + 1);
        }

        // Passo 2: Gaussiana separável densa de 15 taps (H → horiz[level], V → slots[victim]).
        let gauss_in = if need_down {
            &t.read_down[level]
        } else {
            t.src_groups.get(&src.id)?
        };
        let params_h = PassParams {
            a: [
                1.0 / mip_size.0.max(1) as f32,
                0.0,
                sigma,
                if need_down { 0.0 } else { force_opaque },
            ],
            b: [0.0; 4],
        };
        let off_h = ring.push(queue, &params_h)?;
        record_fullscreen(
            encoder,
            "prismview_glass_gauss_h",
            &self.gauss,
            gauss_in,
            off_h,
            &t.horiz[level].view,
        );
        stats.blur_passes += 1;

        let params_v = PassParams {
            a: [0.0, 1.0 / mip_size.1.max(1) as f32, sigma, 0.0],
            b: [0.0; 4],
        };
        let off_v = ring.push(queue, &params_v)?;
        record_fullscreen(
            encoder,
            "prismview_glass_gauss_v",
            &self.gauss,
            &t.read_horiz[level],
            off_v,
            &t.slots[victim].tex.view,
        );
        stats.blur_passes += 1;

        let slot = &mut t.slots[victim];
        slot.key = Some(key);
        slot.stamp = now;
        stats.blur_misses += 1;
        if bumped_epoch {
            self.epoch = self.epoch.wrapping_add(1);
        }
        Some(BlurHandle { slot: victim })
    }
}
