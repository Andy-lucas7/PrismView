//! Esculturas procedurais nativas (`SHOWCASE`: `MONOLITH`, `GYROSCOPE`, `KINETIC_SLAB`),
//! espelhando `apps/prismview-studio/src/engine/scene3d/sculptures.ts` para que os templates 3D
//! nativos e objetos `SHOWCASE` apareçam idênticos no desktop aplicado (sem Three.js).
//!
//! Construídas 1× na troca de cena e cacheadas na GPU em `Scene3dShared::models` pelo `AssetHash`.
//! Zero código de SO, zero `unwrap`/`expect`, zero alocação no loop de quadro.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{EulerRot, Quat, Vec3};

use super::lighting::hex_linear;
use crate::engine_core::assets::{
    Aabb, AlphaMode, AssetHash, MaterialData, MeshData, ModelAsset, NodeData, PrimitiveData,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowcaseVariant {
    Monolith,
    Gyroscope,
    KineticSlab,
}

impl ShowcaseVariant {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("GYROSCOPE") => Self::Gyroscope,
            Some(v) if v.eq_ignore_ascii_case("KINETIC_SLAB") => Self::KineticSlab,
            _ => Self::Monolith,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Monolith => "MONOLITH",
            Self::Gyroscope => "GYROSCOPE",
            Self::KineticSlab => "KINETIC_SLAB",
        }
    }

    fn hash(self) -> AssetHash {
        match self {
            Self::Monolith => AssetHash::of_bytes(b"prismview::showcase::MONOLITH::v1"),
            Self::Gyroscope => AssetHash::of_bytes(b"prismview::showcase::GYROSCOPE::v1"),
            Self::KineticSlab => AssetHash::of_bytes(b"prismview::showcase::KINETIC_SLAB::v1"),
        }
    }
}

