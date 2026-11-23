//! SVG/ícones: rasterização por resvg com cache por markup e tamanho.

#[allow(unused_imports)]
use super::*;

// ───────────────────────── SVG (resvg) ─────────────────────────

/// Máximo de rasters de SVG mantidos (markup × tamanho); acima disso o cache é esvaziado.
pub(super) const SVG_CACHE_MAX: usize = 96;
/// Lado máximo do raster de um SVG (px físicos).
pub(super) const SVG_MAX_PX: u32 = 2048;

#[derive(Default)]
pub(super) struct SvgCache {
    pub(super) loaded: HashMap<(u64, u32, u32), ImageEntry>,
    pub(super) failed: HashSet<u64>,
}

impl SvgCache {
    pub(super) fn get(
        &mut self,
        markup: &str,
        w: u32,
        h: u32,
        compositor: &mut Compositor2D,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<ImageEntry> {
        let hash = hash_str(markup);
        if self.failed.contains(&hash) {
            return None;
        }
        if let Some(entry) = self.loaded.get(&(hash, w, h)) {
            return Some(*entry);
        }
        if self.loaded.len() >= SVG_CACHE_MAX {
            self.release_all(compositor);
        }
        let Some(rgba) = rasterize_svg(markup, w, h) else {
            self.failed.insert(hash);
            return None;
        };
        let id = compositor.create_texture_rgba(device, queue, w, h, &rgba)?;
        let entry = ImageEntry { id, w, h };
        self.loaded.insert((hash, w, h), entry);
        Some(entry)
    }

    pub(super) fn release_all(&mut self, compositor: &mut Compositor2D) {
        for (_, entry) in self.loaded.drain() {
            compositor.release_texture(entry.id);
        }
        self.failed.clear();
    }
}

/// Rasteriza um documento SVG em `w×h` (contain, centralizado). Saída RGBA8 pré-multiplicada.
pub(super) fn rasterize_svg(markup: &str, w: u32, h: u32) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(markup, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let (sw, sh) = (size.width().max(1e-3), size.height().max(1e-3));
    let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    let k = (w as f32 / sw).min(h as f32 / sh);
    let tx = (w as f32 - sw * k) * 0.5;
    let ty = (h as f32 - sh * k) * 0.5;
    resvg::render(&tree, tiny_skia::Transform::from_row(k, 0.0, 0.0, k, tx, ty), &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// Tamanho de raster em degraus de 4 px (evita refazer o raster por variações sub-pixel).
pub(super) fn svg_bucket(px: f32) -> u32 {
    let v = if px.is_finite() { px.ceil().max(4.0) as u32 } else { 4 };
    (v.div_ceil(4) * 4).min(SVG_MAX_PX)
}
