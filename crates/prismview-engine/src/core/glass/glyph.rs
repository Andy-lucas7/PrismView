//! Mapas de normais dos glifos (vidro nas letras): blur de altura + Sobel na GPU.

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Mapas de glifo ─────────────────────────

pub(super) struct GlyphEntry {
    pub(super) _texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub(super) size: (u32, u32),
    pub(super) token: u64,
    pub(super) bevel_q: u32,
    pub(super) uid: u64,
    pub(super) used: u64,
}

pub(super) struct GlyphMaps {
    pub(super) gauss: wgpu::RenderPipeline,
    pub(super) pack: wgpu::RenderPipeline,
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) entries: HashMap<Arc<str>, GlyphEntry>,
    pub(super) serial: u64,
    pub(super) next_uid: u64,
}

impl GlyphMaps {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prismview_glass_glyph_layout"),
            entries: &[
                dynamic_uniform_entry(0),
                texture_entry(1, wgpu::ShaderStages::FRAGMENT),
                texture_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prismview_glass_glyph_pipeline_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_glass_glyph"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/glass_glyph.wgsl").into()),
        });
        Self {
            gauss: fullscreen_pipeline(
                device,
                "prismview_glass_glyph_gauss",
                &pipeline_layout,
                &module,
                "vs_full",
                "fs_gauss",
                GLYPH_FORMAT,
            ),
            pack: fullscreen_pipeline(
                device,
                "prismview_glass_glyph_pack",
                &pipeline_layout,
                &module,
                "vs_full",
                "fs_pack",
                GLYPH_FORMAT,
            ),
            layout,
            entries: HashMap::new(),
            serial: 0,
            next_uid: 1,
        }
    }

    pub(super) fn begin_build(&mut self) {
        self.serial += 1;
    }

    /// Libera mapas que a lista atual não referencia (chamado após o primeiro `prepare`).
    pub(super) fn sweep(&mut self) {
        let serial = self.serial;
        self.entries.retain(|_, e| e.used == serial);
    }

    pub(super) fn group(
        &self,
        device: &wgpu::Device,
        ring: &ParamRing,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_glass_glyph_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ring.binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(a),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(b),
                },
            ],
        })
    }

    pub(super) fn view(&self, key: &str) -> Option<(u64, &wgpu::TextureView)> {
        self.entries.get(key).map(|e| (e.uid, &e.view))
    }

    /// Garante o mapa de normais de `key` para este `token`. Gera: máscara → altura (blur H, V) →
    /// Sobel empacotado, tudo na GPU. `false` se faltou slot de parâmetros neste frame (tenta de
    /// novo no próximo) ou o tamanho é inválido.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        ring: &mut ParamRing,
        stats: &mut GlassStats,
        source: &GlyphSource,
        mask: &wgpu::TextureView,
    ) -> bool {
        let size = (
            source.size.0.clamp(1, MAX_GLYPH_DIM),
            source.size.1.clamp(1, MAX_GLYPH_DIM),
        );
        // Mesma conversão do TS: bevel = clamp(round(bevelPx * 0.48), 2, 32); σ = max(2, bevel * 0.55).
        let bevel = (fin(source.bevel_px, 0.0) * 0.48).round().clamp(2.0, 32.0);
        let bevel_q = bevel as u32;

        if let Some(e) = self.entries.get_mut(source.key.as_ref()) {
            if e.token == source.token && e.size == size && e.bevel_q == bevel_q {
                e.used = self.serial;
                return true;
            }
        }
        if ring.remaining() < 3 {
            return false;
        }

        let sigma = (bevel * 0.55).max(2.0);
        let radius = (sigma * 3.0).ceil().clamp(1.0, 64.0);
        let region = [size.0 as f32, size.1 as f32, 0.0, 0.0];

        let tmp = make_target(device, "prismview_glyph_tmp", size, GLYPH_FORMAT);
        let height = make_target(device, "prismview_glyph_height", size, GLYPH_FORMAT);
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prismview_glyph_normals"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: GLYPH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let out_view = out.create_view(&wgpu::TextureViewDescriptor::default());

        // Passe 1: blur horizontal máscara → tmp. Passe 2: vertical tmp → altura.
        let g_h = self.group(device, ring, mask, mask);
        let g_v = self.group(device, ring, &tmp.view, &tmp.view);
        let g_pack = self.group(device, ring, mask, &height.view);

        let params_h = PassParams {
            a: [1.0, 0.0, sigma, radius],
            b: region,
        };
        let params_v = PassParams {
            a: [0.0, 1.0, sigma, radius],
            b: region,
        };
        let params_pack = PassParams {
            a: [0.0; 4],
            b: region,
        };
        let (Some(off_h), Some(off_v), Some(off_pack)) = (
            ring.push(queue, &params_h),
            ring.push(queue, &params_v),
            ring.push(queue, &params_pack),
        ) else {
            return false;
        };
        record_fullscreen(encoder, "prismview_glyph_blur_h", &self.gauss, &g_h, off_h, &tmp.view);
        record_fullscreen(encoder, "prismview_glyph_blur_v", &self.gauss, &g_v, off_v, &height.view);
        record_fullscreen(encoder, "prismview_glyph_pack", &self.pack, &g_pack, off_pack, &out_view);
        stats.glyph_builds += 1;

        let uid = self.next_uid;
        self.next_uid += 1;
        self.entries.insert(
            Arc::clone(&source.key),
            GlyphEntry {
                _texture: out,
                view: out_view,
                size,
                token: source.token,
                bevel_q,
                uid,
                used: self.serial,
            },
        );
        true
    }
}
