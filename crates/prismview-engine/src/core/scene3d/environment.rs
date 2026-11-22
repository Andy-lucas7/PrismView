//! IBL da cena 3D, todo na GPU (compute WGSL em `shaders/pmrem.wgsl`):
//!
//! 1. cubo base `Rgba16Float` 256² com mips completos, gerado do ambiente procedural (fundo +
//!    cards do preset) ou de um HDRI equirretangular;
//! 2. cubo especular pré-filtrado (GGX, 6 mips, rugosidade = mip / 5) por filtered importance
//!    sampling sobre os mips do cubo base;
//! 3. cubo de irradiância 32² (convolução de cosseno);
//! 4. LUT do BRDF split-sum (uma vez por device).
//!
//! Só roda quando o ambiente muda (preset/HDRI): nunca no loop de quadro. Os buffers de parâmetros
//! de cada dispatch são criados aqui (evento de troca de asset, não quadro).

use std::path::Path;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::lighting::{ProceduralEnv, MAX_ENV_CARDS};

pub const BASE_SIZE: u32 = 256;
pub const BASE_MIPS: u32 = 9; // 256 → 1
pub const SPEC_SIZE: u32 = 256;
pub const SPEC_MIPS: u32 = 6; // 256 → 8
pub const IRR_SIZE: u32 = 32;
pub const LUT_SIZE: u32 = 128;
pub const ENV_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Amostras GGX por mip do cubo especular (mip 0 = espelho, 1 amostra).
const SPEC_SAMPLES: [u32; SPEC_MIPS as usize] = [1, 64, 96, 128, 192, 256];
const IRR_SAMPLES: u32 = 256;
/// Largura máxima mantida de um HDRI (2:1). O cubo base tem 256²; 2048 já sobra.
pub const HDRI_MAX_WIDTH: u32 = 2048;

// ───────────────────────── HDRI ─────────────────────────

/// Imagem equirretangular linear RGBA32F (CPU).
pub struct EquirectImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
}

/// Lê um `.hdr` (Radiance) e reduz para no máximo `HDRI_MAX_WIDTH`. Usar fora do thread de render.
pub fn load_hdri(path: &Path) -> Result<EquirectImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("HDRI: {e}"))?;
    drop(bytes);
    let mut rgba = img.into_rgba32f();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err("HDRI vazio".into());
    }
    if w > HDRI_MAX_WIDTH {
        let nh = ((h as f32 * HDRI_MAX_WIDTH as f32 / w as f32) as u32).max(1);
        rgba = image::imageops::resize(&rgba, HDRI_MAX_WIDTH, nh, image::imageops::FilterType::Triangle);
    }
    let (width, height) = rgba.dimensions();
    Ok(EquirectImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

pub enum EnvSource<'a> {
    Procedural(&'a ProceduralEnv),
    /// HDRI + multiplicador de radiância.
    Equirect(&'a EquirectImage, f32),
}

// ───────────────────────── Uniform ─────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EnvParamsGpu {
    background: [f32; 4],
    card_center: [[f32; 4]; MAX_ENV_CARDS],
    card_u: [[f32; 4]; MAX_ENV_CARDS],
    card_v: [[f32; 4]; MAX_ENV_CARDS],
    card_radiance: [[f32; 4]; MAX_ENV_CARDS],
    info: [f32; 4],
    info2: [f32; 4],
}

impl EnvParamsGpu {
    fn base(out_size: u32) -> Self {
        let mut p = Self::zeroed();
        p.info = [out_size as f32, 0.0, 1.0, BASE_SIZE as f32];
        p.info2 = [BASE_MIPS as f32, 1.0, 0.0, 0.0];
        p
    }

    fn procedural(env: &ProceduralEnv) -> Self {
        let mut p = Self::base(BASE_SIZE);
        p.background = env.background.extend(1.0).to_array();
        for (i, c) in env.cards.iter().take(env.card_count).enumerate() {
            p.card_center[i] = c.center.extend(0.0).to_array();
            p.card_u[i] = c.half_u.extend(0.0).to_array();
            p.card_v[i] = c.half_v.extend(0.0).to_array();
            p.card_radiance[i] = c.radiance.extend(1.0).to_array();
        }
        p.info2[2] = env.card_count as f32;
        p
    }
}

// ───────────────────────── Resultado ─────────────────────────

pub struct EnvironmentMaps {
    _specular: wgpu::Texture,
    _irradiance: wgpu::Texture,
    pub specular_view: wgpu::TextureView,
    pub irradiance_view: wgpu::TextureView,
    /// Último mip do cubo especular (rugosidade 1).
    pub max_mip: f32,
}

// ───────────────────────── Baker ─────────────────────────

// Bindings de `pmrem.wgsl`.
const B_PARAMS: u32 = 0;
const B_OUT_CUBE: u32 = 1;
const B_EQUIRECT: u32 = 2;
const B_SRC_MIP: u32 = 3;
const B_ENV_CUBE: u32 = 4;
const B_SAMPLER: u32 = 5;
const B_OUT_LUT: u32 = 6;

struct Stage {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

pub struct EnvBaker {
    procedural: Stage,
    equirect: Stage,
    downsample: Stage,
    prefilter: Stage,
    irradiance: Stage,
    brdf: Stage,
    sampler: wgpu::Sampler,
}

fn layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    let ty = match binding {
        B_PARAMS => wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<EnvParamsGpu>() as u64),
        },
        B_OUT_CUBE => wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: ENV_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        },
        B_EQUIRECT => wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        B_SRC_MIP => wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        B_ENV_CUBE => wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::Cube,
            multisampled: false,
        },
        B_SAMPLER => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        _ => wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: ENV_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
    };
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    }
}

