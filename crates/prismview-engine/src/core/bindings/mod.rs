//! BindingSolver (Fase 4): liga o `SignalBus` (via `SignalSnapshot`) a objetos 3D, shaders e
//! camadas 2D, com pipeline **aditivo drift-free**:
//!
//! 1. desfaz o offset/rotação do quadro anterior — nunca por inversão numérica: a base limpa é
//!    restaurada (`Pose::restore_animated` no 3D; no 2D/vidro a base vem do FX/estático a cada
//!    reconstrução e o delta fica numa tabela à parte);
//! 2. atualiza a animação (clipe glTF amostrado sobre a pose de repouso);
//! 3. reaplica os bindings (`AdditiveLayer` no 3D, `LayerFx` no 2D/vidro).
//!
//! Como o delta de cada quadro é recalculado do zero a partir do estado das molas, não há erro
//! acumulado (o "drift" de `q·q⁻¹` em float) nem após horas de reprodução.
//!
//! - `scene3d`: contrato `ReactiveBinding3D` do Estúdio (`Scene3DLayer.tsx`), 1:1 nas fórmulas
//!   (mola `1 - exp(-k·60·dt)`, snap 0,002, pulso de clique, giro acumulado);
//! - `layers`: extensão nativa para camadas 2D (`PrismScene::bindings`) e parâmetros do vidro.
//!
//! Regras: nada de SO aqui; nenhuma alocação por quadro (estados e alvos resolvidos no load).

pub mod layers;
pub mod scene3d;

use glam::Vec3;

use super::signals::{SignalKind, SignalSnapshot, SignalSource};

pub use layers::{GlassFx, LayerBindingSolver, LayerFx};
pub use scene3d::{BindingSolver3d, InstanceAccess, PickHit};

/// Limiar de "assentou" das molas (igual ao Estúdio).
pub const SETTLE_EPS: f32 = 0.002;

/// Eixo de `NODE_ROTATE_SWING` / `NODE_MOVE_OFFSET` / `NODE_ROTATE_SPIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Axis {
    X,
    #[default]
    Y,
    Z,
}

impl Axis {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some(a) if a.eq_ignore_ascii_case("X") => Self::X,
            Some(a) if a.eq_ignore_ascii_case("Z") => Self::Z,
            _ => Self::Y,
        }
    }

    #[inline]
    pub fn unit(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
    }
}

/// Origem do valor de um binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingInput {
    /// Escalar do `SignalBus` (áudio, teclado, mídia, telemetria, bateria).
    Signal(SignalSource),
    /// Cursor normalizado sobre a camada → yaw/pitch.
    CursorLook,
    /// 1 enquanto o cursor está sobre a malha `sourceNode`.
    HoverMesh,
    /// Pulso/alternância a cada clique na malha `sourceNode`.
    ClickMesh,
    /// Valor de uma fórmula FX (`$df(m)$`, `$mi(pos)$`, `$sys(cpu)$`, globais...) normalizado por
    /// `formulaMin..formulaMax`. Avaliada pelo host só quando o contexto FX muda (nunca por quadro).
    Formula,
}

impl BindingInput {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim() {
            "CURSOR_LOOK" => Self::CursorLook,
            "HOVER_MESH" => Self::HoverMesh,
            "CLICK_MESH" => Self::ClickMesh,
            "FORMULA" => Self::Formula,
            other => Self::Signal(SignalSource::parse(other)?),
        })
    }

    /// Que sinal precisa estar ativo no bus para alimentar este binding.
    pub fn kind(self) -> SignalKind {
        match self {
            Self::Signal(s) => s.kind(),
            Self::CursorLook | Self::HoverMesh => SignalKind::MOUSE,
            Self::ClickMesh => SignalKind::DESKTOP_TAP,
            // Dependências (mídia/telemetria/relógio) são resolvidas pelo host ao avaliar a fórmula.
            Self::Formula => SignalKind::NONE,
        }
    }

    /// Lê o valor escalar 0..=1 (entradas de cursor são tratadas à parte).
    #[inline]
    pub fn scalar(self, snap: &SignalSnapshot) -> f32 {
        match self {
            Self::Signal(s) => snap.get(s),
            _ => 0.0,
        }
    }
}

