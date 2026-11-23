//! Passe de desenho do quadro (PBR → tonemap).

#[allow(unused_imports)]
use super::*;

impl Scene3dRenderer {
    pub(super) fn frame_uniform(&self, view_proj: &Mat4, eye: Vec3) -> FrameUniform {
        let mut u = FrameUniform::zeroed();
        u.view_proj = view_proj.to_cols_array();
        u.camera_pos = eye.extend(1.0).to_array();
        u.ambient = self.lights.ambient.extend(0.0).to_array();
        let env_intensity = if self.config.env_intensity.is_finite() {
            self.config.env_intensity.clamp(0.0, 3.0)
        } else {
            1.2
        };
        u.env = [
            self.env.max_mip,
            self.lights.dir_count as f32,
            self.lights.point_count as f32,
            env_intensity,
        ];
        for (dst, l) in u.dir.iter_mut().zip(self.lights.dir.iter()).take(self.lights.dir_count) {
            let d = l.direction;
            let c = l.radiance;
            *dst = [d.x, d.y, d.z, 0.0, c.x, c.y, c.z, 0.0];
        }
        for (dst, l) in u.point.iter_mut().zip(self.lights.point.iter()).take(self.lights.point_count) {
            let p = l.position;
            let c = l.radiance;
            *dst = [p.x, p.y, p.z, l.range, c.x, c.y, c.z, l.decay];
        }
        u
    }

