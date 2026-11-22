//! Iluminação da cena 3D espelhando `engine/scene3d/environment.ts` do Estúdio:
//! - preset → luz ambiente + key (direcional) + rim (pontual, alcance 24, decaimento 2);
//! - ambiente procedural = cor de fundo + "cards" emissivos (o PMREM do Estúdio renderiza esses
//!   planos; aqui o compute shader faz interseção raio×retângulo exata, ver `environment.rs`);
//! - ciclo dia/noite por hora local (mesmas chaves e `smoothstep`).
//!
//! Cores hex são sRGB e viram lineares (como `THREE.Color` com color management).

use glam::{EulerRot, Mat3, Vec3};

use super::config::Scene3dEnvPreset;

pub const MAX_DIR_LIGHTS: usize = 4;
pub const MAX_POINT_LIGHTS: usize = 4;
pub const MAX_ENV_CARDS: usize = 4;

/// sRGB (0..1) → linear.
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear → sRGB (0..1).
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// `#rgb`, `#rrggbb` ou `#rrggbbaa` → RGBA sRGB 0..1.
pub fn parse_hex_srgb(raw: &str) -> Option<[f32; 4]> {
    let hex = raw.trim().strip_prefix('#')?;
    let nib = |i: usize| -> Option<u8> { u8::from_str_radix(hex.get(i..i + 1)?, 16).ok() };
    let byte = |i: usize| -> Option<u8> { u8::from_str_radix(hex.get(i..i + 2)?, 16).ok() };
    let (r, g, b, a) = match hex.len() {
        3 => (nib(0)? * 17, nib(1)? * 17, nib(2)? * 17, 255),
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some([r, g, b, a].map(|v| f32::from(v) / 255.0))
}

/// Hex sRGB → RGB linear (inválido = `fallback`).
pub fn hex_linear(raw: &str, fallback: Vec3) -> Vec3 {
    parse_hex_srgb(raw).map_or(fallback, |c| {
        Vec3::new(srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]))
    })
}

// ───────────────────────── Presets ─────────────────────────

struct CardSpec {
    color: &'static str,
    intensity: f32,
    pos: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 2],
}

struct PresetSpec {
    bg: &'static str,
    ambient: &'static str,
    ambient_intensity: f32,
    key: &'static str,
    key_intensity: f32,
    key_pos: [f32; 3],
    rim: &'static str,
    rim_intensity: f32,
    rim_pos: [f32; 3],
    cards: [CardSpec; 4],
}

const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;

const fn card(color: &'static str, intensity: f32, pos: [f32; 3], rot: [f32; 3], scale: [f32; 2]) -> CardSpec {
    CardSpec {
        color,
        intensity,
        pos,
        rot,
        scale,
    }
}

static STUDIO_DARK: PresetSpec = PresetSpec {
    bg: "#07080b",
    ambient: "#b8c0d0",
    ambient_intensity: 0.4,
    key: "#ffffff",
    key_intensity: 2.2,
    key_pos: [4.5, 5.5, 4.5],
    rim: "#c8d4ea",
    rim_intensity: 1.4,
    rim_pos: [-4.5, 2.2, -3.5],
    cards: [
        card("#ffffff", 4.0, [0.0, 6.5, 1.5], [HALF_PI, 0.0, 0.0], [9.0, 5.0]),
        card("#d6e0f0", 2.4, [-7.0, 2.5, 2.0], [0.0, HALF_PI, 0.0], [6.0, 5.0]),
        card("#f4ede4", 2.0, [7.0, 1.5, -2.0], [0.0, -HALF_PI, 0.0], [5.0, 5.0]),
        card("#1d2028", 0.8, [0.0, -5.0, 0.0], [-HALF_PI, 0.0, 0.0], [14.0, 14.0]),
    ],
};