/// `1 - exp(-k·60·dt)` com `k = clamp(smoothing ou padrão, 0,02, 0,45)` (fórmula do Estúdio).
#[inline]
pub fn spring_alpha(smoothing: f32, default: f32, dt: f32) -> f32 {
    let k = if smoothing.is_finite() && smoothing > 0.0 { smoothing } else { default };
    1.0 - (-k.clamp(0.02, 0.45) * 60.0 * dt).exp()
}

/// Avança `value` para `target`. Devolve `true` se o valor mudou neste quadro.
#[inline]
pub fn spring_step(value: &mut f32, target: f32, alpha: f32) -> bool {
    let target = if target.is_finite() { target } else { 0.0 };
    let prev = *value;
    *value += (target - *value) * alpha;
    if (target - *value).abs() < SETTLE_EPS {
        *value = target;
    }
    *value != prev
}

/// Fórmula → 0..=1: `(v - min) / (max - min)` limitado; faixa degenerada = `v` limitado a 0..1.
/// Espelho de `formulaUnit` (Estúdio, `types/prism.ts`).
#[inline]
pub fn formula_unit(value: f32, min: f32, max: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    let (min, max) = (
        if min.is_finite() { min } else { 0.0 },
        if max.is_finite() { max } else { 1.0 },
    );
    let span = max - min;
    let t = if span.abs() < 1e-9 { value } else { (value - min) / span };
    t.clamp(0.0, 1.0)
}

/// Mola com volta circular em 0..1 (`formulaWrap`): segue o caminho mais curto (59 min → 0 gira
/// para frente, não volta a hora inteira). Devolve `true` se mudou.
#[inline]
pub fn spring_step_wrapped(value: &mut f32, target: f32, alpha: f32) -> bool {
    let target = if target.is_finite() { target.rem_euclid(1.0) } else { 0.0 };
    let prev = *value;
    let mut delta = target - *value;
    if delta > 0.5 {
        delta -= 1.0;
    } else if delta < -0.5 {
        delta += 1.0;
    }
    if delta.abs() < SETTLE_EPS {
        *value = target;
    } else {
        *value = (*value + delta * alpha).rem_euclid(1.0);
    }
    *value != prev
}

/// Valor numérico com padrão quando ausente/zero/inválido (`b.strength || padrão` do TS).
#[inline]
pub fn or_default(v: f32, default: f32) -> f32 {
    if v.is_finite() && v != 0.0 {
        v
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_unit_maps_and_clamps() {
        // Ponteiro de minutos: `$df(m)$` 0..60 → 0..1.
        assert_eq!(formula_unit(15.0, 0.0, 60.0), 0.25);
        assert_eq!(formula_unit(90.0, 0.0, 60.0), 1.0);
        assert_eq!(formula_unit(-3.0, 0.0, 60.0), 0.0);
        // Faixa invertida também funciona (max < min).
        assert_eq!(formula_unit(15.0, 60.0, 0.0), 0.75);
        // Faixa degenerada: valor já em 0..1.
        assert_eq!(formula_unit(0.4, 5.0, 5.0), 0.4);
        assert_eq!(formula_unit(f32::NAN, 0.0, 1.0), 0.0);
    }

    #[test]
    fn wrapped_spring_takes_short_path() {
        // 59 min (0,983) → 0 min: anda para frente cruzando 1,0 (não volta a hora inteira).
        let mut v = 59.0 / 60.0;
        let alpha = spring_alpha(0.2, 0.18, 1.0 / 60.0);
        assert!(spring_step_wrapped(&mut v, 0.0, alpha));
        assert!(v > 59.0 / 60.0 || v < 0.01, "andou para trás: {v}");
        for _ in 0..600 {
            spring_step_wrapped(&mut v, 0.0, alpha);
        }
        assert_eq!(v, 0.0);
    }
}
