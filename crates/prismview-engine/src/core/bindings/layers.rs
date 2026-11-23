//! Bindings reativos de camadas 2D e do shader de vidro (extensão nativa do contrato `.prism`,
//! lida de `PrismScene::bindings`).
//!
//! ```json
//! { "id": "b1", "layerId": "clock", "source": "AUDIO_BASS", "target": "SCALE_PULSE",
//!   "strength": 0.12, "smoothing": 0.18 }
//! ```
//!
//! Drift-free por construção: a base de cada propriedade (estática ou FX) é reavaliada a cada
//! reconstrução da lista em `scene2d`, e o delta vive em `LayerFx`, recalculado do zero por
//! quadro a partir das molas — o valor nunca é escrito de volta na camada.
//!
//! `strength` (padrões entre parênteses): offset em px lógicos (24), rotação em graus (12),
//! escala relativa (0,12), opacidade somada (-0,5 ⇒ apaga com o sinal), vidro: blur px (18),
//! refração px (24), dispersão (0,006), especular (0,8), borda (0,6), fosco (0,3).

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use super::{or_default, spring_alpha, spring_step, BindingInput};
use crate::engine_core::signals::{SignalKind, SignalSnapshot};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LayerBindingSpec {
    id: String,
    enabled: bool,
    layer_id: String,
    source: String,
    target: String,
    strength: f32,
    smoothing: f32,
    invert: bool,
}

impl Default for LayerBindingSpec {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            layer_id: String::new(),
            source: String::new(),
            target: String::new(),
            strength: 0.0,
            smoothing: 0.0,
            invert: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerTarget {
    OffsetX,
    OffsetY,
    Rotate,
    ScalePulse,
    Opacity,
    GlassBlur,
    GlassRefraction,
    GlassDispersion,
    GlassSpecular,
    GlassRim,
    GlassFrost,
}

impl LayerTarget {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim() {
            "OFFSET_X" => Self::OffsetX,
            "OFFSET_Y" => Self::OffsetY,
            "ROTATE" => Self::Rotate,
            "SCALE_PULSE" => Self::ScalePulse,
            "OPACITY" => Self::Opacity,
            "GLASS_BLUR" => Self::GlassBlur,
            "GLASS_REFRACTION" => Self::GlassRefraction,
            "GLASS_DISPERSION" => Self::GlassDispersion,
            "GLASS_SPECULAR" => Self::GlassSpecular,
            "GLASS_RIM" => Self::GlassRim,
            "GLASS_FROST" => Self::GlassFrost,
            _ => return None,
        })
    }

    fn default_strength(self) -> f32 {
        match self {
            Self::OffsetX | Self::OffsetY => 24.0,
            Self::Rotate => 12.0,
            Self::ScalePulse => 0.12,
            Self::Opacity => -0.5,
            Self::GlassBlur => 18.0,
            Self::GlassRefraction => 24.0,
            Self::GlassDispersion => 0.006,
            Self::GlassSpecular => 0.8,
            Self::GlassRim => 0.6,
            Self::GlassFrost => 0.3,
        }
    }
}

/// Deltas aditivos do vidro óptico (unidades do schema `OpticalCrystalParams`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlassFx {
    pub blur_px: f32,
    pub refraction: f32,
    pub dispersion: f32,
    pub specular: f32,
    pub rim: f32,
    pub frost: f32,
}

/// Deltas aditivos de uma camada 2D (somados à base FX/estática em `scene2d`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerFx {
    /// px lógicos (multiplicados pela escala de UI no layout).
    pub offset: [f32; 2],
    pub rotation_deg: f32,
    /// Escala relativa: efetiva = base × (1 + scale).
    pub scale: f32,
    /// Somada à opacidade própria da camada (resultado limitado a 0..=1).
    pub opacity: f32,
    pub glass: GlassFx,
}

impl LayerFx {
    #[inline]
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

struct LayerBinding {
    id: String,
    layer_id: String,
    input: BindingInput,
    target: LayerTarget,
    strength: f32,
    smoothing: f32,
    sign: f32,
    weight: f32,
}

#[derive(Default)]
pub struct LayerBindingSolver {
    bindings: Vec<LayerBinding>,
    /// Uma entrada por camada com binding (chaves criadas no load; o quadro só reescreve valores).
    fx: HashMap<String, LayerFx>,
    demand: SignalKind,
    animating: bool,
}

impl LayerBindingSolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Carrega `PrismScene::bindings`. Mantém o estado das molas por `id`.
    pub fn set_specs(&mut self, raw: &[Value]) {
        let old = std::mem::take(&mut self.bindings);
        self.fx.clear();
        let mut demand = SignalKind::NONE;
        for value in raw {
            let Ok(spec) = LayerBindingSpec::deserialize(value) else {
                continue;
            };
            if !spec.enabled || spec.layer_id.trim().is_empty() {
                continue;
            }
            // Camadas 2D só usam escalares do bus (hover/clique de malha são do 3D).
            let Some(input @ BindingInput::Signal(_)) = BindingInput::parse(&spec.source) else {
                continue;
            };
            let Some(target) = LayerTarget::parse(&spec.target) else {
                continue;
            };
            demand |= input.kind();
            let weight = old
                .iter()
                .find(|b| !spec.id.is_empty() && b.id == spec.id)
                .map_or(0.0, |b| b.weight);
            self.fx.entry(spec.layer_id.clone()).or_default();
            self.bindings.push(LayerBinding {
                id: spec.id,
                layer_id: spec.layer_id,
                input,
                target,
                strength: or_default(spec.strength, target.default_strength()),
                smoothing: spec.smoothing,
                sign: if spec.invert { -1.0 } else { 1.0 },
                weight,
            });
        }
        self.demand = demand;
        self.animating = false;
    }