static CITY_NIGHT: PresetSpec = PresetSpec {
    bg: "#05070e",
    ambient: "#7b92bc",
    ambient_intensity: 0.35,
    key: "#b9d7ff",
    key_intensity: 2.1,
    key_pos: [3.8, 5.0, 4.2],
    rim: "#9aa8ff",
    rim_intensity: 2.0,
    rim_pos: [-5.0, 2.0, -3.2],
    cards: [
        card("#9ac7ff", 4.2, [-6.5, 3.0, 2.0], [0.0, HALF_PI, 0.0], [6.0, 6.0]),
        card("#c9d4ff", 3.0, [6.5, 2.0, -2.0], [0.0, -HALF_PI, 0.0], [5.0, 6.0]),
        card("#4d6796", 1.5, [0.0, 7.0, 0.0], [HALF_PI, 0.0, 0.0], [10.0, 10.0]),
        card("#131926", 0.7, [0.0, -5.0, 0.0], [-HALF_PI, 0.0, 0.0], [14.0, 14.0]),
    ],
};

static SUNSET: PresetSpec = PresetSpec {
    bg: "#100a0b",
    ambient: "#e6b8a2",
    ambient_intensity: 0.45,
    key: "#ffbe85",
    key_intensity: 2.6,
    key_pos: [5.5, 3.2, 3.8],
    rim: "#b79ad8",
    rim_intensity: 1.5,
    rim_pos: [-4.5, 3.4, -4.0],
    cards: [
        card("#ffb06e", 4.5, [7.0, 2.2, 2.0], [0.0, -HALF_PI, 0.0], [7.0, 4.5]),
        card("#a588c9", 2.2, [-7.0, 4.0, -1.0], [0.0, HALF_PI, 0.0], [7.0, 5.0]),
        card("#ffe0bd", 2.4, [0.0, 6.8, 2.0], [HALF_PI, 0.0, 0.0], [9.0, 4.0]),
        card("#221719", 0.7, [0.0, -5.0, 0.0], [-HALF_PI, 0.0, 0.0], [14.0, 14.0]),
    ],
};

static DAYLIGHT: PresetSpec = PresetSpec {
    bg: "#15181e",
    ambient: "#eef2f8",
    ambient_intensity: 0.65,
    key: "#fffaf2",
    key_intensity: 2.6,
    key_pos: [4.5, 6.5, 5.0],
    rim: "#dce8f8",
    rim_intensity: 1.3,
    rim_pos: [-5.0, 3.5, -3.5],
    cards: [
        card("#ffffff", 4.6, [0.0, 7.0, 1.0], [HALF_PI, 0.0, 0.0], [12.0, 8.0]),
        card("#eef4ff", 2.8, [-7.0, 2.5, 1.0], [0.0, HALF_PI, 0.0], [8.0, 6.0]),
        card("#fff6ea", 2.6, [7.0, 2.5, 1.0], [0.0, -HALF_PI, 0.0], [8.0, 6.0]),
        card("#4d535e", 1.2, [0.0, -5.0, 0.0], [-HALF_PI, 0.0, 0.0], [14.0, 14.0]),
    ],
};

fn spec(preset: Scene3dEnvPreset) -> &'static PresetSpec {
    match preset {
        Scene3dEnvPreset::StudioDark => &STUDIO_DARK,
        Scene3dEnvPreset::CityNight => &CITY_NIGHT,
        Scene3dEnvPreset::Sunset => &SUNSET,
        Scene3dEnvPreset::Daylight => &DAYLIGHT,
    }
}

// ───────────────────────── Ambiente procedural ─────────────────────────

/// Retângulo emissivo do ambiente: centro, semi-eixos U/V (já rotacionados e escalados) e
/// radiância linear.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EnvCard {
    pub center: Vec3,
    pub half_u: Vec3,
    pub half_v: Vec3,
    pub radiance: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProceduralEnv {
    pub background: Vec3,
    pub cards: [EnvCard; MAX_ENV_CARDS],
    pub card_count: usize,
}

