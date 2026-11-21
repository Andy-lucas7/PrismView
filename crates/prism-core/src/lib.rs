pub mod fx;
pub mod package;
pub mod schema;

pub mod stem_stretch {
    use crate::schema::StemStretchParams;
    use lyon_path::math::point;
    use lyon_path::Path as LyonPath;
    use lyon_tessellation::{
        BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
    };
    use ttf_parser::{Face, OutlineBuilder};

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct GlyphVertex {
        pub position: [f32; 2],
        pub uv: [f32; 2],
    }

    #[derive(Debug, Clone)]
    pub struct StretchedMesh {
        pub vertices: Vec<GlyphVertex>,
        pub indices: Vec<u32>,
        pub width: f32,
        pub height: f32,
        pub baseline_y: f32,
    }

    struct ContourCollector {
        contours: Vec<Vec<[f32; 2]>>,
        current: Vec<[f32; 2]>,
        last: [f32; 2],
        scale: f32,
        offset_x: f32,
    }

    impl ContourCollector {
        fn new(scale: f32, offset_x: f32) -> Self {
            Self {
                contours: Vec::new(),
                current: Vec::new(),
                last: [0.0, 0.0],
                scale,
                offset_x,
            }
        }

        fn map_pt(&self, x: f32, y: f32) -> [f32; 2] {
            [self.offset_x + x * self.scale, -y * self.scale]
        }

        fn push_pt(&mut self, p: [f32; 2]) {
            if self
                .current
                .last()
                .map(|prev| (prev[0] - p[0]).hypot(prev[1] - p[1]) > 0.25)
                .unwrap_or(true)
            {
                self.current.push(p);
            }
            self.last = p;
        }
    }

    impl OutlineBuilder for ContourCollector {
        fn move_to(&mut self, x: f32, y: f32) {
            if self.current.len() >= 3 {
                self.contours.push(std::mem::take(&mut self.current));
            } else {
                self.current.clear();
            }
            let p = self.map_pt(x, y);
            self.push_pt(p);
        }

