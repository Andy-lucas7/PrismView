//! Hover/clique: raio da câmera × AABB por draw.

#[allow(unused_imports)]
use super::*;

impl Scene3dRenderer {
    /// Clique normalizado (0..1 sobre a camada) para bindings `CLICK_MESH`. `true` se disparou.
    pub fn click(&mut self, pointer: [f32; 2]) -> bool {
        if !self.solver.wants_click() {
            return false;
        }
        let Some(hit) = self.pick(pointer) else {
            return false;
        };
        let mut access = SlotAccess {
            slots: &mut self.slots,
            objects: &self.objects,
        };
        let fired = self.solver.click(&mut access, hit);
        if fired {
            self.dirty = true;
        }
        fired
    }

    /// Objeto/nó sob o cursor normalizado (raio da câmera × AABB de mundo de cada draw). Usa as
    /// matrizes do último `render`. Sem alocação.
    pub fn pick(&self, pointer: [f32; 2]) -> Option<PickHit> {
        if !pointer[0].is_finite() || !pointer[1].is_finite() {
            return None;
        }
        let (w, h) = self.viewport;
        if w == 0 || h == 0 {
            return None;
        }
        let aspect = w as f32 / h as f32;
        let (eye, target, fov) = self.camera_view();
        let fwd = (target - eye).normalize_or_zero();
        let right = fwd.cross(camera_up_ref(fwd)).normalize_or_zero();
        if fwd == Vec3::ZERO || right == Vec3::ZERO {
            return None;
        }
        let up = right.cross(fwd);
        let t = (fov.to_radians() * 0.5).tan();
        let nx = pointer[0] * 2.0 - 1.0;
        let ny = 1.0 - pointer[1] * 2.0;
        let dir = (fwd + right * (nx * t * aspect) + up * (ny * t)).normalize_or_zero();
        let safe = |v: f32| if v.abs() < 1e-9 { 1e-9_f32.copysign(v) } else { v };
        let inv = Vec3::new(1.0 / safe(dir.x), 1.0 / safe(dir.y), 1.0 / safe(dir.z));

        let mut best: Option<(f32, PickHit)> = None;
        for (oi, slot) in self.slots.iter().enumerate() {
            let Some(instance) = &slot.instance else {
                continue;
            };
            if !self.visible.get(oi).copied().unwrap_or(false) {
                continue;
            }
            let object = self.object_world.get(oi).copied().unwrap_or(Mat4::IDENTITY);
            let base = object * instance.root_matrix() * instance.normalize;
            for draw in &instance.draws {
                let Some(prim) = instance.primitive(draw) else {
                    continue;
                };
                let (model, bounds) = if draw.skinned {
                    (base, instance.skinned_bounds)
                } else {
                    (base * instance.node_world(draw.node), prim.bounds)
                };
                let Some(dist) = ray_aabb(eye, inv, &bounds.transformed(&model)) else {
                    continue;
                };
                if best.map_or(true, |(d, _)| dist < d) {
                    best = Some((dist, PickHit { object: oi, node: draw.node }));
                }
            }
        }
        best.map(|(_, hit)| hit)
    }

    /// Olho, alvo e FOV da câmera do wallpaper (órbita aplicada).
    pub(super) fn camera_view(&self) -> (Vec3, Vec3, f32) {
        let cam = &self.config.camera;
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        let target = match cam.target {
            Some(t) => Vec3::new(finite(t[0], 0.0), finite(t[1], finite(cam.target_y, 0.0)), finite(t[2], 0.0)),
            None => Vec3::new(0.0, finite(cam.target_y, 0.0), 0.0),
        };
        let az = (finite(cam.azimuth_deg, 0.0) + self.orbit[0]).to_radians();
        let el = (finite(cam.elevation_deg, 0.0) + self.orbit[1]).clamp(-80.0, 80.0).to_radians();
        let dist = if cam.distance.is_finite() && cam.distance != 0.0 { cam.distance } else { 4.8 }.max(0.3);
        let fov = if cam.fov.is_finite() && cam.fov != 0.0 { cam.fov } else { 42.0 }.clamp(20.0, 85.0);
        let eye = target + Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos()) * dist;
        (eye, target, fov)
    }
}
