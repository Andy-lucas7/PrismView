//! Contrato JSON da cena 3D (`Scene3DConfig` / `Scene3DObject` de
//! `apps/prismview-studio/src/types/prism.ts`, schema v3), em camelCase como o Estúdio salva.
//!
//! `parentId` ausente = raiz (ver `SceneGraph`). Schema ≤2 (modelo único legado) é migrado em
//! `effective_objects` exatamente como `normalizeScene3DConfig` do TS (`obj-primary`, giro 0,22→0).
//!
//! Todos os campos têm `default` (projetos antigos/editados à mão não derrubam o parse). `bindings`
//! e `dynamicTextures` ficam como JSON bruto até a Fase 4 (SignalBus/BindingSolver).

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Scene3dObjectKind {
    #[default]
    Glb,
    Showcase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Scene3dEnvPreset {
    #[default]
    StudioDark,
    CityNight,
    Sunset,
    Daylight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Scene3dBackgroundMode {
    /// Fora dos modelos: alpha 0 (saída pré-multiplicada).
    #[default]
    Transparent,
    /// `backgroundColor` opaco por baixo.
    Solid,
    /// Cor de fundo do preset de ambiente (ou céu do ciclo dia/noite), como `applySceneBackground`.
    Environment,
}

/// Giro padrão antigo (schema ≤2) que a migração v3 zera (`LEGACY_DEFAULT_AUTO_ROTATE` no TS).
pub const LEGACY_DEFAULT_AUTO_ROTATE: f32 = 0.22;
/// Id do objeto sintético criado a partir do modelo único legado (igual ao TS).
pub const LEGACY_PRIMARY_ID: &str = "obj-primary";
/// Primeiro schema com `objects[]`/`backgroundMode`.
pub const SCENE3D_SCHEMA_VERSION: u32 = 3;

/// v2→v3: 0,22 rad/s vira 0; outro valor do usuário é mantido.
pub fn migrate_legacy_spin(speed: f32) -> f32 {
    if !speed.is_finite() || (speed - LEGACY_DEFAULT_AUTO_ROTATE).abs() < 1e-6 {
        0.0
    } else {
        speed
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scene3dPlayback {
    pub clip_name: String,
    pub speed: f32,
    #[serde(rename = "loop")]
    pub looping: bool,
}

impl Default for Scene3dPlayback {
    fn default() -> Self {
        Self {
            clip_name: String::new(),
            speed: 1.0,
            looping: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scene3dCamera {
    pub fov: f32,
    pub distance: f32,
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    pub target_y: f32,
    pub target: Option<[f32; 3]>,
    pub orbit_parallax_x: f32,
    pub orbit_parallax_y: f32,
    pub damping: f32,
}

impl Default for Scene3dCamera {
    fn default() -> Self {
        Self {
            // Mesmo enquadramento padrão do Estúdio (`defaultScene3D`, modelo externo).
            fov: 42.0,
            distance: 4.8,
            azimuth_deg: 0.0,
            elevation_deg: 8.0,
            target_y: 0.0,
            target: None,
            orbit_parallax_x: 12.0,
            orbit_parallax_y: 7.0,
            damping: 0.08,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scene3dObject {
    pub id: String,
    pub name: String,
    pub kind: Scene3dObjectKind,
    pub model_src: String,
    pub showcase_variant: Option<String>,
    /// Hierarquia: `id` de outro objeto da mesma cena (ausente/`null` = raiz). Posição, rotação e
    /// escala acima são **locais** ao pai.
    pub parent_id: Option<String>,
    pub position: [f32; 3],
    /// Euler em graus `[rx, ry, rz]`. O runtime do Estúdio (`Scene3DLayer`) aplica ordem `YXZ`
    /// (matriz = Ry·Rx·Rz) somando o giro de `autoRotateSpeed` em Y.
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub visible: bool,
    pub auto_rotate_speed: f32,
    pub playback: Scene3dPlayback,
    pub dynamic_textures: Vec<Value>,
}

impl Default for Scene3dObject {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: Scene3dObjectKind::Glb,
            model_src: String::new(),
            showcase_variant: None,
            parent_id: None,
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            visible: true,
            auto_rotate_speed: 0.0,
            playback: Scene3dPlayback::default(),
            dynamic_textures: Vec::new(),
        }
    }
}

impl Scene3dObject {
    /// Matriz local `T · R(yxz) · S`, igual ao pivô do `Scene3DLayer` sem giro.
    pub fn local_matrix(&self) -> Mat4 {
        self.local_matrix_with_spin(0.0)
    }

    /// Matriz local com o giro acumulado de `autoRotateSpeed` (rad) somado à rotação Y. Escala
    /// mínima 0,05 por eixo, como o Estúdio.
    pub fn local_matrix_with_spin(&self, spin_rad: f32) -> Mat4 {
        let [rx, ry, rz] = self.rotation.map(|d| if d.is_finite() { d.to_radians() } else { 0.0 });
        let spin = if spin_rad.is_finite() { spin_rad } else { 0.0 };
        let rotation = Quat::from_euler(glam::EulerRot::YXZ, ry + spin, rx, rz);
        let finite = |v: [f32; 3], default: f32| {
            Vec3::new(
                if v[0].is_finite() { v[0] } else { default },
                if v[1].is_finite() { v[1] } else { default },
                if v[2].is_finite() { v[2] } else { default },
            )
        };
        Mat4::from_scale_rotation_translation(
            finite(self.scale, 1.0).max(Vec3::splat(0.05)),
            rotation,
            finite(self.position, 0.0),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scene3dConfig {
    pub schema_version: u32,
    /// Modelo único legado (schema ≤2); `objects` tem prioridade quando não vazio.
    pub model_src: String,
    /// Legado (schema ≤2): variante da escultura quando não há `modelSrc`.
    pub showcase_variant: Option<String>,
    pub objects: Vec<Scene3dObject>,
    pub background_mode: Scene3dBackgroundMode,
    pub background_color: Option<String>,
    pub env_preset: Scene3dEnvPreset,
    /// HDRI equirretangular (`.hdr`) opcional; ausente = ambiente procedural de `env_preset`.
    /// (Extensão do contrato nativo: o Estúdio ainda não grava este campo.)
    pub env_map_src: Option<String>,
    pub env_intensity: f32,
    pub exposure: f32,
    pub render_scale: f32,
    /// Teto de FPS (5–200, passo 5); ignorado se `fps_mode == "MONITOR"`.
    pub max_fps: u32,
    pub fps_mode: String,
    pub auto_rotate_speed: f32,
    pub model_scale: f32,
    pub model_offset_y: f32,
    pub auto_day_night: bool,
    /// Hora fixa (0..24) do ciclo dia/noite (`dayNightHourOverride`); `None` = relógio local.
    pub day_night_hour_override: Option<f32>,
    pub camera: Scene3dCamera,
    pub playback: Scene3dPlayback,
    /// Bindings reativos e texturas dinâmicas: tratados na Fase 4.
    pub bindings: Vec<Value>,
    pub dynamic_textures: Vec<Value>,
}

impl Default for Scene3dConfig {
    fn default() -> Self {
        Self {
            schema_version: 2,
            model_src: String::new(),
            showcase_variant: None,
            objects: Vec::new(),
            background_mode: Scene3dBackgroundMode::Transparent,
            background_color: None,
            env_preset: Scene3dEnvPreset::StudioDark,
            env_map_src: None,
            env_intensity: 1.2,
            exposure: 1.0,
            render_scale: 1.0,
            max_fps: 60,
            fps_mode: "CAP".to_owned(),
            auto_rotate_speed: 0.0,
            model_scale: 1.0,
            model_offset_y: 0.0,
            auto_day_night: false,
            day_night_hour_override: None,
            camera: Scene3dCamera::default(),
            playback: Scene3dPlayback::default(),
            bindings: Vec::new(),
            dynamic_textures: Vec::new(),
        }
    }
}

impl Scene3dConfig {
    /// Objetos efetivos (espelho de `normalizeScene3DObjects` + migração v3 do TS): `objects`
    /// (schema ≤2: giro 0,22→0; id vazio → `obj-{i}`), ou o objeto sintético `obj-primary` do
    /// modelo único legado (escala uniforme `modelScale`, `position[1] = modelOffsetY`, giro e
    /// playback globais; sem `modelSrc` = SHOWCASE).
    pub fn effective_objects(&self) -> Vec<Scene3dObject> {
        let legacy = self.schema_version < SCENE3D_SCHEMA_VERSION;
        if !self.objects.is_empty() {
            let mut objects = self.objects.clone();
            for (i, obj) in objects.iter_mut().enumerate() {
                if obj.id.is_empty() {
                    obj.id = format!("obj-{i}");
                }
                if legacy {
                    obj.auto_rotate_speed = migrate_legacy_spin(obj.auto_rotate_speed);
                }
            }
            return objects;
        }
        let src = self.model_src.trim();
        let scale = if self.model_scale.is_finite() && self.model_scale > 0.0 {
            self.model_scale
        } else {
            1.0
        };
        let has_model = !src.is_empty();
        vec![Scene3dObject {
            id: LEGACY_PRIMARY_ID.to_owned(),
            name: "Primary".to_owned(),
            kind: if has_model {
                Scene3dObjectKind::Glb
            } else {
                Scene3dObjectKind::Showcase
            },
            model_src: src.to_owned(),
            showcase_variant: if has_model {
                None
            } else {
                Some(self.showcase_variant.clone().unwrap_or_else(|| "MONOLITH".to_owned()))
            },
            position: [
                0.0,
                if self.model_offset_y.is_finite() { self.model_offset_y } else { 0.0 },
                0.0,
            ],
            scale: [scale; 3],
            auto_rotate_speed: migrate_legacy_spin(self.auto_rotate_speed),
            playback: self.playback.clone(),
            dynamic_textures: self.dynamic_textures.clone(),
            ..Scene3dObject::default()
        }]
    }

    /// Hora do ciclo dia/noite: override finito (0..24) ou `None` (usar o relógio local).
    pub fn hour_override(&self) -> Option<f32> {
        self.day_night_hour_override
            .filter(|h| h.is_finite())
            .map(|h| h.rem_euclid(24.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_studio_json_without_parent_id() {
        let json = r#"{
            "schemaVersion": 2, "modelSrc": "a.glb", "envPreset": "CITY_NIGHT", "maxFps": 30,
            "objects": [{ "id": "o1", "name": "A", "kind": "GLB", "modelSrc": "a.glb",
                          "position": [1,2,3], "rotation": [0,90,0], "scale": [1,1,1],
                          "visible": true, "playback": { "clipName": "", "speed": 1, "loop": true } }]
        }"#;
        let cfg: Scene3dConfig = serde_json::from_str(json).expect("json válido");
        assert_eq!(cfg.env_preset, Scene3dEnvPreset::CityNight);
        assert_eq!(cfg.objects[0].parent_id, None);
        assert_eq!(cfg.max_fps, 30);
    }

    #[test]
    fn parses_v3_background_and_hour_override() {
        let json = r##"{ "schemaVersion": 3, "backgroundMode": "ENVIRONMENT", "backgroundColor": "#101010",
                        "dayNightHourOverride": 25.5,
                        "objects": [{ "id": "a", "autoRotateSpeed": 0.22 }, { "id": "b", "parentId": "a" }] }"##;
        let cfg: Scene3dConfig = serde_json::from_str(json).expect("json válido");
        assert_eq!(cfg.background_mode, Scene3dBackgroundMode::Environment);
        assert_eq!(cfg.hour_override(), Some(1.5));
        let objects = cfg.effective_objects();
        // v3: 0,22 escolhido pelo usuário é mantido.
        assert!((objects[0].auto_rotate_speed - 0.22).abs() < 1e-6);
        assert_eq!(objects[1].parent_id.as_deref(), Some("a"));
    }

    #[test]
    fn legacy_single_model_becomes_obj_primary() {
        let json = r#"{ "schemaVersion": 2, "modelSrc": " a.glb ", "modelScale": 2, "modelOffsetY": -0.5,
                        "autoRotateSpeed": 0.22, "playback": { "clipName": "Idle", "speed": 2, "loop": false } }"#;
        let cfg: Scene3dConfig = serde_json::from_str(json).expect("json válido");
        let objects = cfg.effective_objects();
        assert_eq!(objects.len(), 1);
        let o = &objects[0];
        assert_eq!(o.id, LEGACY_PRIMARY_ID);
        assert_eq!(o.model_src, "a.glb");
        assert_eq!(o.scale, [2.0; 3]);
        assert_eq!(o.position, [0.0, -0.5, 0.0]);
        assert_eq!(o.auto_rotate_speed, 0.0);
        assert_eq!(o.playback.clip_name, "Idle");
        assert!(!o.playback.looping);

        let empty: Scene3dConfig = serde_json::from_str("{}").expect("json válido");
        assert_eq!(empty.effective_objects()[0].kind, Scene3dObjectKind::Showcase);
    }

    #[test]
    fn local_matrix_matches_threejs_xyz_order() {
        let obj = Scene3dObject {
            rotation: [0.0, 90.0, 0.0],
            position: [1.0, 0.0, 0.0],
            ..Scene3dObject::default()
        };
        let p = obj.local_matrix().transform_point3(Vec3::new(0.0, 0.0, 1.0));
        // Ry(90°) leva +Z para +X; depois translada +1 em X.
        assert!((p - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5);
    }
}
