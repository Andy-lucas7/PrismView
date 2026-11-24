//! Alongamento vetorial por haste ("stem stretch"), estilo relógios Samsung/Xiaomi/Apple.
//!
//! Trabalha direto no contorno do glifo (nada de raster, nada de escala uniforme):
//!
//! 1. **Mapa vertical por glifo** `F(y)`, monotônico, que só estica onde o contorno é *haste*:
//!    linhas de varredura cujo tinta tem corridas estreitas (≈ largura da haste) e bordas quase
//!    verticais. Barras horizontais, topo/base de arcos e diagonais recebem peso baixo e quase
//!    não esticam; linhas sem tinta absorvem o excedente.
//! 2. **Âncoras tipográficas** (baseline, altura-x, caixa-alta, ascendente, descendente): `F`
//!    passa exatamente por `s·L` em cada linha de referência, então todos os glifos crescem para
//!    a mesma altura e alinham entre si.
//! 3. **Compensação de espessura**: onde sobra inclinação (`F' > 1`) a borda é recuada ao longo
//!    da normal para que a *espessura perpendicular* do traço continue igual (barras e diagonais
//!    não engrossam; círculos mantêm a curvatura das pontas).
//! 4. **Contornos pequenos** (pontos, tilde, pingos) são transladados rigidamente: continuam
//!    redondos.
//!
//! Sem dependência de GPU ou de SO: testável e usada pelo motor nativo (`prismview-engine`).

use i_overlay::core::fill_rule::FillRule;
use i_overlay::float::simplify::SimplifyShape;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

pub type P = [f32; 2];

/// Contorno fechado como polilinha (último ponto ≠ primeiro). Eixo y para cima (unidades da fonte).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contour {
    pub pts: Vec<P>,
}

// ───────────────────────── Extração de contornos ─────────────────────────

struct Flatten {
    out: Vec<Contour>,
    cur: Vec<P>,
    last: P,
    tol: f32,
}

impl Flatten {
    fn flush(&mut self) {
        if self.cur.len() >= 3 {
            let first = self.cur[0];
            if let Some(last) = self.cur.last() {
                if (last[0] - first[0]).abs() < 1e-4 && (last[1] - first[1]).abs() < 1e-4 {
                    self.cur.pop();
                }
            }
            if self.cur.len() >= 3 {
                self.out.push(Contour { pts: std::mem::take(&mut self.cur) });
                return;
            }
        }
        self.cur.clear();
    }

    fn quad(&mut self, p0: P, p1: P, p2: P, depth: u32) {
        // Distância do controle à corda (≈ 2× o desvio máximo da curva).
        let d = point_line_dist(p1, p0, p2);
        if depth >= 12 || d <= self.tol * 2.0 {
            self.cur.push(p2);
            return;
        }
        let a = mid(p0, p1);
        let b = mid(p1, p2);
        let m = mid(a, b);
        self.quad(p0, a, m, depth + 1);
        self.quad(m, b, p2, depth + 1);
    }

    fn cubic(&mut self, p0: P, p1: P, p2: P, p3: P, depth: u32) {
        let d = point_line_dist(p1, p0, p3).max(point_line_dist(p2, p0, p3));
        if depth >= 12 || d <= self.tol * 1.33 {
            self.cur.push(p3);
            return;
        }
        let a = mid(p0, p1);
        let b = mid(p1, p2);
        let c = mid(p2, p3);
        let ab = mid(a, b);
        let bc = mid(b, c);
        let m = mid(ab, bc);
        self.cubic(p0, a, ab, m, depth + 1);
        self.cubic(m, bc, c, p3, depth + 1);
    }
}

fn mid(a: P, b: P) -> P {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

fn point_line_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-12 {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len2.sqrt()
}

impl OutlineBuilder for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        self.last = [x, y];
        self.cur.push([x, y]);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.last = [x, y];
        self.cur.push([x, y]);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p0 = self.last;
        self.quad(p0, [x1, y1], [x, y], 0);
        self.last = [x, y];
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.last;
        self.cubic(p0, [x1, y1], [x2, y2], [x, y], 0);
        self.last = [x, y];
    }
    fn close(&mut self) {
        self.flush();
    }
}

/// Contornos do glifo em unidades da fonte, achatados com tolerância `tol_units`.
pub fn glyph_contours(face: &Face<'_>, gid: GlyphId, tol_units: f32) -> Option<Vec<Contour>> {
    let mut b = Flatten { out: Vec::new(), cur: Vec::new(), last: [0.0, 0.0], tol: tol_units.max(0.01) };
    face.outline_glyph(gid, &mut b)?;
    b.flush();
    Some(union_contours(b.out))
}

/// União (nonzero) dos contornos: remove sobreposições (fontes variáveis, "2" = barra + curva,
/// "B" = haste + bojos) para que só exista borda visível, sem arestas internas.
pub fn union_contours(contours: Vec<Contour>) -> Vec<Contour> {
    if contours.len() < 2 {
        return contours;
    }
    let paths: Vec<Vec<[f32; 2]>> = contours.iter().map(|c| c.pts.clone()).collect();
    let shapes = paths.simplify_shape(FillRule::NonZero);
    let out: Vec<Contour> = shapes
        .into_iter()
        .flat_map(|shape| shape.into_iter())
        .filter(|path| path.len() >= 3)
        .map(|pts| Contour { pts })
        .collect();
    if out.is_empty() {
        contours
    } else {
        out
    }
}

// ───────────────────────── Métricas da fonte ─────────────────────────

