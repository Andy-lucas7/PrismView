//! Texto com alongamento por haste (`prism_core::vstem`) rasterizado na CPU.
//!
//! O layout (shaping, kerning, quebra, alinhamento) continua sendo o do cosmic-text; aqui só se
//! troca a *rasterização*: cada glifo vira contorno vetorial, o mapa vertical por glifo estica
//! as hastes (barras, diagonais e curvas mantêm espessura e raio) e o resultado é preenchido com
//! antialiasing (tiny-skia, 2× supersample quando cabe) numa textura branca pré-multiplicada,
//! igual à que o glyphon produziria. Roda só quando texto/tamanho/estiramento mudam.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{fontdb, Buffer, FontSystem};
use prism_core::ttf_parser::{Face, Tag};
use prism_core::vstem::{self, Contour, FontLines, StemParams};
use resvg::tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

/// Origem de glifos: um `Buffer` do cosmic-text deslocado em `(ox, oy)` px dentro da caixa de texto.
pub struct GlyphSource<'a> {
    pub buffer: &'a Buffer,
    pub ox: f32,
    pub oy: f32,
}

/// Cache de métricas por face/peso (a análise por glifo é refeita só quando o texto muda).
#[derive(Default)]
pub struct StemCache {
    lines: HashMap<(fontdb::ID, u16), Arc<FontLines>>,
}

/// Textura pronta (RGBA8 pré-multiplicado, branco).
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Placed {
    font_id: fontdb::ID,
    glyph_id: u16,
    size: f32,
    x: f32,
    base_y: f32,
}

/// Limite de pixels do buffer supersampled (≈ 3 Mpx): acima disso rasteriza em 1×.
const SUPERSAMPLE_MAX_PIXELS: u64 = 3_000_000;

/// Rasteriza `sources` com `stretch` (≥ 1). `width`/`height` = tamanho da textura útil (px, já com
/// `pad` e com a altura esticada). Devolve `None` se nada pôde ser desenhado (o chamador cai no
/// caminho comum do glyphon).
#[allow(clippy::too_many_arguments)]
pub fn rasterize(
    fs: &mut FontSystem,
    cache: &mut StemCache,
    sources: &[GlyphSource<'_>],
    stretch: f32,
    compensation: f32,
    weight: u16,
    width: u32,
    height: u32,
    pad: f32,
) -> Option<Raster> {
    if width == 0 || height == 0 {
        return None;
    }
    // 1. Posição de cada glifo (mesma conta de `LayoutGlyph::physical`, sem arredondar).
    let mut placed: Vec<Placed> = Vec::new();
    for src in sources {
        for run in src.buffer.layout_runs() {
            for g in run.glyphs.iter() {
                placed.push(Placed {
                    font_id: g.font_id,
                    glyph_id: g.glyph_id,
                    size: g.font_size,
                    x: src.ox + g.x + g.font_size * g.x_offset,
                    base_y: src.oy + run.line_y + g.y - g.font_size * g.y_offset,
                });
            }
        }
    }
    if placed.is_empty() {
        return None;
    }

    let params = StemParams { compensation: compensation.clamp(0.0, 1.5), ..StemParams::default() };

    // 2. Supersample.
    let ss: u32 = if u64::from(width) * u64::from(height) * 4 <= SUPERSAMPLE_MAX_PIXELS { 2 } else { 1 };
    let (pw, ph) = (width * ss, height * ss);
    let mut pixmap = Pixmap::new(pw, ph)?;

    // 3. Contornos por face (um `with_face_data` por face).
    let mut by_font: HashMap<fontdb::ID, Vec<usize>> = HashMap::new();
    for (i, p) in placed.iter().enumerate() {
        by_font.entry(p.font_id).or_default().push(i);
    }
    let mut pb = PathBuilder::new();
    let mut any = false;
    for (font_id, idxs) in by_font {
        let lines_cache = &mut cache.lines;
        let drawn = fs
            .db()
            .with_face_data(font_id, |data, index| {
                let mut face = Face::parse(data, index).ok()?;
                if face.is_variable() {
                    let _ = face.set_variation(Tag::from_bytes(b"wght"), f32::from(weight));
                }
                let lines = lines_cache
                    .entry((font_id, weight))
                    .or_insert_with(|| Arc::new(FontLines::measure(&face)))
                    .clone();
                let upem = lines.upem.max(1.0);
                let mut glyph_memo: HashMap<(u16, u32), Vec<Contour>> = HashMap::new();
                let mut any_here = false;
                for &i in &idxs {
                    let p = &placed[i];
                    let k = p.size / upem;
                    let key = (p.glyph_id, p.size.to_bits());
                    let cs = glyph_memo.entry(key).or_insert_with(|| {
                        // tolerância de 0,05 px na tela, em unidades da fonte
                        let tol = 0.08 / (k.max(1e-4) * ss as f32);
                        vstem::glyph_contours(&face, prism_core::ttf_parser::GlyphId(p.glyph_id), tol)
                            .map(|c| vstem::stretch_glyph(&c, &lines, stretch, &params))
                            .unwrap_or_default()
                    });
                    for c in cs.iter() {
                        let mut first = true;
                        for pt in &c.pts {
                            let x = (p.x + pt[0] * k + pad) * ss as f32;
                            let y = (p.base_y * stretch - pt[1] * k + pad) * ss as f32;
                            if first {
                                pb.move_to(x, y);
                                first = false;
                            } else {
                                pb.line_to(x, y);
                            }
                        }
                        if !first {
                            pb.close();
                            any_here = true;
                        }
                    }
                }
                Some(any_here)
            })
            .flatten()
            .unwrap_or(false);
        any |= drawn;
    }
    if !any {
        return None;
    }
    let path = pb.finish()?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);

    // 4. Redução (caixa 2×2 sobre RGBA pré-multiplicado).
    let rgba = if ss == 1 {
        pixmap.take()
    } else {
        let src = pixmap.data();
        let mut out = vec![0u8; (width * height * 4) as usize];
        let row = (pw * 4) as usize;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let i0 = (2 * y) * row + (2 * x) * 4;
                let i1 = i0 + row;
                for c in 0..4 {
                    let sum = u32::from(src[i0 + c])
                        + u32::from(src[i0 + 4 + c])
                        + u32::from(src[i1 + c])
                        + u32::from(src[i1 + 4 + c]);
                    out[(y * width as usize + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        out
    };
    Some(Raster { width, height, rgba })
}