/// Constrói o `ModelAsset` completo de uma escultura `SHOWCASE` (geometria, materiais PBR e nós).
pub fn build_showcase_asset(variant: ShowcaseVariant) -> ModelAsset {
    let mut b = AssetBuilder::new(variant.hash(), variant.key());
    let obsidian = b.add_material(pbr_mat("ObsidianCore", "#15171e", 0.88, 0.14, [0.0; 3], false));
    let chrome = b.add_material(pbr_mat("TitaniumRing", "#d8dde8", 0.96, 0.16, [0.0; 3], false));
    let matte = b.add_material(pbr_mat("GraphiteRing", "#232630", 0.70, 0.32, [0.0; 3], false));

    match variant {
        ShowcaseVariant::Gyroscope => {
            let core_m = b.add_mesh("AstroCore", sphere_prim(0.62, 32, 20, obsidian));
            let inner_m = b.add_mesh("InnerGimbal", torus_prim(1.06, 0.026, 16, 72, chrome));
            let mid_m = b.add_mesh("MidGimbal", torus_prim(1.38, 0.022, 16, 72, matte));
            let outer_m = b.add_mesh("OuterGimbal", torus_prim(1.70, 0.016, 14, 72, chrome));
            let marker_chrome = b.add_mesh("MarkerChrome", sphere_prim(0.055, 12, 10, chrome));
            let marker_obs = b.add_mesh("MarkerObsidian", sphere_prim(0.055, 12, 10, obsidian));

            b.add_node("AstroCore", None, Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, Some(core_m));
            b.add_node("InnerGimbal", None, Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, Some(inner_m));
            b.add_node(
                "MidGimbal",
                None,
                Vec3::ZERO,
                Quat::from_euler(EulerRot::XYZ, PI * 0.42, 0.3, 0.0),
                Vec3::ONE,
                Some(mid_m),
            );
            let outer_node = b.add_node(
                "OuterGimbal",
                None,
                Vec3::ZERO,
                Quat::from_euler(EulerRot::XYZ, -PI * 0.25, PI * 0.2, 0.0),
                Vec3::ONE,
                Some(outer_m),
            );
            let markers = b.add_node("SolarMarkers", Some(outer_node), Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, None);
            for i in 0..4u32 {
                let a = (i as f32 / 4.0) * TAU;
                let mesh = if i % 2 == 0 { marker_chrome } else { marker_obs };
                b.add_node(
                    &format!("Marker_{}", i + 1),
                    Some(markers),
                    Vec3::new(a.cos() * 1.7, a.sin() * 1.7, 0.0),
                    Quat::IDENTITY,
                    Vec3::ONE,
                    Some(mesh),
                );
            }
        }
        ShowcaseVariant::KineticSlab => {
            let accent = b.add_material(pbr_mat(
                "KineticAccent",
                "#2c3242",
                0.85,
                0.22,
                (hex_linear("#9fb6ff", Vec3::ONE) * 0.35).to_array(),
                false,
            ));
            let screen = b.add_material(pbr_mat(
                "DisplayScreen",
                "#1c202a",
                0.12,
                0.25,
                (hex_linear("#181c26", Vec3::ZERO) * 0.40).to_array(),
                true,
            ));

            let body_m = b.add_mesh("SlabBody", box_prim(1.86, 1.86, 0.14, obsidian));
            let bezel_m = b.add_mesh("AccentBezel", box_prim(1.94, 1.94, 0.08, accent));
            let screen_m = b.add_mesh("DisplayScreen", plane_prim(1.66, 1.66, screen));
            let halo_m = b.add_mesh("PulseHalo", torus_prim(1.68, 0.02, 16, 72, chrome));
            let back_m = b.add_mesh("OuterHalo", torus_prim(1.92, 0.012, 14, 72, matte));

            let frame = b.add_node("KineticFrame", None, Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, None);
            b.add_node("SlabBody", Some(frame), Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, Some(body_m));
            b.add_node(
                "AccentBezel",
                Some(frame),
                Vec3::new(0.0, 0.0, -0.02),
                Quat::IDENTITY,
                Vec3::ONE,
                Some(bezel_m),
            );
            b.add_node(
                "DisplayScreen",
                Some(frame),
                Vec3::new(0.0, 0.0, 0.076),
                Quat::IDENTITY,
                Vec3::ONE,
                Some(screen_m),
            );
            b.add_node("PulseHalo", None, Vec3::new(0.0, 0.0, -0.22), Quat::IDENTITY, Vec3::ONE, Some(halo_m));
            b.add_node("OuterHalo", None, Vec3::new(0.0, 0.0, -0.35), Quat::IDENTITY, Vec3::ONE, Some(back_m));
        }
        ShowcaseVariant::Monolith => {
            let core_m = b.add_mesh(
                "PrismCore",
                octahedron_prim(0.86, Vec3::new(0.86, 1.24, 0.86), obsidian),
            );
            let ring_a_m = b.add_mesh("OrbitalRingA", torus_prim(1.38, 0.022, 16, 72, chrome));
            let ring_b_m = b.add_mesh("OrbitalRingB", torus_prim(1.66, 0.014, 14, 72, matte));
            let sat_chrome = b.add_mesh("SatChrome", box_prim(0.11, 0.11, 0.11, chrome));
            let sat_obs = b.add_mesh("SatObsidian", box_prim(0.11, 0.11, 0.11, obsidian));

            b.add_node("PrismCore", None, Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, Some(core_m));
            b.add_node(
                "OrbitalRingA",
                None,
                Vec3::ZERO,
                Quat::from_euler(EulerRot::XYZ, PI * 0.36, PI * 0.12, 0.0),
                Vec3::ONE,
                Some(ring_a_m),
            );
            b.add_node(
                "OrbitalRingB",
                None,
                Vec3::ZERO,
                Quat::from_euler(EulerRot::XYZ, -PI * 0.28, PI * 0.42, 0.2),
                Vec3::ONE,
                Some(ring_b_m),
            );
            let sats = b.add_node("Satellites", None, Vec3::ZERO, Quat::IDENTITY, Vec3::ONE, None);
            for i in 0..6u32 {
                let angle = (i as f32 / 6.0) * TAU;
                let radius = 1.38 + ((i % 2) as f32) * 0.28;
                let mesh = if i % 2 == 0 { sat_chrome } else { sat_obs };
                b.add_node(
                    &format!("Node_{}", i + 1),
                    Some(sats),
                    Vec3::new(angle.cos() * radius, (angle * 2.0).sin() * 0.32, angle.sin() * radius),
                    Quat::from_euler(EulerRot::XYZ, angle, angle * 0.7, 0.4),
                    Vec3::ONE,
                    Some(mesh),
                );
            }
        }
    }

    b.finish()
}

