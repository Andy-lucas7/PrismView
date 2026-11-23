//! Uniforms da GPU e recursos compartilhados por device (`Scene3dShared`).

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Uniforms ─────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct FrameUniform {
    pub(super) view_proj: [f32; 16],
    pub(super) camera_pos: [f32; 4],
    pub(super) ambient: [f32; 4],
    pub(super) env: [f32; 4],
    pub(super) dir: [[f32; 8]; MAX_DIR_LIGHTS],
    pub(super) point: [[f32; 8]; MAX_POINT_LIGHTS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct DrawGpu {
    pub(super) model: [f32; 16],
    pub(super) normal: [f32; 16],
    /// x = brilho emissivo extra dos bindings (`EMISSIVE_GLOW`); yzw livres.
    pub(super) glow: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct TonemapUniform {
    pub(super) info: [f32; 4],
    pub(super) background: [f32; 4],
}

// ───────────────────────── Recursos por device ─────────────────────────

/// Pipelines, layouts, LUT do BRDF e cache de modelos na GPU. 1 por device, compartilhado entre
/// renderers (um por monitor/camada) via `Arc`.
pub struct Scene3dShared {
    pub upload: ModelUploadContext,
    pub(super) baker: EnvBaker,
    pub(super) _brdf_lut: wgpu::Texture,
    pub(super) brdf_lut_view: wgpu::TextureView,
    pub(super) env_sampler: wgpu::Sampler,
    pub(super) frame_layout: wgpu::BindGroupLayout,
    pub(super) draw_layout: wgpu::BindGroupLayout,
    pub(super) tonemap_layout: wgpu::BindGroupLayout,
    pub(super) deform: DeformKernel,
    pub(super) pbr_module: wgpu::ShaderModule,
    pub(super) tonemap_module: wgpu::ShaderModule,
    pub(super) models: Mutex<HashMap<AssetHash, Weak<GpuModel>>>,
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
                source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/pbr.wgsl").into()),
            }),
            tonemap_module: device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("prismview_tonemap"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../../shaders/tonemap.wgsl").into()),
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