fn stage(device: &wgpu::Device, module: &wgpu::ShaderModule, entry: &'static str, bindings: &[u32]) -> Stage {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = bindings.iter().map(|&b| layout_entry(b)).collect();
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(entry),
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(&pipeline_layout),
        module,
        entry_point: Some(entry),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });
    Stage { layout, pipeline }
}

fn cube_texture(device: &wgpu::Device, label: &str, size: u32, mips: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ENV_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn mip_view(texture: &wgpu::Texture, mip: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("prismview_env_mip"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: mip,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(6),
        ..Default::default()
    })
}

fn cube_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("prismview_env_cube"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        array_layer_count: Some(6),
        ..Default::default()
    })
}

#[inline]
fn groups(size: u32) -> u32 {
    size.div_ceil(8).max(1)
}

impl EnvBaker {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prismview_pmrem"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/pmrem.wgsl").into()),
        });
        Self {
            procedural: stage(device, &module, "cs_sky_procedural", &[B_PARAMS, B_OUT_CUBE]),
            equirect: stage(device, &module, "cs_sky_equirect", &[B_PARAMS, B_OUT_CUBE, B_EQUIRECT]),
            downsample: stage(device, &module, "cs_downsample", &[B_PARAMS, B_OUT_CUBE, B_SRC_MIP]),
            prefilter: stage(
                device,
                &module,
                "cs_prefilter",
                &[B_PARAMS, B_OUT_CUBE, B_ENV_CUBE, B_SAMPLER],
            ),
            irradiance: stage(
                device,
                &module,
                "cs_irradiance",
                &[B_PARAMS, B_OUT_CUBE, B_ENV_CUBE, B_SAMPLER],
            ),
            brdf: stage(device, &module, "cs_brdf_lut", &[B_OUT_LUT]),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("prismview_env_bake_sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    fn params(device: &wgpu::Device, p: &EnvParamsGpu) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("prismview_env_params"),
            contents: bytemuck::bytes_of(p),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    }

    /// LUT do BRDF (A, B em RG). Uma vez por device.
    pub fn bake_brdf_lut(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("prismview_brdf_lut"),
            size: wgpu::Extent3d {
                width: LUT_SIZE,
                height: LUT_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ENV_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_brdf_lut"),
            layout: &self.brdf.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: B_OUT_LUT,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("prismview_brdf_lut"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("prismview_brdf_lut"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.brdf.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups(LUT_SIZE), groups(LUT_SIZE), 1);
        }
        queue.submit(std::iter::once(encoder.finish()));
        (texture, view)
    }

    /// Gera os cubos especular + irradiância de `source`.
    pub fn bake(&self, device: &wgpu::Device, queue: &wgpu::Queue, source: EnvSource<'_>) -> EnvironmentMaps {
        let base = cube_texture(device, "prismview_env_base", BASE_SIZE, BASE_MIPS);
        let specular = cube_texture(device, "prismview_env_specular", SPEC_SIZE, SPEC_MIPS);
        let irradiance = cube_texture(device, "prismview_env_irradiance", IRR_SIZE, 1);
        let base_mips: Vec<wgpu::TextureView> = (0..BASE_MIPS).map(|m| mip_view(&base, m)).collect();
        let base_cube = cube_view(&base);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("prismview_env_bake"),
        });

        // 1. Cubo base, mip 0. Os recursos temporários vivem até o submit.
        let mut keep_alive: Vec<wgpu::Buffer> = Vec::with_capacity(4 + BASE_MIPS as usize + SPEC_MIPS as usize);
        let mut equirect_texture: Option<wgpu::Texture> = None;
        let sky_bind = match source {
            EnvSource::Procedural(env) => {
                let params = Self::params(device, &EnvParamsGpu::procedural(env));
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("prismview_env_sky"),
                    layout: &self.procedural.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: B_PARAMS,
                            resource: params.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: B_OUT_CUBE,
                            resource: wgpu::BindingResource::TextureView(&base_mips[0]),
                        },
                    ],
                });
                keep_alive.push(params);
                (bind, &self.procedural.pipeline)
            }
            EnvSource::Equirect(img, scale) => {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("prismview_env_equirect"),
                    size: wgpu::Extent3d {
                        width: img.width.max(1),
                        height: img.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let expected = img.width as usize * img.height as usize * 4;
                if img.rgba.len() >= expected && expected > 0 {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytemuck::cast_slice(&img.rgba[..expected]),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(img.width * 16),
                            rows_per_image: Some(img.height),
                        },
                        wgpu::Extent3d {
                            width: img.width,
                            height: img.height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut p = EnvParamsGpu::base(BASE_SIZE);
                p.info2[1] = if scale.is_finite() { scale.max(0.0) } else { 1.0 };
                let params = Self::params(device, &p);
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("prismview_env_equirect"),
                    layout: &self.equirect.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: B_PARAMS,
                            resource: params.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: B_OUT_CUBE,
                            resource: wgpu::BindingResource::TextureView(&base_mips[0]),
                        },
                        wgpu::BindGroupEntry {
                            binding: B_EQUIRECT,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                    ],
                });
                keep_alive.push(params);
                equirect_texture = Some(texture);
                (bind, &self.equirect.pipeline)
            }
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("prismview_env_sky"),
                timestamp_writes: None,
            });
            pass.set_pipeline(sky_bind.1);
            pass.set_bind_group(0, &sky_bind.0, &[]);
            pass.dispatch_workgroups(groups(BASE_SIZE), groups(BASE_SIZE), 6);
        }

        // 2. Mips do cubo base (cada mip lê o anterior; passes separados = barreira garantida).
        for mip in 1..BASE_MIPS {
            let size = (BASE_SIZE >> mip).max(1);
            let params = Self::params(device, &EnvParamsGpu::base(size));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("prismview_env_downsample"),
                layout: &self.downsample.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: B_PARAMS,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: B_OUT_CUBE,
                        resource: wgpu::BindingResource::TextureView(&base_mips[mip as usize]),
                    },
                    wgpu::BindGroupEntry {
                        binding: B_SRC_MIP,
                        resource: wgpu::BindingResource::TextureView(&base_mips[mip as usize - 1]),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("prismview_env_downsample"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.downsample.pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(groups(size), groups(size), 6);
            }
            keep_alive.push(params);
        }

        // 3. Especular pré-filtrado + 4. irradiância (leem o cubo base inteiro).
        let spec_mips: Vec<wgpu::TextureView> = (0..SPEC_MIPS).map(|m| mip_view(&specular, m)).collect();
        let irr_out = mip_view(&irradiance, 0);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("prismview_env_prefilter"),
                timestamp_writes: None,
            });
            let mut binds: Vec<(wgpu::BindGroup, u32, bool)> = Vec::with_capacity(SPEC_MIPS as usize + 1);
            for mip in 0..SPEC_MIPS {
                let size = (SPEC_SIZE >> mip).max(1);
                let mut p = EnvParamsGpu::base(size);
                p.info[1] = mip as f32 / (SPEC_MIPS - 1) as f32;
                p.info[2] = SPEC_SAMPLES[mip as usize] as f32;
                let params = Self::params(device, &p);
                let bind = self.filter_bind(device, &self.prefilter.layout, &params, &spec_mips[mip as usize], &base_cube);
                keep_alive.push(params);
                binds.push((bind, size, true));
            }
            let mut p = EnvParamsGpu::base(IRR_SIZE);
            p.info[2] = IRR_SAMPLES as f32;
            let params = Self::params(device, &p);
            let bind = self.filter_bind(device, &self.irradiance.layout, &params, &irr_out, &base_cube);
            keep_alive.push(params);
            binds.push((bind, IRR_SIZE, false));

            for (bind, size, is_spec) in &binds {
                pass.set_pipeline(if *is_spec {
                    &self.prefilter.pipeline
                } else {
                    &self.irradiance.pipeline
                });
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(groups(*size), groups(*size), 6);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
        drop(keep_alive);
        drop(equirect_texture);

        EnvironmentMaps {
            specular_view: cube_view(&specular),
            irradiance_view: cube_view(&irradiance),
            _specular: specular,
            _irradiance: irradiance,
            max_mip: (SPEC_MIPS - 1) as f32,
        }
    }

    fn filter_bind(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        out: &wgpu::TextureView,
        cube: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prismview_env_filter"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: B_PARAMS,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: B_OUT_CUBE,
                    resource: wgpu::BindingResource::TextureView(out),
                },
                wgpu::BindGroupEntry {
                    binding: B_ENV_CUBE,
                    resource: wgpu::BindingResource::TextureView(cube),
                },
                wgpu::BindGroupEntry {
                    binding: B_SAMPLER,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}