    /// Grava o quadro em `encoder` (deformação + PBR + tone mapping). `false` se não havia o que
    /// desenhar (viewport 0). O chamador submete o encoder.
    pub fn render(&mut self, gpu: &GpuContext, encoder: &mut wgpu::CommandEncoder) -> bool {
        if self.viewport.0 == 0 || self.viewport.1 == 0 {
            return false;
        }
        let device = gpu.device();
        let queue = gpu.queue();
        let size = self.render_size();
        self.apply_msaa(device, size);
        self.ensure_targets(device, size);

        // 1. Matrizes de objeto (pai antes do filho).
        for &i in &self.graph.order {
            let (Some(obj), Some(slot)) = (self.objects.get(i), self.slots.get(i)) else {
                continue;
            };
            let local = obj.local_matrix_with_spin(slot.spin);
            let parent = self.graph.parent.get(i).copied().flatten();
            let world = match parent.and_then(|p| self.object_world.get(p)) {
                Some(pw) => *pw * local,
                None => local,
            };
            if let Some(dst) = self.object_world.get_mut(i) {
                *dst = world;
            }
        }

        // 2-3. Pose e deformação na GPU (só o que mudou).
        let mut any_deform = false;
        for slot in &mut self.slots {
            if let Some(instance) = &mut slot.instance {
                instance.refresh_pose();
                any_deform |= instance.needs_deform();
            }
        }
        if any_deform {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("prismview_3d_deform"),
                timestamp_writes: None,
            });
            for slot in &mut self.slots {
                if let Some(instance) = &mut slot.instance {
                    instance.encode_deform(queue, &self.shared.deform, &mut pass);
                }
            }
        }

        // 4. Câmera (reverse-Z infinito).
        let (eye, target, fov) = self.camera_view();
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let proj = Mat4::perspective_infinite_reverse_rh(fov.to_radians(), aspect, NEAR_PLANE);
        let view = Mat4::look_at_rh(eye, target, camera_up_ref((target - eye).normalize_or_zero()));
        let view_proj = proj * view;
        let frustum = Frustum::from_view_proj(&view_proj);

        // 5. Lista de draws com culling (sem alocar: capacidade reservada na troca de cena).
        self.draw_staging.clear();
        self.opaque.clear();
        self.blended.clear();
        let mut culled = 0u32;
        let mut vram: u64 = self
            .dynamic_textures
            .values()
            .map(|entry| match entry {
                DynamicTexSlot::Ready { vram_bytes, .. } => *vram_bytes,
                DynamicTexSlot::Loading | DynamicTexSlot::Failed => 0,
            })
            .sum();
        let mut instances = 0u32;
        let capacity = self.draw_staging.capacity().min(self.draw_capacity);
        for (oi, slot) in self.slots.iter().enumerate() {
            let Some(instance) = &slot.instance else {
                continue;
            };
            instances += 1;
            vram += instance.model.vram_bytes;
            if !self.visible.get(oi).copied().unwrap_or(false) {
                continue;
            }
            let object = self.object_world.get(oi).copied().unwrap_or(Mat4::IDENTITY);
            // `__ROOT__` dos bindings: em torno do centro do modelo normalizado.
            let base = object * instance.root_matrix() * instance.normalize;
            for (di, draw) in instance.draws.iter().enumerate() {
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let (model, local_bounds) = if draw.skinned {
                    (base, instance.skinned_bounds)
                } else {
                    (base * instance.node_world(draw.node), prim.bounds)
                };
                let world_bounds = local_bounds.transformed(&model);
                if !frustum.intersects_aabb(&world_bounds) {
                    culled += 1;
                    continue;
                }
                if self.draw_staging.len() >= capacity {
                    break;
                }
                let Some(material) = slot.material(instance, prim.material) else {
                    continue;
                };
                let m3 = Mat3::from_mat4(model);
                let det = m3.determinant();
                let normal = if det.abs() > 1e-12 && det.is_finite() {
                    Mat4::from_mat3(m3.inverse().transpose())
                } else {
                    model
                };
                let slot_index = u32::try_from(self.draw_staging.len()).unwrap_or(u32::MAX);
                self.draw_staging.push(DrawGpu {
                    model: model.to_cols_array(),
                    normal: normal.to_cols_array(),
                    glow: [instance.glow(prim.material), 0.0, 0.0, 0.0],
                });
                // Espelhado (det < 0) inverte o winding: usa a variante sem culling.
                let double = material.double_sided || det < 0.0;
                let variant = u8::from(material.blend) * 2 + u8::from(double);
                let center = (world_bounds.min + world_bounds.max) * 0.5;
                let depth = view.transform_point3(center).z; // RH: mais longe = mais negativo
                let cmd = DrawCmd {
                    key: (u64::from(variant) << 56)
                        | ((oi as u64 & 0xffff) << 40)
                        | (u64::from(prim.material & 0xffff) << 24)
                        | u64::from(draw.prim & 0xff_ffff),
                    depth,
                    object: u32::try_from(oi).unwrap_or(u32::MAX),
                    draw: u32::try_from(di).unwrap_or(u32::MAX),
                    slot: slot_index,
                    variant,
                };
                if material.blend {
                    self.blended.push(cmd);
                } else {
                    self.opaque.push(cmd);
                }
            }
        }
        // Sort instável = in-place, sem alocação.
        self.opaque.sort_unstable_by_key(|c| c.key);
        self.blended.sort_unstable_by(|a, b| a.depth.total_cmp(&b.depth));

        // 6. Uploads do quadro.
        if !self.draw_staging.is_empty() {
            queue.write_buffer(&self.draw_buffer, 0, bytemuck::cast_slice(&self.draw_staging));
        }
        let frame = self.frame_uniform(&view_proj, eye);
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));

        let Some(targets) = &self.targets else {
            return false;
        };

        // 7. Passe PBR (HDR).
        {
            let (color_view, resolve) = match &targets.msaa_view {
                Some(msaa) => (msaa, Some(&targets.hdr_view)),
                None => (&targets.hdr_view, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismview_3d_pbr"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: if resolve.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0), // reverse-Z
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.set_bind_group(1, &self.draw_bind, &[]);
            let mut current_variant = u8::MAX;
            for cmd in self.opaque.iter().chain(self.blended.iter()) {
                let Some(slot) = self.slots.get(cmd.object as usize) else {
                    continue;
                };
                let Some(instance) = slot.instance.as_ref() else {
                    continue;
                };
                let Some(draw) = instance.draws.get(cmd.draw as usize) else {
                    continue;
                };
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let Some(material) = slot.material(instance, prim.material) else {
                    continue;
                };
                if cmd.variant != current_variant {
                    let Some(pipeline) = self.pipelines.get(cmd.variant as usize) else {
                        continue;
                    };
                    pass.set_pipeline(pipeline);
                    current_variant = cmd.variant;
                }
                pass.set_bind_group(2, &material.bind_group, &[]);
                pass.set_vertex_buffer(0, instance.position_slice(draw, prim));
                pass.set_vertex_buffer(1, instance.model.attr.slice(u64::from(prim.first_vertex) * ATTR_STRIDE..));
                pass.set_index_buffer(instance.model.index.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    prim.first_index..prim.first_index + prim.index_count,
                    0,
                    cmd.slot..cmd.slot + 1,
                );
            }
        }

        // 8. Tone mapping → saída RGBA8.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismview_3d_tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.output_view,
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
            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &targets.tonemap_bind, &[]);
            pass.draw(0..3, 0..1);
        }

        self.stats = Scene3dStats {
            drawn: u32::try_from(self.draw_staging.len()).unwrap_or(u32::MAX),
            culled,
            instances,
            vram_bytes: vram,
        };
        self.dirty = false;
        true
    }
}