/// Linhas de referência e traço típico da fonte, medidos nos próprios glifos (independe de OS/2).
#[derive(Clone, Debug, PartialEq)]
pub struct FontLines {
    pub upem: f32,
    /// Largura típica da haste (unidades da fonte).
    pub stroke: f32,
    /// Linhas de referência ordenadas (inclui a baseline 0).
    pub lines: Vec<f32>,
    /// Inclinação das hastes (`dx/dy`; itálico à direita > 0).
    pub slant: f32,
}

impl FontLines {
    pub fn measure(face: &Face<'_>) -> Self {
        let upem = face.units_per_em().max(1) as f32;
        let bbox = |c: char| -> Option<(f32, f32)> {
            let g = face.glyph_index(c)?;
            let r = face.glyph_bounding_box(g)?;
            Some((r.y_min as f32, r.y_max as f32))
        };
        let mut raw: Vec<f32> = vec![0.0];
        for c in ['x', 'z', 'H', 'E', '1', '7', 'd', 'l', 'k'] {
            if let Some((_, top)) = bbox(c) {
                if top > upem * 0.2 {
                    raw.push(top);
                }
            }
        }
        for c in ['p', 'q', 'g'] {
            if let Some((bot, _)) = bbox(c) {
                if bot < -upem * 0.06 {
                    raw.push(bot);
                }
            }
        }
        raw.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // Agrupa linhas a menos de 2,5% do em (ex.: caixa-alta 700 e algarismos 704).
        let tol = upem * 0.025;
        let mut lines: Vec<f32> = Vec::new();
        let mut group: Vec<f32> = Vec::new();
        for v in raw {
            if let Some(&g0) = group.first() {
                if (v - g0).abs() > tol {
                    lines.push(group.iter().sum::<f32>() / group.len() as f32);
                    group.clear();
                }
            }
            group.push(v);
        }
        if !group.is_empty() {
            lines.push(group.iter().sum::<f32>() / group.len() as f32);
        }
        // A baseline fica exatamente em 0.
        if let Some(i) = lines.iter().position(|v| v.abs() <= tol) {
            lines[i] = 0.0;
        } else {
            lines.push(0.0);
            lines.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        }

        let slant = {
            let t = (-face.italic_angle()).to_radians().tan();
            if t.is_finite() && t.abs() < 0.7 { t } else { 0.0 }
        };

        // Haste: corrida de tinta de 'l' (ou 'I', '1', 'i') na metade da altura-x.
        let xh = bbox('x').map(|b| b.1).unwrap_or(upem * 0.5);
        let mut stroke = 0.0f32;
        for c in ['l', 'I', 'i', '1'] {
            let Some(g) = face.glyph_index(c) else { continue };
            let Some(cs) = glyph_contours(face, g, upem * 0.002) else { continue };
            let ivs = scan_intervals(&cs, xh * 0.5);
            if let Some(m) = ivs.iter().map(|iv| iv.x1 - iv.x0).filter(|l| *l > upem * 0.01).fold(None, |a: Option<f32>, l| Some(a.map_or(l, |v| v.min(l)))) {
                stroke = m;
                break;
            }
        }
        if !(stroke > upem * 0.01 && stroke < upem * 0.5) {
            stroke = upem * 0.09;
        }
        Self { upem, stroke, lines, slant }
    }

    /// Métricas sintéticas (testes): baseline 0, altura-x `xh`, caixa-alta `cap`.
    pub fn synthetic(upem: f32, stroke: f32, xh: f32, cap: f32) -> Self {
        Self { upem, stroke, lines: vec![0.0, xh, cap], slant: 0.0 }
    }
}

// ───────────────────────── Varredura ─────────────────────────

#[derive(Clone, Copy, Debug)]
struct Interval {
    x0: f32,
    x1: f32,
    /// |dx/dy| das duas bordas.
    s0: f32,
    s1: f32,
}

#[derive(Clone, Copy)]
struct Edge {
    x0: f32,
    y0: f32,
    y1: f32,
    inv: f32, // dx/dy
    dir: i32,
}

fn build_edges(contours: &[&Contour]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for c in contours {
        let n = c.pts.len();
        for i in 0..n {
            let a = c.pts[i];
            let b = c.pts[(i + 1) % n];
            if (a[1] - b[1]).abs() < 1e-6 {
                continue;
            }
            let (lo, hi, dir) = if a[1] < b[1] { (a, b, 1) } else { (b, a, -1) };
            edges.push(Edge { x0: lo[0], y0: lo[1], y1: hi[1], inv: (hi[0] - lo[0]) / (hi[1] - lo[1]), dir });
        }
    }
    edges
}

fn scan_edges(edges: &[Edge], y: f32, slant: f32, buf: &mut Vec<(f32, i32, f32)>, out: &mut Vec<Interval>) {
    buf.clear();
    out.clear();
    for e in edges {
        if y >= e.y0 && y < e.y1 {
            buf.push((e.x0 + (y - e.y0) * e.inv, e.dir, ((e.inv - slant).abs())));
        }
    }
    buf.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut wind = 0;
    let mut start: Option<(f32, f32)> = None;
    for &(x, d, s) in buf.iter() {
        let before = wind;
        wind += d;
        if before == 0 && wind != 0 {
            start = Some((x, s));
        } else if before != 0 && wind == 0 {
            if let Some((x0, s0)) = start.take() {
                if x - x0 > 1e-4 {
                    out.push(Interval { x0, x1: x, s0, s1: s });
                }
            }
        }
    }
}