impl ProceduralEnv {
    pub fn from_preset(preset: Scene3dEnvPreset) -> Self {
        let s = spec(preset);
        let mut cards = [EnvCard::default(); MAX_ENV_CARDS];
        for (dst, c) in cards.iter_mut().zip(s.cards.iter()) {
            // PlaneGeometry(1,1) no plano XY, rotação Euler XYZ do three.js (padrão).
            let rot = Mat3::from_euler(EulerRot::XYZ, c.rot[0], c.rot[1], c.rot[2]);
            *dst = EnvCard {
                center: Vec3::from(c.pos),
                half_u: rot * Vec3::new(c.scale[0] * 0.5, 0.0, 0.0),
                half_v: rot * Vec3::new(0.0, c.scale[1] * 0.5, 0.0),
                radiance: hex_linear(c.color, Vec3::ONE) * c.intensity,
            };
        }
        Self {
            background: hex_linear(s.bg, Vec3::ZERO),
            cards,
            card_count: s.cards.len().min(MAX_ENV_CARDS),
        }
    }
}

// ───────────────────────── Luzes ─────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DirLight {
    /// Direção **para** a luz (normalizada).
    pub direction: Vec3,
    /// Cor linear × intensidade.
    pub radiance: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointLight {
    pub position: Vec3,
    pub radiance: Vec3,
    /// Distância de corte (0 = infinita), como `PointLight.distance`.
    pub range: f32,
    pub decay: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightRig {
    /// Irradiância ambiente (cor × intensidade), aplicada como `AmbientLight`.
    pub ambient: Vec3,
    pub dir: [DirLight; MAX_DIR_LIGHTS],
    pub dir_count: usize,
    pub point: [PointLight; MAX_POINT_LIGHTS],
    pub point_count: usize,
}

impl LightRig {
    pub fn from_preset(preset: Scene3dEnvPreset) -> Self {
        let s = spec(preset);
        Self::key_rim(
            hex_linear(s.ambient, Vec3::ONE) * s.ambient_intensity,
            Vec3::from(s.key_pos),
            hex_linear(s.key, Vec3::ONE) * s.key_intensity,
            Vec3::from(s.rim_pos),
            hex_linear(s.rim, Vec3::ONE) * s.rim_intensity,
        )
    }

    fn key_rim(ambient: Vec3, key_pos: Vec3, key: Vec3, rim_pos: Vec3, rim: Vec3) -> Self {
        let mut dir = [DirLight::default(); MAX_DIR_LIGHTS];
        let mut point = [PointLight::default(); MAX_POINT_LIGHTS];
        // DirectionalLight do three aponta da posição para o alvo (origem).
        dir[0] = DirLight {
            direction: key_pos.try_normalize().unwrap_or(Vec3::Y),
            radiance: key,
        };
        point[0] = PointLight {
            position: rim_pos,
            radiance: rim,
            range: 24.0,
            decay: 2.0,
        };
        Self {
            ambient,
            dir,
            dir_count: 1,
            point,
            point_count: 1,
        }
    }

    /// Ciclo dia/noite (`applyDayNightLighting`): `hour` local 0..24.
    pub fn day_night(hour: f32) -> Self {
        let h = hour.rem_euclid(24.0);
        let (k0, k1, e) = day_night_segment(h);
        let mix = |a: &str, b: &str| hex_linear(a, Vec3::ONE).lerp(hex_linear(b, Vec3::ONE), e);
        let lerp = |a: f32, b: f32| a + (b - a) * e;

        let sun = (h - 6.0) / 12.0 * std::f32::consts::PI;
        let is_day = (6.0..=18.0).contains(&h);
        let arc_x = if is_day { sun.cos() * 6.5 } else { -sun.cos() * 5.0 };
        let arc_y = if is_day { (sun.sin() * 7.2).max(1.2) } else { 4.2 };
        let arc_z = if is_day { 4.2 } else { -3.8 };

        Self::key_rim(
            mix(k0.amb, k1.amb) * lerp(k0.amb_int, k1.amb_int),
            Vec3::new(arc_x, arc_y, arc_z),
            mix(k0.key, k1.key) * lerp(k0.key_int, k1.key_int),
            Vec3::new(-arc_x * 0.7, 2.8, -arc_z * 0.8),
            mix(k0.rim, k1.rim) * lerp(k0.rim_int, k1.rim_int),
        )
    }
}

/// Cor de fundo linear do preset (`ENV_SPECS[preset].bgHex`).
pub fn preset_background(preset: Scene3dEnvPreset) -> Vec3 {
    hex_linear(spec(preset).bg, Vec3::ZERO)
}

/// Céu do ciclo dia/noite (`sampleDayNightBackground`): cor linear interpolada para `hour`.
pub fn day_night_background(hour: f32) -> Vec3 {
    let (k0, k1, e) = day_night_segment(hour.rem_euclid(24.0));
    hex_linear(k0.bg, Vec3::ZERO).lerp(hex_linear(k1.bg, Vec3::ZERO), e)
}

/// Par de chaves que contém `h` (0..24) e o `smoothstep` entre elas (`dayNightSegment`).
fn day_night_segment(h: f32) -> (&'static DayNightKey, &'static DayNightKey, f32) {
    let mut k0 = &DAY_NIGHT[0];
    let mut k1 = &DAY_NIGHT[DAY_NIGHT.len() - 1];
    for pair in DAY_NIGHT.windows(2) {
        if h >= pair[0].hour && h <= pair[1].hour {
            k0 = &pair[0];
            k1 = &pair[1];
            break;
        }
    }
    let span = (k1.hour - k0.hour).max(0.001);
    let t = ((h - k0.hour) / span).clamp(0.0, 1.0);
    (k0, k1, t * t * (3.0 - 2.0 * t))
}

struct DayNightKey {
    hour: f32,
    bg: &'static str,
    amb: &'static str,
    amb_int: f32,
    key: &'static str,
    key_int: f32,
    rim: &'static str,
    rim_int: f32,
}

const fn dn(
    hour: f32,
    bg: &'static str,
    amb: &'static str,
    amb_int: f32,
    key: &'static str,
    key_int: f32,
    rim: &'static str,
    rim_int: f32,
) -> DayNightKey {
    DayNightKey {
        hour,
        bg,
        amb,
        amb_int,
        key,
        key_int,
        rim,
        rim_int,
    }
}

static DAY_NIGHT: [DayNightKey; 7] = [
    dn(0.0, "#05070e", "#6e84b0", 0.3, "#8baae6", 1.4, "#5a6c9e", 1.5),
    dn(5.5, "#0d0c14", "#9f8ba8", 0.38, "#ffa876", 2.0, "#7f89c4", 1.4),
    dn(7.5, "#16151c", "#e6c7b2", 0.52, "#ffd4a3", 2.5, "#b8c8e8", 1.3),
    dn(13.0, "#161a22", "#eef3fa", 0.68, "#fffaf0", 2.8, "#d8e6fa", 1.2),
    dn(18.0, "#140b0e", "#df9e82", 0.48, "#ff9854", 2.7, "#a482cf", 1.6),
    dn(20.5, "#070812", "#788db8", 0.34, "#9ab4e8", 1.6, "#6d7db8", 1.6),
    dn(24.0, "#05070e", "#6e84b0", 0.3, "#8baae6", 1.4, "#5a6c9e", 1.5),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex_srgb("#fff"), Some([1.0; 4]));
        assert!(parse_hex_srgb("07080b").is_none());
        let c = hex_linear("#808080", Vec3::ZERO);
        assert!((c.x - 0.2158605).abs() < 1e-4);
    }

    #[test]
    fn floor_card_faces_up() {
        let env = ProceduralEnv::from_preset(Scene3dEnvPreset::StudioDark);
        let floor = env.cards[3];
        let n = floor.half_u.cross(floor.half_v).normalize();
        assert!(n.y.abs() > 0.99);
    }

    #[test]
    fn day_night_is_continuous_at_midnight() {
        let a = LightRig::day_night(0.0);
        let b = LightRig::day_night(23.999);
        assert!((a.ambient - b.ambient).length() < 1e-2);
        assert!((day_night_background(0.0) - day_night_background(23.999)).length() < 1e-3);
    }

    #[test]
    fn srgb_round_trip() {
        for v in [0.0_f32, 0.002, 0.2, 0.5, 1.0] {
            assert!((srgb_to_linear(linear_to_srgb(v)) - v).abs() < 1e-4);
        }
        assert_eq!(preset_background(Scene3dEnvPreset::StudioDark), hex_linear("#07080b", Vec3::ZERO));
    }
}
