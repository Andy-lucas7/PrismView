//! Layout + alongamento vetorial de uma caixa de texto, sem DOM e sem GPU.
//!
//! Espelha o caminho do motor nativo (`prismview-engine::core::{text, stem_text}`): shaping com
//! rustybuzz (o mesmo do cosmic-text), quebra só entre palavras com a mesma tolerância, linha de
//! 1,06 × corpo, baseline pelo ascendente/descendente `hhea`, alinhamento dentro da caixa e a
//! posição vertical (linha e baseline) multiplicada pelo alongamento. O resultado são comandos de
//! caminho (`0 = M x y`, `1 = L x y`, `2 = Z`) em px CSS, y para baixo, prontos para um `Path2D`.

use std::collections::HashMap;

use rustybuzz::{Face as BuzzFace, UnicodeBuffer, Variation};
use ttf_parser::{Face, GlyphId, Tag};

use crate::vstem::{self, Contour, FontLines, StemParams};

pub const OP_MOVE: f32 = 0.0;
pub const OP_LINE: f32 = 1.0;
pub const OP_CLOSE: f32 = 2.0;

/// Entrelinha (paridade com `UnifiedCompositor.tsx` e `core/text.rs`).
pub const LINE_HEIGHT: f32 = 1.06;
/// Folga de quebra (igual a `WRAP_TOLERANCE_*` de `core/text.rs`).
const WRAP_TOLERANCE_REL: f32 = 0.03;
const WRAP_TOLERANCE_EM: f32 = 0.08;
/// Limites de entrada.
const MAX_CHARS: usize = 8192;
const MIN_SIZE: f32 = 1.0;
const MAX_SIZE: f32 = 4096.0;
/// Contornos guardados por fonte antes de recomeçar o cache.
const GLYPH_CACHE_LIMIT: usize = 6000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

impl Align {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Align::Center,
            2 => Align::End,
            _ => Align::Start,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params<'a> {
    pub text: &'a str,
    /// Corpo em px CSS.
    pub size: f32,
    /// Peso para fontes variáveis (eixo `wght`); ignorado em fontes estáticas.
    pub weight: f32,
    pub letter_spacing_px: f32,
    pub align: Align,
    /// 0 = largura automática (uma linha por parágrafo).
    pub max_width: f32,
    pub stretch: f32,
    pub compensation: f32,
    /// Densidade de pixels da tela: define a tolerância ao achatar curvas.
    pub device_scale: f32,
}

#[derive(Debug, Default)]
pub struct Layout {
    /// Caixa de layout (sem margem), px CSS. A altura já inclui o alongamento.
    pub width: f32,
    pub height: f32,
    pub lines: usize,
    /// `[op, x, y?, ...]` (ver `OP_*`).
    pub cmds: Vec<f32>,
}

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
struct GlyphKey {
    gid: u16,
    weight: u32,
    stretch: u32,
    comp: u32,
    tol_bucket: i32,
}

/// Uma fonte registrada: bytes + caches por peso.
pub struct Font {
    data: Vec<u8>,
    index: u32,
    lines: HashMap<u32, FontLines>,
    glyphs: HashMap<GlyphKey, Vec<Contour>>,
}

impl Font {
    /// `None` se os bytes não são uma fonte TrueType/OpenType legível.
    pub fn new(data: Vec<u8>, index: u32) -> Option<Self> {
        Face::parse(&data, index).ok()?;
        BuzzFace::from_slice(&data, index)?;
        Some(Self { data, index, lines: HashMap::new(), glyphs: HashMap::new() })
    }

    pub fn clear_caches(&mut self) {
        self.lines.clear();
        self.glyphs.clear();
    }
}

struct Glyph {
    gid: u16,
    /// Posição do glifo na linha (px), já com `x_offset`.
    x: f32,
    /// Deslocamento vertical (px, y para cima) do shaping.
    y_off: f32,
}