        fn line_to(&mut self, x: f32, y: f32) {
            let p = self.map_pt(x, y);
            self.push_pt(p);
        }

        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            let p0 = self.last;
            let p1 = self.map_pt(x1, y1);
            let p2 = self.map_pt(x, y);
            let approx_len = (p1[0] - p0[0]).hypot(p1[1] - p0[1]) + (p2[0] - p1[0]).hypot(p2[1] - p1[1]);
            let steps = (approx_len / 2.0).ceil().clamp(4.0, 32.0) as usize;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let u = 1.0 - t;
                let px = u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0];
                let py = u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1];
                self.push_pt([px, py]);
            }
        }

        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            let p0 = self.last;
            let p1 = self.map_pt(x1, y1);
            let p2 = self.map_pt(x2, y2);
            let p3 = self.map_pt(x, y);
            let approx_len = (p1[0] - p0[0]).hypot(p1[1] - p0[1])
                + (p2[0] - p1[0]).hypot(p2[1] - p1[1])
                + (p3[0] - p2[0]).hypot(p3[1] - p2[1]);
            let steps = (approx_len / 2.0).ceil().clamp(6.0, 48.0) as usize;
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let u = 1.0 - t;
                let px = u * u * u * p0[0]
                    + 3.0 * u * u * t * p1[0]
                    + 3.0 * u * t * t * p2[0]
                    + t * t * t * p3[0];
                let py = u * u * u * p0[1]
                    + 3.0 * u * u * t * p1[1]
                    + 3.0 * u * t * t * p2[1]
                    + t * t * t * p3[1];
                self.push_pt([px, py]);
            }
        }

        fn close(&mut self) {
            if self.current.len() >= 3 {
                self.contours.push(std::mem::take(&mut self.current));
            } else {
                self.current.clear();
            }
        }
    }

    pub fn build_stretched_mesh(
        font_bytes: &[u8],
        text: &str,
        font_size_px: f32,
        letter_spacing_em: f32,
        stretch_factor: f32,
        params: &StemStretchParams,
    ) -> Option<StretchedMesh> {
        let face = Face::parse(font_bytes, 0).ok()?;
        let upem = f32::from(face.units_per_em().max(1));
        let scale = font_size_px / upem;
        let tracking_px = letter_spacing_em * font_size_px;
        let stretch = stretch_factor.clamp(1.0, 3.2);
        let alpha = params.curve_alpha.clamp(0.12, 0.75);
        let stroke_comp = params.stroke_compensation.clamp(0.0, 1.25);

        let mut all_contours: Vec<(Vec<[f32; 2]>, bool)> = Vec::new();
        let mut pen_x = 0.0f32;
        let mut prev_glyph = None;

        for ch in text.chars() {
            let Some(gid) = face.glyph_index(ch) else {
                pen_x += font_size_px * 0.35;
                prev_glyph = None;
                continue;
            };
            if let Some(prev) = prev_glyph {
                if let Some(kern) = face.tables().kern.and_then(|k| {
                    k.subtables
                        .into_iter()
                        .find_map(|st| st.glyphs_kerning(prev, gid))
                }) {
                    pen_x += f32::from(kern) * scale;
                }
            }

            let is_punct = matches!(ch, ':' | '.' | ',' | ';' | '·' | '-');
            let mut collector = ContourCollector::new(scale, pen_x);
            face.outline_glyph(gid, &mut collector);
            for contour in collector.contours {
                all_contours.push((contour, is_punct));
            }

            let advance = face
                .glyph_hor_advance(gid)
                .map(|a| f32::from(a) * scale)
                .unwrap_or(font_size_px * 0.5);
            pen_x += advance + tracking_px;
            prev_glyph = Some(gid);
        }

        if all_contours.is_empty() {
            return Some(StretchedMesh {
                vertices: Vec::new(),
                indices: Vec::new(),
                width: pen_x.max(0.0),
                height: font_size_px * stretch,
                baseline_y: font_size_px * 0.8 * stretch,
            });
        }

        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (c, _) in &all_contours {
            for p in c {
                min_x = min_x.min(p[0]);
                max_x = max_x.max(p[0]);
                min_y = min_y.min(p[1]);
                max_y = max_y.max(p[1]);
            }
        }
        let raw_h = (max_y - min_y).max(1.0);
        let base_stroke_w = (font_size_px * 0.11).max(2.0);
        let max_comp_shift = base_stroke_w * 0.38;

        let mut path_builder = LyonPath::builder();
        let mut out_min_x = f32::INFINITY;
        let mut out_max_x = f32::NEG_INFINITY;
        let mut out_min_y = f32::INFINITY;
        let mut out_max_y = f32::NEG_INFINITY;

        let mut transformed_contours = Vec::with_capacity(all_contours.len());
        for (contour, is_punct) in all_contours {
            let n = contour.len();
            if n < 3 {
                continue;
            }
            let signed_area: f32 = (0..n)
                .map(|i| {
                    let a = contour[i];
                    let b = contour[(i + 1) % n];
                    a[0] * b[1] - b[0] * a[1]
                })
                .sum::<f32>()
                * 0.5;
            let orient_sign = if signed_area >= 0.0 { 1.0f32 } else { -1.0f32 };

            let mut out_pts = Vec::with_capacity(n);
            for i in 0..n {
                let prev = contour[(i + n - 1) % n];
                let curr = contour[i];
                let next = contour[(i + 1) % n];

                let u = ((curr[1] - min_y) / raw_h).clamp(0.0, 1.0);
                let y_warped = if is_punct {
                    let center = (min_y + max_y) * 0.5;
                    let warped_center = min_y + warp_c_inf(0.5, alpha, stretch) * raw_h;
                    warped_center + (curr[1] - center)
                } else {
                    min_y + warp_c_inf(u, alpha, stretch) * raw_h
                };

                let tx = next[0] - prev[0];
                let ty = next[1] - prev[1];
                let tlen = tx.hypot(ty).max(1e-4);
                let nx = orient_sign * (ty / tlen);
                let ny = orient_sign * (-tx / tlen);

                let local_slope = warp_c_inf_deriv(u, alpha, stretch);
                let arch_factor = (1.0 - (2.0 * u - 1.0).powi(2)).clamp(0.0, 1.0);
                let comp_y = if is_punct || (stretch - 1.0).abs() < 1e-3 {
                    0.0
                } else {
                    let raw_shift = -ny
                        * ny.abs()
                        * (local_slope - 1.0).max(0.0)
                        * base_stroke_w
                        * 0.28
                        * stroke_comp
                        * (1.0 - 0.35 * arch_factor);
                    raw_shift.clamp(-max_comp_shift, max_comp_shift)
                };

                let x_final = curr[0];
                let y_final = y_warped + comp_y * (1.0 - nx.abs() * 0.5);
                out_min_x = out_min_x.min(x_final);
                out_max_x = out_max_x.max(x_final);
                out_min_y = out_min_y.min(y_final);
                out_max_y = out_max_y.max(y_final);
                out_pts.push([x_final, y_final]);
            }
            transformed_contours.push(out_pts);
        }

        let width = (out_max_x - out_min_x).max(1.0);
        let height = (out_max_y - out_min_y).max(1.0);
        for contour in &transformed_contours {
            if let Some(&first) = contour.first() {
                path_builder.begin(point(first[0] - out_min_x, first[1] - out_min_y));
                for &p in contour.iter().skip(1) {
                    path_builder.line_to(point(p[0] - out_min_x, p[1] - out_min_y));
                }
                path_builder.end(true);
            }
        }
        let path = path_builder.build();

        let mut geometry: VertexBuffers<GlyphVertex, u32> = VertexBuffers::new();
        let mut tessellator = FillTessellator::new();
        let options = FillOptions::default()
            .with_tolerance(0.35)
            .with_fill_rule(FillRule::EvenOdd);
        tessellator
            .tessellate_path(
                &path,
                &options,
                &mut BuffersBuilder::new(&mut geometry, |v: FillVertex| {
                    let p = v.position();
                    GlyphVertex {
                        position: [p.x, p.y],
                        uv: [(p.x / width).clamp(0.0, 1.0), (p.y / height).clamp(0.0, 1.0)],
                    }
                }),
            )
            .ok()?;

        let baseline_y = (-out_min_y).clamp(0.0, height);
        Some(StretchedMesh {
            vertices: geometry.vertices,
            indices: geometry.indices,
            width,
            height,
            baseline_y,
        })
    }

    fn warp_c_inf(u: f32, alpha: f32, stretch: f32) -> f32 {
        let extra = stretch - 1.0;
        let bell_int = u - (std::f32::consts::TAU * u).sin() / std::f32::consts::TAU;
        u + extra * ((1.0 - alpha) * bell_int + alpha * u)
    }

    fn warp_c_inf_deriv(u: f32, alpha: f32, stretch: f32) -> f32 {
        let extra = stretch - 1.0;
        let bell = 1.0 - (std::f32::consts::TAU * u).cos();
        1.0 + extra * ((1.0 - alpha) * bell + alpha)
    }
}
