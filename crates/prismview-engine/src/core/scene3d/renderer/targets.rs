//! Alvos de render (HDR, MSAA, profundidade, saída) e escala/MSAA efetivos.

#[allow(unused_imports)]
use super::*;

impl Scene3dRenderer {
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
    pub(super) fn apply_msaa(&mut self, device: &wgpu::Device, size: (u32, u32)) {
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

    /// Hibernação de VRAM (Fase 5): solta MSAA/HDR/profundidade/saída. O próximo `render`
    /// recria tudo (`dirty`), mantendo cena, modelos e ambiente.
    pub fn release_targets(&mut self) {
        self.targets = None;
        self.dirty = true;
    }

    pub(super) fn render_size(&self) -> (u32, u32) {
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

    pub(super) fn ensure_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
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

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| t.size)
    }
}
