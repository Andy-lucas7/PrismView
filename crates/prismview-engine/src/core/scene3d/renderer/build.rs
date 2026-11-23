//! Construção de pipelines, bind groups e buffers do renderer 3D.

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Construção de recursos ─────────────────────────

pub(super) fn build_pbr_pipelines(shared: &Scene3dShared, device: &wgpu::Device, sample_count: u32) -> [wgpu::RenderPipeline; 4] {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("prismview_pbr"),
        bind_group_layouts: &[&shared.frame_layout, &shared.draw_layout, &shared.upload.material_layout],
        push_constant_ranges: &[],
    });
    let buffers = vertex_layouts();
    let make = |variant: usize| {
        let blend = variant >= 2;
        let double = variant % 2 == 1;
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prismview_pbr"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shared.pbr_module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shared.pbr_module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: blend.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: if double { None } else { Some(wgpu::Face::Back) },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: !blend,
                depth_compare: wgpu::CompareFunction::GreaterEqual, // reverse-Z
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        })
    };
    [make(0), make(1), make(2), make(3)]
}

pub(super) fn build_tonemap_pipeline(shared: &Scene3dShared, device: &wgpu::Device) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("prismview_tonemap"),
        bind_group_layouts: &[&shared.tonemap_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("prismview_tonemap"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shared.tonemap_module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shared.tonemap_module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OUTPUT_FORMAT,
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
}

pub(super) fn create_frame_bind(
    shared: &Scene3dShared,
    device: &wgpu::Device,
    frame_buffer: &wgpu::Buffer,
    env: &EnvironmentMaps,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("prismview_pbr_frame"),
        layout: &shared.frame_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&env.specular_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&env.irradiance_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&shared.brdf_lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&shared.env_sampler),
            },
        ],
    })
}

pub(super) fn create_draw_buffer(shared: &Scene3dShared, device: &wgpu::Device, capacity: usize) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("prismview_pbr_draws"),
        size: capacity.max(1) as u64 * DRAW_STRIDE,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("prismview_pbr_draws"),
        layout: &shared.draw_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    (buffer, bind)
}

/// Hora local fracionária (0..24) para o ciclo dia/noite.
pub(super) fn local_hour() -> f32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() as f32 + now.minute() as f32 / 60.0 + now.second() as f32 / 3600.0
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    pub(super) fn decodes_and_caps_dynamic_image_and_tracks_file_changes() {
        let dir = std::env::temp_dir().join(format!("prismview_dyntex_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cover.png");

        let img = image::RgbaImage::from_pixel(16, 8, image::Rgba([220, 40, 90, 255]));
        img.save(&path).expect("salva png de teste");

        let key1 = DynamicImageKey::from_path(path.clone()).expect("key inicial");
        let decoded = decode_dynamic_image(&path, 8).expect("decodifica e reduz");
        assert_eq!(decoded.width, 8);
        assert_eq!(decoded.height, 4);
        assert_eq!(decoded.rgba.len(), 8 * 4 * 4);

        let img2 = image::RgbaImage::from_pixel(24, 24, image::Rgba([10, 200, 120, 255]));
        img2.save(&path).expect("sobrescreve png");
        let key2 = DynamicImageKey::from_path(path.clone()).expect("key atualizada");
        assert_ne!(key1, key2);

        let empty_path = dir.join("empty.png");
        std::fs::write(&empty_path, b"").expect("arquivo vazio");
        assert!(DynamicImageKey::from_path(empty_path.clone()).is_none());
        assert!(decode_dynamic_image(&empty_path, 64).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// "Up" de referência da câmera: `Y`, ou `Z` quando o olhar é (quase) vertical — senão
/// `fwd × Y` zera e a view/o pick degeneram (câmera a ±90°).
pub(super) fn camera_up_ref(fwd: Vec3) -> Vec3 {
    if fwd.dot(Vec3::Y).abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

#[cfg(test)]
pub(super) mod camera_up_tests {
    use super::*;

    #[test]
    pub(super) fn vertical_view_still_has_a_right_vector() {
        for fwd in [Vec3::NEG_Y, Vec3::Y, Vec3::new(0.3, -0.9, 0.1).normalize()] {
            assert!(fwd.cross(camera_up_ref(fwd)).length() > 1e-3);
        }
    }
}
