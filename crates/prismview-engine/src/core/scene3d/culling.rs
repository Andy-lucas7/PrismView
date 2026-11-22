//! Frustum culling por AABB de mundo. Projeção reverse-Z infinita: 4 planos laterais + plano
//! próximo (o distante está no infinito e não corta nada).

use glam::{Mat4, Vec3, Vec4};

use crate::engine_core::assets::Aabb;

#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    /// `(normal, d)`: ponto dentro ⇔ `dot(normal, p) + d >= 0`.
    planes: [Vec4; 5],
}

impl Frustum {
    /// Extrai os planos de `view_proj` (clip z ∈ [0, w], reverse-Z: z = w no plano próximo).
    pub fn from_view_proj(m: &Mat4) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        let raw = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2];
        let planes = raw.map(|p| {
            let len = p.truncate().length();
            if len > 1e-12 && len.is_finite() {
                p / len
            } else {
                // Plano degenerado: nunca rejeita.
                Vec4::new(0.0, 0.0, 0.0, 1.0)
            }
        });
        Self { planes }
    }

    /// `true` se a caixa toca o frustum (teste conservador pelo vértice positivo).
    pub fn intersects_aabb(&self, b: &Aabb) -> bool {
        if b.is_empty() {
            return false;
        }
        if !(b.min.is_finite() && b.max.is_finite()) {
            return true; // geometria degenerada: deixa a GPU decidir
        }
        for p in &self.planes {
            let n = p.truncate();
            let v = Vec3::select(n.cmpge(Vec3::ZERO), b.max, b.min);
            if n.dot(v) + p.w < 0.0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_behind_and_sides() {
        let proj = Mat4::perspective_infinite_reverse_rh(60f32.to_radians(), 1.0, 0.1);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
        let f = Frustum::from_view_proj(&(proj * view));
        let unit = |c: Vec3| Aabb {
            min: c - Vec3::splat(0.5),
            max: c + Vec3::splat(0.5),
        };
        assert!(f.intersects_aabb(&unit(Vec3::ZERO)));
        assert!(f.intersects_aabb(&unit(Vec3::new(0.0, 0.0, -1000.0))));
        assert!(!f.intersects_aabb(&unit(Vec3::new(0.0, 0.0, 10.0))));
        assert!(!f.intersects_aabb(&unit(Vec3::new(50.0, 0.0, 0.0))));
    }
}