fn pbr_mat(
    name: &str,
    hex: &str,
    metallic: f32,
    roughness: f32,
    emissive: [f32; 3],
    double_sided: bool,
) -> MaterialData {
    let rgb = hex_linear(hex, Vec3::splat(0.5));
    MaterialData {
        name: Some(name.to_owned()),
        base_color: [rgb.x, rgb.y, rgb.z, 1.0],
        metallic,
        roughness,
        emissive,
        alpha_mode: AlphaMode::Opaque,
        double_sided,
        ..MaterialData::default()
    }
}

struct AssetBuilder {
    hash: AssetHash,
    name: String,
    nodes: Vec<NodeData>,
    roots: Vec<u32>,
    meshes: Vec<MeshData>,
    materials: Vec<MaterialData>,
}

impl AssetBuilder {
    fn new(hash: AssetHash, name: &str) -> Self {
        Self {
            hash,
            name: format!("PrismView_Showcase_{name}"),
            nodes: Vec::new(),
            roots: Vec::new(),
            meshes: Vec::new(),
            materials: Vec::new(),
        }
    }

    fn add_material(&mut self, mat: MaterialData) -> u32 {
        let idx = u32::try_from(self.materials.len()).unwrap_or(0);
        self.materials.push(mat);
        idx
    }

    fn add_mesh(&mut self, name: &str, prim: PrimitiveData) -> u32 {
        let idx = u32::try_from(self.meshes.len()).unwrap_or(0);
        self.meshes.push(MeshData {
            name: Some(name.to_owned()),
            primitives: vec![prim],
            weights: Vec::new(),
            target_names: Vec::new(),
        });
        idx
    }

    fn add_node(
        &mut self,
        name: &str,
        parent: Option<u32>,
        translation: Vec3,
        rotation: Quat,
        scale: Vec3,
        mesh: Option<u32>,
    ) -> u32 {
        let idx = u32::try_from(self.nodes.len()).unwrap_or(0);
        self.nodes.push(NodeData {
            name: Some(name.to_owned()),
            parent,
            children: Vec::new(),
            translation,
            rotation: rotation.normalize(),
            scale,
            mesh,
            skin: None,
            weights: Vec::new(),
        });
        match parent {
            Some(p) => {
                if let Some(pnode) = self.nodes.get_mut(p as usize) {
                    pnode.children.push(idx);
                }
            }
            None => self.roots.push(idx),
        }
        idx
    }

    fn finish(self) -> ModelAsset {
        let mut asset = ModelAsset {
            hash: self.hash,
            name: Some(self.name),
            nodes: self.nodes,
            roots: self.roots,
            meshes: self.meshes,
            materials: self.materials,
            skins: Vec::new(),
            animations: Vec::new(),
            textures: Vec::new(),
            bounds: Aabb::EMPTY,
            warnings: Vec::new(),
        };
        let world = asset.world_matrices();
        let mut bounds = Aabb::EMPTY;
        for (i, node) in asset.nodes.iter().enumerate() {
            let Some(mesh) = node.mesh.and_then(|m| asset.meshes.get(m as usize)) else {
                continue;
            };
            for prim in &mesh.primitives {
                if let (Some(pb), Some(m)) = (prim.bounds.as_ref(), world.get(i)) {
                    bounds.union(&pb.transformed(m));
                }
            }
        }
        asset.bounds = bounds;
        asset
    }
}