/// Intervalos de tinta em `y` (usado também para medir a haste).
fn scan_intervals(contours: &[Contour], y: f32) -> Vec<Interval> {
    let refs: Vec<&Contour> = contours.iter().collect();
    let edges = build_edges(&refs);
    let (mut buf, mut out) = (Vec::new(), Vec::new());
    scan_edges(&edges, y, 0.0, &mut buf, &mut out);
    out
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ───────────────────────── Mapa vertical ─────────────────────────

/// Parâmetros ajustáveis do alongamento.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemParams {
    /// Piso de peso: garante solução em glifos só com diagonais/barras (a espessura é compensada).
    pub weight_floor: f32,
    /// Teto da intensidade (K) para não concentrar demais o estiramento.
    pub max_boost: f32,
    /// 0 = sem compensação de espessura; 1 = completa.
    pub compensation: f32,
}

impl Default for StemParams {
    fn default() -> Self {
        Self { weight_floor: 0.10, max_boost: 16.0, compensation: 1.0 }
    }
}

/// Mapa `y → F(y)` por glifo (linear por partes entre nós) e a inclinação local `F'`.
#[derive(Clone, Debug)]
pub struct StemWarp {
    nodes: Vec<f32>,
    f: Vec<f32>,
    /// Inclinação em cada célula (entre `nodes[i]` e `nodes[i+1]`).
    slope: Vec<f32>,
}

impl StemWarp {
    pub fn identity() -> Self {
        Self { nodes: vec![0.0, 1.0], f: vec![0.0, 1.0], slope: vec![1.0] }
    }

    fn cell(&self, y: f32) -> usize {
        match self.nodes.binary_search_by(|n| n.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)) {
            Ok(i) => i.min(self.slope.len() - 1),
            Err(0) => 0,
            Err(i) => (i - 1).min(self.slope.len() - 1),
        }
    }

    /// `F(y)`; fora dos nós a inclinação é 1 (translação).
    pub fn map(&self, y: f32) -> f32 {
        let last = self.nodes.len() - 1;
        if y <= self.nodes[0] {
            return self.f[0] + (y - self.nodes[0]);
        }
        if y >= self.nodes[last] {
            return self.f[last] + (y - self.nodes[last]);
        }
        let i = self.cell(y);
        self.f[i] + (y - self.nodes[i]) * self.slope[i]
    }

    /// `F'(y)` suavizado (interpola entre centros de célula).
    pub fn slope_at(&self, y: f32) -> f32 {
        let last = self.nodes.len() - 1;
        if y <= self.nodes[0] || y >= self.nodes[last] {
            return 1.0;
        }
        let i = self.cell(y);
        let c = 0.5 * (self.nodes[i] + self.nodes[i + 1]);
        let (j, t) = if y >= c {
            if i + 1 < self.slope.len() {
                let c2 = 0.5 * (self.nodes[i + 1] + self.nodes[i + 2]);
                (i + 1, (y - c) / (c2 - c).max(1e-6))
            } else {
                (i, 0.0)
            }
        } else if i > 0 {
            let c0 = 0.5 * (self.nodes[i - 1] + self.nodes[i]);
            (i - 1, (c - y) / (c - c0).max(1e-6))
        } else {
            (i, 0.0)
        };
        self.slope[i] * (1.0 - t) + self.slope[j] * t
    }
}

fn bbox_of(c: &Contour) -> (f32, f32, f32, f32) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &c.pts {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    (x0, y0, x1, y1)
}

/// Contorno que deve ser só transladado, nunca esticado: pontos/pingos (≤ ~2 hastes) e formas
/// compactas e cheias sem furo (discos, quadrados: "●", "■", pontos grandes de fontes pesadas).
fn is_rigid(c: &Contour, all: &[Contour], stroke: f32, outer_sign: f32) -> bool {
    if signed_area(c).signum() != outer_sign {
        return false; // furo: acompanha a tinta ao redor
    }
    let (x0, y0, x1, y1) = bbox_of(c);
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= stroke * 2.1 && h <= stroke * 2.1 {
        return true;
    }
    let aspect = w.max(h) / w.min(h).max(1e-3);
    let fill = signed_area(c).abs() / (w * h).max(1e-3);
    if aspect > 1.5 || fill < 0.62 {
        return false;
    }
    // Com furo dentro (anéis como "O", "0") não é disco.
    !all.iter().any(|o| {
        if std::ptr::eq(o, c) {
            return false;
        }
        let (a0, b0, a1, b1) = bbox_of(o);
        a0 >= x0 && a1 <= x1 && b0 >= y0 && b1 <= y1
    })
}