    pub fn clear(&mut self) {
        self.bindings.clear();
        self.fx.clear();
        self.demand = SignalKind::NONE;
        self.animating = false;
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn demand(&self) -> SignalKind {
        self.demand
    }

    pub fn is_animating(&self) -> bool {
        self.animating
    }

    /// `true` se as molas estão ativas e a camada `layer_id` possui algum binding ativo.
    #[inline]
    pub fn is_layer_animating(&self, layer_id: &str) -> bool {
        self.animating && self.bindings.iter().any(|b| b.layer_id == layer_id)
    }

    /// A camada tem algum binding (mesmo com delta nulo agora).
    #[inline]
    pub fn has_binding(&self, layer_id: &str) -> bool {
        self.bindings.iter().any(|b| b.layer_id == layer_id)
    }

    /// Delta atual da camada (`None` = sem binding ou delta nulo).
    #[inline]
    pub fn fx(&self, layer_id: &str) -> Option<LayerFx> {
        self.fx.get(layer_id).copied().filter(|f| !f.is_zero())
    }

    /// Avança as molas e recalcula os deltas do zero. `true` se algum delta mudou (a lista de
    /// desenho precisa ser refeita). Sem alocação: só reescreve entradas existentes.
    pub fn solve(&mut self, snap: &SignalSnapshot, dt: f32) -> bool {
        if self.bindings.is_empty() {
            return false;
        }
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.25) } else { 0.0 };
        let mut changed = false;
        for b in &mut self.bindings {
            let raw = b.input.scalar(snap);
            changed |= spring_step(&mut b.weight, raw, spring_alpha(b.smoothing, 0.18, dt));
        }
        self.animating = changed;
        if !changed {
            return false;
        }
        // Desfaz (zera) → reaplica todos: nenhum delta sobrevive de um quadro para o outro.
        self.fx.values_mut().for_each(|f| *f = LayerFx::default());
        for b in &self.bindings {
            let Some(fx) = self.fx.get_mut(b.layer_id.as_str()) else {
                continue;
            };
            let v = b.weight * b.strength * b.sign;
            match b.target {
                LayerTarget::OffsetX => fx.offset[0] += v,
                LayerTarget::OffsetY => fx.offset[1] += v,
                LayerTarget::Rotate => fx.rotation_deg += v,
                LayerTarget::ScalePulse => fx.scale += v,
                LayerTarget::Opacity => fx.opacity += v,
                LayerTarget::GlassBlur => fx.glass.blur_px += v,
                LayerTarget::GlassRefraction => fx.glass.refraction += v,
                LayerTarget::GlassDispersion => fx.glass.dispersion += v,
                LayerTarget::GlassSpecular => fx.glass.specular += v,
                LayerTarget::GlassRim => fx.glass.rim += v,
                LayerTarget::GlassFrost => fx.glass.frost += v,
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_returns_to_zero_without_drift() {
        let mut s = LayerBindingSolver::new();
        s.set_specs(&[serde_json::json!({
            "id": "a", "layerId": "L", "source": "AUDIO_BASS", "target": "ROTATE", "strength": 10
        })]);
        let mut snap = SignalSnapshot::default();
        snap.bass = 1.0;
        for _ in 0..120 {
            s.solve(&snap, 1.0 / 60.0);
        }
        assert_eq!(s.fx("L").map(|f| f.rotation_deg), Some(10.0));
        snap.bass = 0.0;
        for _ in 0..600 {
            s.solve(&snap, 1.0 / 60.0);
        }
        assert!(s.fx("L").is_none());
    }

    #[test]
    fn layer_animating_tracks_active_parallax() {
        let mut s = LayerBindingSolver::new();
        s.set_specs(&[serde_json::json!({
            "id": "p", "layerId": "clock", "source": "CURSOR_X", "target": "OFFSET_X",
            "strength": 18.0, "smoothing": 0.12
        })]);
        let mut snap = SignalSnapshot::default();
        snap.cursor_norm = [1.0, 0.0];
        assert!(s.solve(&snap, 1.0 / 60.0));
        assert!(s.is_layer_animating("clock"));
        assert!(!s.is_layer_animating("date"));
        for _ in 0..300 {
            s.solve(&snap, 1.0 / 60.0);
        }
        assert!(!s.is_layer_animating("clock"));
    }
}