// ───────────────────────── Primitivas geométricas (CCW) ─────────────────────────

fn finalize_prim(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uv0: Vec<[f32; 2]>,
    indices: Vec<u32>,
    material: u32,
) -> PrimitiveData {
    let mut bounds = Aabb::EMPTY;
    for p in &positions {
        bounds.extend(Vec3::from(*p));
    }
    PrimitiveData {
        positions,
        normals,
        uv0,
        indices,
        material: Some(material),
        bounds: Some(bounds),
        ..PrimitiveData::default()
    }
}

fn octahedron_prim(radius: f32, scale: Vec3, material: u32) -> PrimitiveData {
    let verts = [
        Vec3::new(0.0, radius, 0.0) * scale,
        Vec3::new(0.0, -radius, 0.0) * scale,
        Vec3::new(radius, 0.0, 0.0) * scale,
        Vec3::new(-radius, 0.0, 0.0) * scale,
        Vec3::new(0.0, 0.0, radius) * scale,
        Vec3::new(0.0, 0.0, -radius) * scale,
    ];
    let faces = [
        [0, 4, 2],
        [0, 2, 5],
        [0, 5, 3],
        [0, 3, 4],
        [1, 2, 4],
        [1, 5, 2],
        [1, 3, 5],
        [1, 4, 3],
    ];
    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut uv0 = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(24);
    for [a, b, c] in faces {
        let (pa, pb, pc) = (verts[a], verts[b], verts[c]);
        let n = (pb - pa).cross(pc - pa).normalize_or_zero().to_array();
        let base = u32::try_from(positions.len()).unwrap_or(0);
        positions.extend([pa.to_array(), pb.to_array(), pc.to_array()]);
        normals.extend([n, n, n]);
        uv0.extend([[0.5, 0.0], [0.0, 1.0], [1.0, 1.0]]);
        indices.extend([base, base + 1, base + 2]);
    }
    finalize_prim(positions, normals, uv0, indices, material)
}