fn signed_area(c: &Contour) -> f32 {
    let n = c.pts.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = c.pts[i];
        let q = c.pts[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Analisa o glifo e devolve o mapa vertical para `stretch` (≥ 1).
pub fn analyze(contours: &[Contour], lines: &FontLines, stretch: f32, params: &StemParams) -> StemWarp {
    let s = stretch.max(1.0);
    if s <= 1.0005 || contours.is_empty() || lines.lines.len() < 2 {
        return StemWarp::identity();
    }
    let outer_sign = contours
        .iter()
        .map(|c| signed_area(c))
        .fold((0.0f32, 1.0f32), |acc, a| if a.abs() > acc.0 { (a.abs(), a.signum()) } else { acc })
        .1;
    // Pontos/pingos não participam da análise (ficam livres e viram translação rígida).
    let body: Vec<&Contour> = contours.iter().filter(|c| !is_rigid(c, contours, lines.stroke, outer_sign)).collect();
    let edges = build_edges(&body);
    let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
    for c in &body {
        for p in &c.pts {
            ymin = ymin.min(p[1]);
            ymax = ymax.max(p[1]);
        }
    }
    let have_body = ymin <= ymax;
    let stroke = lines.stroke.max(1.0);
    let dy = (stroke / 8.0).max(upem_floor(lines));

    // Âncoras: linhas de referência; o glifo pode ficar entre/atravessar linhas.
    let anchors = &lines.lines;
    let mut nodes: Vec<f32> = Vec::new();
    let mut cell_w: Vec<f32> = Vec::new();
    let mut buf = Vec::new();
    let mut ivs = Vec::new();
    for seg in anchors.windows(2) {
        let (a0, a1) = (seg[0], seg[1]);
        let n = (((a1 - a0) / dy).ceil() as usize).max(1);
        let c = (a1 - a0) / n as f32;
        if nodes.is_empty() {
            nodes.push(a0);
        }
        for k in 0..n {
            let ys = a0 + c * k as f32;
            let w = if have_body {
                // 3 amostras por célula: robusto a quinas e a arcos finos.
                let mut acc = 0.0;
                for off in [0.2f32, 0.5, 0.8] {
                    acc += row_weight(&edges, ys + c * off, ymin, ymax, stroke, lines.slant, params, &mut buf, &mut ivs);
                }
                acc / 3.0
            } else {
                1.0
            };
            cell_w.push(w);
            nodes.push(ys + c);
        }
    }
    // Resolve K por segmento entre âncoras: ∫(1 + K·w) = s·len.
    let mut f = vec![0.0f32; nodes.len()];
    let mut slope = vec![1.0f32; cell_w.len()];
    f[0] = s * anchors[0];
    let mut idx = 0usize;
    for seg in anchors.windows(2) {
        let (a0, a1) = (seg[0], seg[1]);
        let n = (((a1 - a0) / dy).ceil() as usize).max(1);
        let c = (a1 - a0) / n as f32;
        let sum_w: f32 = cell_w[idx..idx + n].iter().map(|w| w * c).sum();
        let k = if sum_w > 1e-4 { (((s - 1.0) * (a1 - a0)) / sum_w).min(params.max_boost) } else { 0.0 };
        for j in 0..n {
            let sl = 1.0 + k * cell_w[idx + j];
            slope[idx + j] = sl;
            f[idx + j + 1] = f[idx + j] + sl * c;
        }
        // Se K foi limitado, fecha o resto distribuindo uniformemente para manter o alinhamento.
        let want = s * a1;
        let got = f[idx + n];
        if (want - got).abs() > 1e-3 {
            let fix = (want - f[idx]) / (got - f[idx]).max(1e-6);
            for j in 0..n {
                slope[idx + j] *= fix;
                f[idx + j + 1] = f[idx + j] + slope[idx + j] * c;
            }
        }
        idx += n;
    }
    StemWarp { nodes, f, slope }
}

fn upem_floor(lines: &FontLines) -> f32 {
    (lines.upem / 2048.0).max(0.25)
}

#[allow(clippy::too_many_arguments)]
fn row_weight(
    edges: &[Edge],
    y: f32,
    ymin: f32,
    ymax: f32,
    stroke: f32,
    slant: f32,
    params: &StemParams,
    buf: &mut Vec<(f32, i32, f32)>,
    ivs: &mut Vec<Interval>,
) -> f32 {
    if y < ymin || y > ymax {
        return 1.0; // fora do glifo: livre
    }
    scan_edges(edges, y, slant, buf, ivs);
    if ivs.is_empty() {
        return 1.0; // sem tinta: livre
    }
    let (mut num, mut den) = (0.0f32, 0.0f32);
    for iv in ivs.iter() {
        let l = iv.x1 - iv.x0;
        let width_f = 1.0 - smoothstep(1.35 * stroke, 2.3 * stroke, l);
        let vert_f = 1.0 - smoothstep(0.05, 0.32, iv.s0.max(iv.s1));
        num += l * width_f * vert_f;
        den += l;
    }
    (num / den.max(1e-6)).max(params.weight_floor)
}

// ───────────────────────── Aplicação ─────────────────────────

/// Aplica o mapa ao glifo: estica as hastes, compensa a espessura e mantém pontos rígidos.
pub fn warp_glyph(contours: &[Contour], warp: &StemWarp, lines: &FontLines, params: &StemParams) -> Vec<Contour> {
    let outer_sign = contours
        .iter()
        .map(signed_area)
        .fold((0.0f32, 1.0f32), |acc, a| if a.abs() > acc.0 { (a.abs(), a.signum()) } else { acc })
        .1;
    // Tinta à direita da direção do contorno externo horário (TrueType) ou à esquerda (CFF).
    // `ink_left` = true quando o externo é anti-horário (área > 0 com y para cima).
    let ink_left = outer_sign > 0.0;
    let stroke = lines.stroke;
    let flat = flat_edges(contours);
    let mut out = Vec::with_capacity(contours.len());
    for c in contours {
        let n = c.pts.len();
        if n < 3 {
            continue;
        }
        if is_rigid(c, contours, stroke, outer_sign) {
            let yc = c.pts.iter().map(|p| p[1]).sum::<f32>() / n as f32;
            let dy = warp.map(yc) - yc;
            out.push(Contour { pts: c.pts.iter().map(|p| [p[0], p[1] + dy]).collect() });
            continue;
        }
        // Pontos já mapeados (sem compensação) e, por aresta, recuo + normal externa NOVA.
        let wpts: Vec<P> = c.pts.iter().map(|p| [p[0], warp.map(p[1])]).collect();
        let mut shift = vec![0.0f32; n];
        let mut internal = vec![false; n];
        let mut nrm = vec![[0.0f32; 2]; n];
        for i in 0..n {
            let (a, b) = (c.pts[i], c.pts[(i + 1) % n]);
            nrm[i] = out_normal(wpts[i], wpts[(i + 1) % n], ink_left);
            if params.compensation <= 0.0 {
                continue;
            }
            let no = out_normal(a, b, ink_left); // normal original (para fora da tinta)
            let mid_pt = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
            if ink_at(&flat, [mid_pt[0] + no[0] * stroke * 0.03, mid_pt[1] + no[1] * stroke * 0.03]) {
                internal[i] = true; // aresta interna de contornos sobrepostos: não é borda visível
                continue;
            }
            // Espessura REAL do traço sob esta aresta (raio para dentro até a borda oposta). Em arestas
            // longas amostra vários pontos: basta um trecho fino (barra) para compensar a aresta toda.
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let samples = ((len / stroke).ceil() as usize).clamp(1, 12);
            let mut best: Option<(f32, P)> = None; // (espessura, ponto de origem)
            for k in 0..samples {
                let u = (k as f32 + 0.5) / samples as f32;
                let o = [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u];
                if let Some(t) = ray_thickness(o, [-no[0], -no[1]], &flat, stroke * 0.02) {
                    if best.map_or(true, |(bt, _)| t < bt) {
                        best = Some((t, o));
                    }
                }
            }
            let Some((t, o)) = best else { continue };
            let thin = 1.0 - smoothstep(1.5 * stroke, 2.3 * stroke, t);
            if thin <= 0.0 {
                continue;
            }
            // Bordas horizontais sobre linhas de referência (baseline, altura-x, caixa-alta) ficam
            // travadas: a borda oposta absorve a correção inteira, e os glifos continuam alinhados.
            // (só barras realmente retas e longas; apex de curvas não trava, senão aparece "dente")
            let tol_lock = lines.upem * 0.008;
            let on_anchor = |y: f32| lines.lines.iter().any(|l| (l - y).abs() <= tol_lock);
            let flat = (b[1] - a[1]).abs() <= 0.02 * len && len >= 0.6 * stroke;
            if flat && on_anchor(0.5 * (a[1] + b[1])) {
                continue;
            }
            let partner_locked = flat && on_anchor(o[1] - no[1] * t);
            // Espessura depois do mapa, medida ao longo da normal nova.
            let q = [o[0] - no[0] * t, o[1] - no[1] * t];
            let (po, pq) = ([o[0], warp.map(o[1])], [q[0], warp.map(q[1])]);
            let nn = nrm[i];
            let t_new = ((po[0] - pq[0]) * nn[0] + (po[1] - pq[1]) * nn[1]).abs();
            let share = if partner_locked { 1.0 } else { 0.5 };

            shift[i] = (share * (t_new - t) * params.compensation * thin).clamp(0.0, 0.9 * t);
        }
        smooth_shifts(&wpts, &mut shift, &internal, stroke);
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            let (e0, e1) = ((i + n - 1) % n, i);
            let (n1, n2, a, b) = (nrm[e0], nrm[e1], shift[e0], shift[e1]);
            let mut d = [0.0f32, 0.0];
            if a > 1e-5 || b > 1e-5 {
                if internal[e0] && !internal[e1] {
                    d = [-n2[0] * b, -n2[1] * b]; // só a aresta de borda manda
                } else if internal[e1] && !internal[e0] {
                    d = [-n1[0] * a, -n1[1] * a];
                } else if internal[e0] && internal[e1] {
                    // dentro da tinta: sem deslocamento
                } else {
                    let det = n1[0] * n2[1] - n1[1] * n2[0];
                    if det.abs() > 0.35 {
                        // Quina: interseção das duas arestas recuadas (miter), com teto.
                        d = [(-a * n2[1] + b * n1[1]) / det, (-b * n1[0] + a * n2[0]) / det];
                        let lim = 1.8 * a.max(b) + 1e-3;
                        let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
                        if l > lim {
                            d = [d[0] * lim / l, d[1] * lim / l];
                        }
                    } else {
                        // Trecho suave: recuo médio ao longo da bissetriz.
                        let sx = n1[0] + n2[0];
                        let sy = n1[1] + n2[1];
                        let sl = (sx * sx + sy * sy).sqrt().max(1e-4);
                        let k = 0.5 * (a + b);
                        d = [-sx / sl * k, -sy / sl * k];
                    }
                }
            }
            pts.push([wpts[i][0] + d[0], wpts[i][1] + d[1]]);
        }
        out.push(Contour { pts });
    }
    out
}

/// Suaviza o recuo por aresta ao longo de trechos *curvos* (sem quinas): a espessura medida por
/// raio oscila em junções (cintura do "8", bojo do "5") e isso viraria dentes no contorno. Quinas
/// (> 35°) delimitam os trechos, então barras retas mantêm o recuo exato.
fn smooth_shifts(wp: &[P], shift: &mut [f32], internal: &[bool], stroke: f32) {
    let n = shift.len();
    if n < 4 {
        return;
    }
    let len: Vec<f32> = (0..n)
        .map(|i| {
            let (a, b) = (wp[i], wp[(i + 1) % n]);
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt().max(1e-4)
        })
        .collect();
    // corner[i] = vértice i (entre as arestas i-1 e i) é quina
    let corner: Vec<bool> = (0..n)
        .map(|i| {
            let (p, q, r) = (wp[(i + n - 1) % n], wp[i], wp[(i + 1) % n]);
            let (ax, ay, bx, by) = (q[0] - p[0], q[1] - p[1], r[0] - q[0], r[1] - q[1]);
            let c = (ax * bx + ay * by) / ((ax * ax + ay * ay).sqrt() * (bx * bx + by * by).sqrt()).max(1e-6);
            c < 0.82 // ≈ 35°
        })
        .collect();
    let orig = shift.to_vec();
    let radius = 0.45 * stroke;
    for i in 0..n {
        if internal[i] || orig[i] <= 0.0 && !has_neighbor_shift(&orig, i, n) {
            continue;
        }
        let (mut acc, mut wsum) = (orig[i] * len[i], len[i]);
        // para frente
        let mut d = 0.5 * len[i];
        let mut j = i;
        loop {
            if corner[(j + 1) % n] || d > radius {
                break;
            }
            j = (j + 1) % n;
            if j == i || internal[j] {
                break;
            }
            let w = len[j] * (1.0 - d / radius).max(0.0);
            acc += orig[j] * w;
            wsum += w;
            d += len[j];
        }
        // para trás
        let mut d = 0.5 * len[i];
        let mut j = i;
        loop {
            if corner[j] || d > radius {
                break;
            }
            j = (j + n - 1) % n;
            if j == i || internal[j] {
                break;
            }
            let w = len[j] * (1.0 - d / radius).max(0.0);
            acc += orig[j] * w;
            wsum += w;
            d += len[j];
        }
        shift[i] = acc / wsum.max(1e-6);
    }
}

fn has_neighbor_shift(s: &[f32], i: usize, n: usize) -> bool {
    s[(i + 1) % n] > 0.0 || s[(i + n - 1) % n] > 0.0
}

fn flat_edges(contours: &[Contour]) -> Vec<(P, P)> {
    let mut v = Vec::new();
    for c in contours {
        let n = c.pts.len();
        for i in 0..n {
            v.push((c.pts[i], c.pts[(i + 1) % n]));
        }
    }
    v
}

/// Nonzero winding: o ponto está dentro da tinta (união dos contornos)?
fn ink_at(edges: &[(P, P)], p: P) -> bool {
    let mut w = 0i32;
    for &(a, b) in edges {
        if (a[1] <= p[1]) != (b[1] <= p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if x > p[0] {
                w += if b[1] > a[1] { 1 } else { -1 };
            }
        }
    }
    w != 0
}

/// Espessura da tinta a partir de `o` (já na borda) ao longo de `dir` (unitário, para dentro):
/// distância até onde a tinta realmente acaba. Contornos sobrepostos (variáveis, "2" = barra +
/// curva) não contam como borda: caminha pelos cruzamentos e testa a tinta entre eles.
fn ray_thickness(o: P, dir: [f32; 2], edges: &[(P, P)], eps: f32) -> Option<f32> {
    let org = [o[0] + dir[0] * eps, o[1] + dir[1] * eps];
    let mut hits: Vec<f32> = Vec::new();
    for &(a, b) in edges {
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let den = dir[0] * ey - dir[1] * ex;
        if den.abs() < 1e-9 {
            continue;
        }
        let (ox, oy) = (a[0] - org[0], a[1] - org[1]);
        let t = (ox * ey - oy * ex) / den;
        let u = (ox * dir[1] - oy * dir[0]) / den;
        if t > 1e-4 && (0.0..=1.0).contains(&u) {
            hits.push(t);
        }
    }
    hits.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    hits.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    let mut prev = 0.0f32;
    for (k, &t) in hits.iter().enumerate() {
        let next = hits.get(k + 1).copied().unwrap_or(t + 1.0);
        let mid = 0.5 * (t + next);
        let probe = [org[0] + dir[0] * mid, org[1] + dir[1] * mid];
        // depois deste cruzamento já não há tinta → a tinta acabou em `t`
        if !ink_at(edges, probe) {
            return Some(t + eps);
        }
        prev = t;
    }
    let _ = prev;
    None
}

/// Normal unitária da aresta `a→b` apontando PARA FORA da tinta.
fn out_normal(a: P, b: P, ink_left: bool) -> [f32; 2] {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = (dx * dx + dy * dy).sqrt().max(1e-6);
    let (dx, dy) = (dx / l, dy / l);
    // Normal à esquerda = (-dy, dx); à direita = (dy, -dx).
    if ink_left {
        [dy, -dx]
    } else {
        [-dy, dx]
    }
}

/// Atalho: analisa e aplica.
pub fn stretch_glyph(contours: &[Contour], lines: &FontLines, stretch: f32, params: &StemParams) -> Vec<Contour> {
    if stretch <= 1.0005 {
        return contours.to_vec();
    }
    let warp = analyze(contours, lines, stretch, params);
    warp_glyph(contours, &warp, lines, params)
}

/// Altura (unidades) do topo do glifo depois do alongamento, para dimensionar a caixa do texto.
pub fn bounds(contours: &[Contour]) -> Option<(f32, f32, f32, f32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for c in contours {
        for p in &c.pts {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
    }
    (x0 <= x1).then_some((x0, y0, x1, y1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f32 = 80.0; // haste
    const CAP: f32 = 700.0;

    fn lines() -> FontLines {
        FontLines::synthetic(1000.0, S, 500.0, CAP)
    }
    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Contour {
        // anti-horário (y para cima): tinta à esquerda
        Contour { pts: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]] }
    }
    fn width_at(cs: &[Contour], y: f32) -> f32 {
        scan_intervals(cs, y).iter().map(|i| i.x1 - i.x0).sum()
    }
    fn top(cs: &[Contour]) -> f32 {
        bounds(cs).unwrap().3
    }
    fn height_of_run_pub(cs: &[Contour], x: f32) -> f32 {
        height_of_run(cs, x)
    }
    fn height_of_run(cs: &[Contour], x: f32) -> f32 {
        // espessura vertical (soma de corridas verticais) numa coluna x
        let mut total = 0.0;
        let step = 0.5;
        let (_, y0, _, y1) = bounds(cs).unwrap();
        let mut y = y0;
        while y < y1 {
            if scan_intervals(cs, y).iter().any(|i| x >= i.x0 && x <= i.x1) {
                total += step;
            }
            y += step;
        }
        total
    }

    #[test]
    fn identity_when_stretch_is_one() {
        let g = vec![rect(0.0, 0.0, S, CAP)];
        assert_eq!(stretch_glyph(&g, &lines(), 1.0, &StemParams::default()), g);
    }

    #[test]
    fn stem_grows_by_stretch_and_keeps_width() {
        let g = vec![rect(0.0, 0.0, S, CAP)];
        let o = stretch_glyph(&g, &lines(), 2.0, &StemParams::default());
        assert!((top(&o) - 2.0 * CAP).abs() < 2.0, "topo {}", top(&o));
        assert!((width_at(&o, 500.0) - S).abs() < 0.5);
    }

    #[test]
    fn bar_keeps_thickness_and_height_aligns() {
        // "T": barra no topo (espessura S, largura 500) + haste central
        let g = vec![Contour {
            pts: vec![
                [0.0, CAP], [0.0, CAP - S], [210.0, CAP - S], [210.0, 0.0], [290.0, 0.0],
                [290.0, CAP - S], [500.0, CAP - S], [500.0, CAP],
            ],
        }];
        let o = stretch_glyph(&g, &lines(), 2.0, &StemParams::default());
        assert!((top(&o) - 2.0 * CAP).abs() < 3.0, "topo {}", top(&o));
        let bar = height_of_run(&o, 20.0);
        assert!((bar - S).abs() < 0.06 * S, "barra {bar} vs {S}");
        let stem = height_of_run(&o, 250.0);
        assert!(stem > 1.9 * CAP, "haste {stem}");
    }

    #[test]
    fn diagonal_keeps_perpendicular_thickness() {
        // Diagonal larga de ângulo ~ 50° em relação ao horizonte: paralelogramo.
        let h = 600.0;
        let dx = 420.0; // desloca o topo
        let wdt = 100.0; // largura horizontal
        let g = vec![Contour { pts: vec![[0.0, 0.0], [wdt, 0.0], [wdt + dx, h], [dx, h]] }];
        let perp = |cs: &[Contour], y: f32| {
            let iv = &scan_intervals(cs, y)[0];
            // espessura perpendicular = largura horizontal * sin(ângulo da borda com o eixo x)
            let slope = iv.s0.max(iv.s1); // |dx/dy|
            (iv.x1 - iv.x0) / (1.0 + slope * slope).sqrt()
        };
        let o = stretch_glyph(&g, &lines(), 2.0, &StemParams::default());
        let before = perp(&g, 300.0);
        let (_, y0, _, y1) = bounds(&o).unwrap();
        let after = perp(&o, (y0 + y1) * 0.5);
        assert!((after - before).abs() < 0.10 * before, "{after} vs {before}");
    }

    #[test]
    fn ring_ends_stay_round() {
        // "O": anel de raio externo R e espessura S. As pontas (topo/base) mantêm o raio.
        let r = 300.0;
        let cx = 300.0;
        let cy = 350.0;
        let n = 360;
        let circle = |rad: f32, ccw: bool| -> Contour {
            Contour {
                pts: (0..n)
                    .map(|i| {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU * if ccw { 1.0 } else { -1.0 };
                        [cx + rad * a.cos(), cy + rad * a.sin()]
                    })
                    .collect(),
            }
        };
        let g = vec![circle(r, true), circle(r - S, false)];
        let o = stretch_glyph(&g, &lines(), 2.0, &StemParams::default());
        let t = top(&o);
        assert!(t > cy + r + 0.5 * r, "o anel deveria crescer: {t}");
        // cap externo: até 0,3R abaixo do topo, sobre um círculo de raio R
        let mut worst = 0.0f32;
        for p in o[0].pts.iter().filter(|p| p[1] > t - 0.3 * r) {
            let d = ((p[0] - cx).powi(2) + (p[1] - (t - r)).powi(2)).sqrt();
            worst = worst.max((d - r).abs());
        }
        assert!(worst < 0.06 * r, "desvio do cap {worst} (R={r})");
        // largura (diâmetro) preservada
        let (x0, _, x1, _) = bounds(&o).unwrap();
        assert!(((x1 - x0) - 2.0 * r).abs() < 1.0);
        // espessura da parede no equador preservada
        let (_, y0, _, y1) = bounds(&o).unwrap();
        let eq = scan_intervals(&o, (y0 + y1) * 0.5);
        let wall = eq.iter().map(|i| i.x1 - i.x0).fold(f32::MAX, f32::min);
        assert!((wall - S).abs() < 2.0, "parede {wall}");
    }

    #[test]
    fn solid_disc_stays_a_disc() {
        let r = 200.0;
        let pts: Vec<P> = (0..180)
            .map(|i| {
                let a = i as f32 / 180.0 * std::f32::consts::TAU;
                [300.0 + r * a.cos(), 300.0 + r * a.sin()]
            })
            .collect();
        let g = vec![Contour { pts }];
        let o = stretch_glyph(&g, &lines(), 2.4, &StemParams::default());
        let (x0, y0, x1, y1) = bounds(&o).unwrap();
        assert!(((x1 - x0) - (y1 - y0)).abs() < 0.5, "disco virou elipse");
    }

    #[test]
    fn dot_is_translated_not_stretched() {
        let dot = Contour {
            pts: (0..48)
                .map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    [60.0 + 40.0 * a.cos(), 100.0 + 40.0 * a.sin()]
                })
                .collect(),
        };
        let o = stretch_glyph(&[dot.clone()], &lines(), 2.4, &StemParams::default());
        let (a0, b0, a1, b1) = bounds(&[dot]).unwrap();
        let (c0, d0, c1, d1) = bounds(&o).unwrap();
        assert!(((c1 - c0) - (a1 - a0)).abs() < 0.01);
        assert!(((d1 - d0) - (b1 - b0)).abs() < 0.01);
    }

    #[test]
    fn all_glyphs_reach_same_cap_height() {
        let i = vec![rect(0.0, 0.0, S, CAP)];
        let seven = vec![Contour { pts: vec![[0.0, CAP], [0.0, CAP - S], [400.0, CAP - S], [120.0, 0.0], [200.0, 0.0], [500.0, CAP - 0.0]] }];
        let p = StemParams::default();
        let ti = top(&stretch_glyph(&i, &lines(), 1.8, &p));
        let t7 = top(&stretch_glyph(&seven, &lines(), 1.8, &p));
        assert!((ti - t7).abs() < 3.0, "{ti} vs {t7}");
    }

    #[test]
    fn map_is_monotonic() {
        let g = vec![rect(0.0, 0.0, S, CAP), rect(0.0, 300.0, 400.0, 300.0 + S)];
        let w = analyze(&g, &lines(), 2.2, &StemParams::default());
        let mut prev = f32::MIN;
        let mut y = -50.0;
        while y < 800.0 {
            let v = w.map(y);
            assert!(v > prev);
            prev = v;
            y += 3.0;
        }
    }

    /// Ferramenta de inspeção: `STEM_FONT=arq.ttf STEM_TEXT=0812:45 STEM_S=2 STEM_OUT=out.svg cargo test -p prism-core render_svg -- --ignored`
    #[test]
    #[ignore]
    fn render_svg() {
        let Ok(path) = std::env::var("STEM_FONT") else { return };
        let text = std::env::var("STEM_TEXT").unwrap_or_else(|_| "08:29 HEBA".into());
        let sv: f32 = std::env::var("STEM_S").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        let out = std::env::var("STEM_OUT").unwrap_or_else(|_| "/tmp/vstem.svg".into());
        let data = std::fs::read(path).unwrap();
        let face = Face::parse(&data, 0).unwrap();
        let lines = FontLines::measure(&face);
        eprintln!("upem {} stroke {} lines {:?} slant {}", lines.upem, lines.stroke, lines.lines, lines.slant);
        let params = StemParams::default();
        let k: f32 = std::env::var("STEM_K").ok().and_then(|v| v.parse().ok()).unwrap_or(0.12);
        let mut svg = String::new();
        let rows = [("orig", 0), ("naive", 1), ("stem", 2)];
        let line_h = lines.upem * 1.1 * k * sv;
        let mut width = 0.0f32;
        for (ri, (_, mode)) in rows.iter().enumerate() {
            let mut x = 20.0f32;
            let base = 40.0 + (ri as f32) * (line_h + 30.0) + line_h * 0.8;
            for ch in text.chars() {
                let gid = face.glyph_index(ch).unwrap_or_default();
                let adv = face.glyph_hor_advance(gid).unwrap_or(500) as f32 * k;
                if let Some(cs) = glyph_contours(&face, gid, lines.upem * 0.0015) {
                    if *mode == 2 && std::env::var("STEM_DEBUG").is_ok() {
                        let (bx0, _, bx1, _) = bounds(&cs).unwrap();
                        let xs = bx0 + 0.85 * (bx1 - bx0);
                        let before = height_of_run_pub(&cs, xs);
                        let after = height_of_run_pub(&stretch_glyph(&cs, &lines, sv, &params), xs);
                        eprintln!("glyph {ch}: coluna {xs:.0}: espessura vertical {before:.0} -> {after:.0}");
                    }
                    let cs2: Vec<Contour> = match mode {
                        0 => cs,
                        1 => cs.iter().map(|c| Contour { pts: c.pts.iter().map(|p| [p[0], p[1] * sv]).collect() }).collect(),
                        _ => stretch_glyph(&cs, &lines, sv, &params),
                    };
                    let mut d = String::new();
                    for c in &cs2 {
                        for (i, p) in c.pts.iter().enumerate() {
                            d.push_str(&format!("{}{:.2},{:.2} ", if i == 0 { "M" } else { "L" }, x + p[0] * k, base - p[1] * k));
                        }
                        d.push_str("Z ");
                    }
                    svg.push_str(&format!("<path d=\"{}\" fill=\"#fff\" fill-rule=\"nonzero\"/>\n", d));
                }
                x += adv;
            }
            width = width.max(x);
        }
        let h = 60.0 + 3.0 * (line_h + 30.0);
        std::fs::write(out, format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\"><rect width=\"100%\" height=\"100%\" fill=\"#111\"/>{}</svg>", width + 40.0, h, svg)).unwrap();
    }
}