struct Shaped {
    glyphs: Vec<Glyph>,
    /// Largura da linha (px), contando o espaçamento também depois do último glifo (como o CSS).
    width: f32,
}

fn shape_line(buzz: &BuzzFace<'_>, text: &str, k: f32, ls: f32) -> Shaped {
    if text.is_empty() {
        return Shaped { glyphs: Vec::new(), width: 0.0 };
    }
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    buf.guess_segment_properties();
    let out = rustybuzz::shape(buzz, &[], buf);
    let mut pen = 0.0f32;
    let mut glyphs = Vec::with_capacity(out.len());
    for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
        glyphs.push(Glyph {
            gid: info.glyph_id as u16,
            x: pen + pos.x_offset as f32 * k,
            y_off: pos.y_offset as f32 * k,
        });
        pen += pos.x_advance as f32 * k + ls;
    }
    Shaped { glyphs, width: pen.max(0.0) }
}

fn wrap_tolerance(limit: f32, size: f32) -> f32 {
    (limit * WRAP_TOLERANCE_REL).max(size * WRAP_TOLERANCE_EM)
}

/// Quebra só em espaços; palavra que não cabe sozinha fica inteira (a caixa se estende).
fn wrap_lines(text: &str, limit: f32, size: f32, measure: &mut dyn FnMut(&str) -> f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        if limit <= 0.0 || para.is_empty() {
            out.push(para.trim_end().to_string());
            continue;
        }
        let fit = limit + wrap_tolerance(limit, size);
        let mut current = String::new();
        for word in para.split_inclusive(char::is_whitespace) {
            if current.is_empty() {
                current.push_str(word);
                continue;
            }
            let mut cand = String::with_capacity(current.len() + word.len());
            cand.push_str(&current);
            cand.push_str(word.trim_end());
            if measure(&cand) <= fit {
                current.push_str(word);
            } else {
                out.push(current.trim_end().to_string());
                current.clear();
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            out.push(current.trim_end().to_string());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Faz o layout e o alongamento. `None` se a fonte não pôde ser lida.
pub fn layout(font: &mut Font, p: &Params<'_>) -> Option<Layout> {
    let size = if p.size.is_finite() { p.size.clamp(MIN_SIZE, MAX_SIZE) } else { MIN_SIZE };
    let stretch = if p.stretch.is_finite() { p.stretch.clamp(1.0, 2.8) } else { 1.0 };
    let ls = if p.letter_spacing_px.is_finite() { p.letter_spacing_px.clamp(-0.5 * size, 2.0 * size) } else { 0.0 };
    let device = if p.device_scale.is_finite() { p.device_scale.clamp(0.5, 4.0) } else { 1.0 };
    let text: String = p.text.chars().take(MAX_CHARS).collect();

    let mut face = Face::parse(&font.data, font.index).ok()?;
    let mut buzz = BuzzFace::from_slice(&font.data, font.index)?;
    let weight = if p.weight.is_finite() { p.weight.clamp(1.0, 1000.0) } else { 400.0 };
    if face.is_variable() {
        let _ = face.set_variation(Tag::from_bytes(b"wght"), weight);
        buzz.set_variations(&[Variation { tag: rustybuzz::ttf_parser::Tag::from_bytes(b"wght"), value: weight }]);
    }
    let upem = f32::from(face.units_per_em().max(1));
    let k = size / upem;
    let (asc, desc) = (f32::from(face.ascender()) * k, -f32::from(face.descender()) * k);

    let line_h = size * LINE_HEIGHT;
    let wrap_limit = if p.max_width.is_finite() && p.max_width >= 1.0 { p.max_width } else { 0.0 };
    let lines = wrap_lines(&text, wrap_limit, size, &mut |s| shape_line(&buzz, s, k, ls).width);
    let shaped: Vec<Shaped> = lines.iter().map(|l| shape_line(&buzz, l, k, ls)).collect();

    let widest = shaped.iter().map(|s| s.width).fold(0.0f32, f32::max);
    let inner_w = if wrap_limit > 0.0 { wrap_limit.max(widest) } else { widest };
    let height = lines.len() as f32 * line_h * stretch;
    let mut out = Layout { width: inner_w, height, lines: lines.len(), cmds: Vec::new() };
    if stretch <= 1.0005 {
        // Sem alongamento o chamador usa o texto do navegador; devolvemos só a caixa.
        return Some(out);
    }

    let fl = font.lines.entry(weight.to_bits()).or_insert_with(|| FontLines::measure(&face)).clone();
    let params = StemParams { compensation: p.compensation.clamp(0.0, 1.5), ..StemParams::default() };
    // Achata a ~0,06 px de tela; agrupa em potências de 2 para o cache render por render.
    let tol_units = (0.06 / (k * device).max(1e-4)).max(0.01);
    let tol_bucket = tol_units.log2().round() as i32;
    let tol = 2f32.powi(tol_bucket);
    if font.glyphs.len() > GLYPH_CACHE_LIMIT {
        font.glyphs.clear();
    }

    let baseline_off = (line_h - (asc + desc)) * 0.5 + asc;
    for (li, line) in shaped.iter().enumerate() {
        let x0 = match p.align {
            Align::Start => 0.0,
            Align::Center => (inner_w - line.width) * 0.5,
            Align::End => inner_w - line.width,
        };
        let base = li as f32 * line_h + baseline_off;
        for g in &line.glyphs {
            let key = GlyphKey {
                gid: g.gid,
                weight: weight.to_bits(),
                stretch: stretch.to_bits(),
                comp: params.compensation.to_bits(),
                tol_bucket,
            };
            if !font.glyphs.contains_key(&key) {
                let cs = vstem::glyph_contours(&face, GlyphId(g.gid), tol)
                    .map(|c| vstem::stretch_glyph(&c, &fl, stretch, &params))
                    .unwrap_or_default();
                font.glyphs.insert(key, cs);
            }
            let Some(cs) = font.glyphs.get(&key) else { continue };
            for c in cs {
                let mut first = true;
                for pt in &c.pts {
                    let x = x0 + g.x + pt[0] * k;
                    let y = (base - g.y_off) * stretch - pt[1] * k;
                    out.cmds.extend_from_slice(&[if first { OP_MOVE } else { OP_LINE }, x, y]);
                    first = false;
                }
                if !first {
                    out.cmds.push(OP_CLOSE);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTFIT: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/prismview-studio/src-tauri/fonts/Outfit-400.ttf"));

    fn params(text: &str) -> Params<'_> {
        Params {
            text,
            size: 100.0,
            weight: 400.0,
            letter_spacing_px: 0.0,
            align: Align::Start,
            max_width: 0.0,
            stretch: 2.0,
            compensation: 1.0,
            device_scale: 1.0,
        }
    }

    fn bounds(cmds: &[f32]) -> (f32, f32, f32, f32) {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut i = 0;
        while i < cmds.len() {
            if cmds[i] == OP_CLOSE {
                i += 1;
                continue;
            }
            let (x, y) = (cmds[i + 1], cmds[i + 2]);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
            i += 3;
        }
        (x0, y0, x1, y1)
    }

    #[test]
    fn rejects_garbage_font_bytes() {
        assert!(Font::new(vec![0, 1, 2, 3], 0).is_none());
    }

    #[test]
    fn stretched_text_is_taller_by_the_stretch_and_keeps_width() {
        let mut font = Font::new(OUTFIT.to_vec(), 0).unwrap();
        let plain = layout(&mut font, &Params { stretch: 1.0, ..params("08:29") }).unwrap();
        let tall = layout(&mut font, &params("08:29")).unwrap();
        assert!((tall.width - plain.width).abs() < 0.01, "largura {} vs {}", tall.width, plain.width);
        assert!((tall.height - 2.0 * plain.height).abs() < 0.01);
        assert!(plain.cmds.is_empty(), "sem alongamento só devolve a caixa");
        assert!(!tall.cmds.is_empty());
        // O topo dos algarismos chega ~2× mais alto que a altura de caixa-alta sem alongar.
        let (_, y0, _, y1) = bounds(&tall.cmds);
        let ink_h = y1 - y0;
        assert!(ink_h > 1.7 * 70.0 && ink_h < 2.3 * 74.0, "altura da tinta {ink_h}");
    }

    #[test]
    fn narrow_box_never_splits_a_word_and_extends() {
        let mut font = Font::new(OUTFIT.to_vec(), 0).unwrap();
        let natural = layout(&mut font, &params("00:06")).unwrap().width;
        let l = layout(&mut font, &Params { max_width: natural * 0.4, ..params("00:06") }).unwrap();
        assert_eq!(l.lines, 1);
        assert!((l.width - natural).abs() < 0.01, "a caixa estende até a palavra: {} vs {}", l.width, natural);
    }

    #[test]
    fn wraps_between_words_with_tolerance() {
        let mut font = Font::new(OUTFIT.to_vec(), 0).unwrap();
        let one = layout(&mut font, &params("Solus lesus salvat")).unwrap();
        let narrow = layout(&mut font, &Params { max_width: one.width * 0.6, ..params("Solus lesus salvat") }).unwrap();
        assert!(narrow.lines >= 2, "{} linhas", narrow.lines);
        // Folga: uma caixa 1% menor que o texto ainda cabe numa linha.
        let tight = layout(&mut font, &Params { max_width: one.width * 0.99, ..params("Solus lesus salvat") }).unwrap();
        assert_eq!(tight.lines, 1);
    }

    #[test]
    fn explicit_newlines_and_alignment_inside_the_box() {
        let mut font = Font::new(OUTFIT.to_vec(), 0).unwrap();
        assert_eq!(layout(&mut font, &params("a\nbbbbbbbb")).unwrap().lines, 2);
        let left_edge = |font: &mut Font, align| {
            let l = layout(font, &Params { align, max_width: 300.0, ..params("a") }).unwrap();
            assert_eq!(l.width, 300.0);
            bounds(&l.cmds).0
        };
        let (s, c, e) = (left_edge(&mut font, Align::Start), left_edge(&mut font, Align::Center), left_edge(&mut font, Align::End));
        assert!(s < c && c < e, "{s} {c} {e}");
        let right = layout(&mut font, &Params { align: Align::End, max_width: 300.0, ..params("a") }).unwrap();
        assert!(bounds(&right.cmds).2 <= 300.5, "texto passou da caixa");
    }

    #[test]
    fn letter_spacing_widens_the_box_including_trailing() {
        let mut font = Font::new(OUTFIT.to_vec(), 0).unwrap();
        let a = layout(&mut font, &Params { stretch: 1.0, ..params("abc") }).unwrap().width;
        let b = layout(&mut font, &Params { stretch: 1.0, letter_spacing_px: 10.0, ..params("abc") }).unwrap().width;
        assert!((b - a - 30.0).abs() < 0.01, "{b} - {a}");
    }

    #[test]
    fn hostile_inputs_do_not_panic() {
        let mut font = Font::new(OUTFIT.to_vec(), 0).unwrap();
        for p in [
            Params { size: f32::NAN, ..params("x") },
            Params { stretch: f32::INFINITY, ..params("x") },
            Params { max_width: f32::NAN, ..params("x y") },
            Params { letter_spacing_px: f32::NEG_INFINITY, ..params("x") },
            params(""),
            params("\n\n"),
            params("\u{0000}\u{FFFF}𝔘"),
        ] {
            let _ = layout(&mut font, &p);
        }
    }
}