fn box_prim(w: f32, h: f32, d: f32, material: u32) -> PrimitiveData {
    let (hx, hy, hz) = (w * 0.5, h * 0.5, d * 0.5);
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[-hx, -hy, hz], [hx, -hy, hz], [hx, hy, hz], [-hx, hy, hz]]),
        ([0.0, 0.0, -1.0], [[hx, -hy, -hz], [-hx, -hy, -hz], [-hx, hy, -hz], [hx, hy, -hz]]),
        ([1.0, 0.0, 0.0], [[hx, -hy, hz], [hx, -hy, -hz], [hx, hy, -hz], [hx, hy, hz]]),
        ([-1.0, 0.0, 0.0], [[-hx, -hy, -hz], [-hx, -hy, hz], [-hx, hy, hz], [-hx, hy, -hz]]),
        ([0.0, 1.0, 0.0], [[-hx, hy, hz], [hx, hy, hz], [hx, hy, -hz], [-hx, hy, -hz]]),
        ([0.0, -1.0, 0.0], [[-hx, -hy, -hz], [hx, -hy, -hz], [hx, -hy, hz], [-hx, -hy, hz]]),
    ];
    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut uv0 = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (n, quad) in faces {
        let base = u32::try_from(positions.len()).unwrap_or(0);
        positions.extend(quad);
        normals.extend([n; 4]);
        uv0.extend([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    finalize_prim(positions, normals, uv0, indices, material)
}

fn plane_prim(w: f32, h: f32, material: u32) -> PrimitiveData {
    let (hx, hy) = (w * 0.5, h * 0.5);
    finalize_prim(
        vec![[-hx, -hy, 0.0], [hx, -hy, 0.0], [hx, hy, 0.0], [-hx, hy, 0.0]],
        vec![[0.0, 0.0, 1.0]; 4],
        vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        vec![0, 1, 2, 0, 2, 3],
        material,
    )
}

fn sphere_prim(radius: f32, segs_u: u32, segs_v: u32, material: u32) -> PrimitiveData {
    let su = segs_u.max(8);
    let sv = segs_v.max(6);
    let mut positions = Vec::with_capacity(((su + 1) * (sv + 1)) as usize);
    let mut normals = Vec::with_capacity(((su + 1) * (sv + 1)) as usize);
    let mut uv0 = Vec::with_capacity(((su + 1) * (sv + 1)) as usize);
    let mut indices = Vec::with_capacity((su * sv * 6) as usize);

    for iv in 0..=sv {
        let v = iv as f32 / sv as f32;
        let phi = v * PI;
        let (sp, cp) = phi.sin_cos();
        for iu in 0..=su {
            let u = iu as f32 / su as f32;
            let theta = u * TAU;
            let (st, ct) = theta.sin_cos();
            let n = Vec3::new(sp * ct, cp, sp * st);
            positions.push((n * radius).to_array());
            normals.push(n.to_array());
            uv0.push([u, v]);
        }
    }
    let row = su + 1;
    for iv in 0..sv {
        for iu in 0..su {
            let a = iv * row + iu;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            indices.extend([a, c, b, b, c, d]);
        }
    }
    finalize_prim(positions, normals, uv0, indices, material)
}

fn torus_prim(major: f32, minor: f32, radial: u32, tubular: u32, material: u32) -> PrimitiveData {
    let r_segs = radial.max(6);
    let t_segs = tubular.max(12);
    let mut positions = Vec::with_capacity(((r_segs + 1) * (t_segs + 1)) as usize);
    let mut normals = Vec::with_capacity(((r_segs + 1) * (t_segs + 1)) as usize);
    let mut uv0 = Vec::with_capacity(((r_segs + 1) * (t_segs + 1)) as usize);
    let mut indices = Vec::with_capacity((r_segs * t_segs * 6) as usize);

    for ir in 0..=r_segs {
        let v = ir as f32 / r_segs as f32;
        let phi = v * TAU;
        let (sp, cp) = phi.sin_cos();
        for it in 0..=t_segs {
            let u = it as f32 / t_segs as f32;
            let theta = u * TAU;
            let (st, ct) = theta.sin_cos();
            let ring = major + minor * cp;
            let pos = Vec3::new(ring * ct, ring * st, minor * sp);
            let center = Vec3::new(major * ct, major * st, 0.0);
            let n = (pos - center).normalize_or_zero();
            positions.push(pos.to_array());
            normals.push(n.to_array());
            uv0.push([u, v]);
        }
    }
    let stride = t_segs + 1;
    for ir in 0..r_segs {
        for it in 0..t_segs {
            let a = ir * stride + it;
            let b = a + stride;
            let c = b + 1;
            let d = a + 1;
            // Winding CCW para fora do tubo (garante culling correto)
            let _ = FRAC_PI_2;
            indices.extend([a, d, b, b, d, c]);
        }
    }
    finalize_prim(positions, normals, uv0, indices, material)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_all_three_showcase_variants_with_valid_geometry() {
        for variant in [
            ShowcaseVariant::Monolith,
            ShowcaseVariant::Gyroscope,
            ShowcaseVariant::KineticSlab,
        ] {
            let asset = build_showcase_asset(variant);
            assert!(!asset.meshes.is_empty(), "{variant:?} sem malhas");
            assert!(!asset.nodes.is_empty(), "{variant:?} sem nós");
            assert!(!asset.bounds.is_empty(), "{variant:?} bounds vazios");
            for m in &asset.meshes {
                for p in &m.primitives {
                    assert!(!p.positions.is_empty());
                    assert_eq!(p.positions.len(), p.normals.len());
                    assert_eq!(p.indices.len() % 3, 0);
                    let max_v = u32::try_from(p.positions.len()).unwrap_or(0);
                    assert!(p.indices.iter().all(|&i| i < max_v));
                }
            }
        }
    }
}
