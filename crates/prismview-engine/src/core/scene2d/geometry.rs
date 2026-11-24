//! Geometria 2D do layout (afim, caixas, âncoras, cores, UV de cover).

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Geometria ─────────────────────────

/// Transformação afim 2D: `m` = [m00, m10, m01, m11] (colunas X e Y), `t` = translação.
#[derive(Clone, Copy, Debug)]
pub(super) struct Affine {
    pub(super) m: [f32; 4],
    pub(super) t: [f32; 2],
}

impl Affine {
    pub(super) const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 1.0],
        t: [0.0, 0.0],
    };

    pub(super) fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        [
            self.m[0] * p[0] + self.m[2] * p[1] + self.t[0],
            self.m[1] * p[0] + self.m[3] * p[1] + self.t[1],
        ]
    }

    /// `self ∘ T(center) ∘ R(rot) ∘ S(sx, sy)`: nó filho posicionado em `center` (espaço do pai).
    pub(super) fn child(&self, center: [f32; 2], rot: f32, scale: f32) -> Self {
        self.child_flipped(center, rot, scale, [false, false])
    }

    /// Variante de `child` com espelhamento local horizontal/vertical (`flip = [flip_x, flip_y]`).
    pub(super) fn child_flipped(
        &self,
        center: [f32; 2],
        rot: f32,
        scale: f32,
        flip: [bool; 2],
    ) -> Self {
        let sx = if flip[0] { -scale } else { scale };
        let sy = if flip[1] { -scale } else { scale };
        let (sn, cs) = rot.sin_cos();
        let (c0, c1) = (cs * sx, sn * sx); // coluna X de R·S
        let (c2, c3) = (-sn * sy, cs * sy); // coluna Y de R·S
        let m = &self.m;
        Self {
            m: [
                m[0] * c0 + m[2] * c1,
                m[1] * c0 + m[3] * c1,
                m[0] * c2 + m[2] * c3,
                m[1] * c2 + m[3] * c3,
            ],
            t: self.apply(center),
        }
    }

    /// Fator de escala uniforme (norma da coluna X).
    pub(super) fn scale(&self) -> f32 {
        self.m[0].hypot(self.m[1])
    }

    /// Inversa (`None` se degenerada).
    pub(super) fn inverse(&self) -> Option<Self> {
        let m = &self.m;
        let det = m[0] * m[3] - m[2] * m[1];
        if !det.is_finite() || det.abs() < 1e-8 {
            return None;
        }
        let k = 1.0 / det;
        let inv = [m[3] * k, -m[1] * k, -m[2] * k, m[0] * k];
        let t = [
            -(inv[0] * self.t[0] + inv[2] * self.t[1]),
            -(inv[1] * self.t[0] + inv[3] * self.t[1]),
        ];
        Some(Self { m: inv, t })
    }

    /// `self ∘ other`.
    pub(super) fn then_apply(&self, other: &Self) -> Self {
        let (a, b) = (&self.m, &other.m);
        Self {
            m: [
                a[0] * b[0] + a[2] * b[1],
                a[1] * b[0] + a[3] * b[1],
                a[0] * b[2] + a[2] * b[3],
                a[1] * b[2] + a[3] * b[3],
            ],
            t: self.apply(other.t),
        }
    }

    pub(super) fn is_identity_linear(&self) -> bool {
        (self.m[0] - 1.0).abs() < 1e-4
            && self.m[1].abs() < 1e-4
            && self.m[2].abs() < 1e-4
            && (self.m[3] - 1.0).abs() < 1e-4
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Size {
    pub(super) w: f32,
    pub(super) h: f32,
}

/// Caixa do pai no espaço local do pai (`left/top` relativos ao centro do pai, ou 0,0 na raiz).
#[derive(Clone, Copy, Debug)]
pub(super) struct ParentBox {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

pub(super) fn anchor_factors(anchor: Anchor) -> (f32, f32) {
    match anchor {
        Anchor::TopLeft => (0.0, 0.0),
        Anchor::TopCenter => (0.5, 0.0),
        Anchor::TopRight => (1.0, 0.0),
        Anchor::CenterLeft => (0.0, 0.5),
        Anchor::Center => (0.5, 0.5),
        Anchor::CenterRight => (1.0, 0.5),
        Anchor::BottomLeft => (0.0, 1.0),
        Anchor::BottomCenter => (0.5, 1.0),
        Anchor::BottomRight => (1.0, 1.0),
    }
}

pub(super) fn map_blend(mode: BlendMode) -> BlendKind {
    match mode {
        BlendMode::Plus => BlendKind::Plus,
        BlendMode::Multiply => BlendKind::Multiply,
        BlendMode::Screen => BlendKind::Screen,
        _ => BlendKind::Normal,
    }
}

pub(super) fn with_alpha(mut rgba: [f32; 4], opacity: f32) -> [f32; 4] {
    rgba[3] *= opacity;
    rgba
}

/// `#RRGGBB` / `#AARRGGBB`; texto inválido vira branco opaco.
pub(super) fn parse_color(raw: &str) -> [f32; 4] {
    FxEngine::parse_rgba_hex(raw).unwrap_or([1.0, 1.0, 1.0, 1.0])
}

/// UV que preenche a caixa (`box_w`×`box_h`) com a imagem sem distorcer (recorte central).
pub(super) fn cover_uv(img_w: u32, img_h: u32, box_w: f32, box_h: f32) -> [f32; 4] {
    let (iw, ih) = (img_w.max(1) as f32, img_h.max(1) as f32);
    if box_w <= 0.0 || box_h <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let image_aspect = iw / ih;
    let box_aspect = box_w / box_h;
    if image_aspect > box_aspect {
        let f = box_aspect / image_aspect;
        let m = (1.0 - f) * 0.5;
        [m, 0.0, 1.0 - m, 1.0]
    } else {
        let f = image_aspect / box_aspect;
        let m = (1.0 - f) * 0.5;
        [0.0, m, 1.0, 1.0 - m]
    }
}
