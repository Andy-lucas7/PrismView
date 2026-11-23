//! Construção da lista de desenho de um quadro (layout das camadas, texto, vidro, mídia).

#[allow(unused_imports)]
use super::*;

// ───────────────────────── Construção do frame ─────────────────────────

#[derive(Clone)]
pub(super) struct TextDraw {
    pub(super) layout: TextLayout,
    pub(super) tex: Option<TextureId>,
    pub(super) content_hash: u64,
    pub(super) slot_key: Option<Arc<str>>,
}

pub(super) struct FillStyle {
    pub(super) color: [f32; 4],
    pub(super) gradient: Option<(GradientKind, [f32; 4], [f32; 2])>,
    pub(super) hollow: bool,
}

pub(super) struct Frame<'a> {
    pub(super) videos: &'a HashMap<String, VideoBinding>,
    pub(super) svgs: &'a mut SvgCache,
    pub(super) touch_index: &'a HashMap<String, u32>,
    pub(super) scene3d_out: Option<Scene3dOutput>,
    /// Backdrop do vidro (mídia de fundo desenhada em `background`), se houver.
    pub(super) backdrop: Option<GlassBackdrop>,
    pub(super) scene: &'a PrismScene,
    pub(super) ctx: &'a EvalContext,
    pub(super) device: &'a wgpu::Device,
    pub(super) queue: &'a wgpu::Queue,
    pub(super) compositor: &'a mut Compositor2D,
    pub(super) text: &'a mut TextSystem,
    pub(super) images: &'a mut ImageCache,
    pub(super) bindings: &'a mut HashMap<String, TextBinding>,
    pub(super) canvas: &'a mut Canvas,
    pub(super) canvas_key: &'a str,
    pub(super) scratch: &'a mut String,
    pub(super) asset_root: Option<&'a Path>,
    /// Deltas dos bindings reativos por camada (somados à base; nunca gravados nela).
    pub(super) fx: &'a LayerBindingSolver,
    pub(super) ui: f32,
    pub(super) vw: f32,
    pub(super) vh: f32,
    pub(super) z: u32,
}

impl Frame<'_> {
    pub(super) fn build(&mut self) {
        self.background();
        let root = ParentBox {
            left: 0.0,
            top: 0.0,
            w: self.vw,
            h: self.vh,
        };
        let scene = self.scene;
        for layer in &scene.layers {
            self.layer(layer, &root, Affine::IDENTITY, 1.0, None, false);
        }
    }

    // ── Avaliação FX ──

    pub(super) fn f(&self, prop: &StaticOrFx<f32>) -> f32 {
        let v = FxEngine::eval_f32(prop, self.ctx).value;
        if v.is_finite() {
            v
        } else {
            0.0
        }
    }

    pub(super) fn color(&self, prop: &StaticOrFx<String>) -> [f32; 4] {
        parse_color(&FxEngine::eval_string(prop, self.ctx).value)
    }

    pub(super) fn next_z(&mut self) -> u32 {
        self.z = self.z.saturating_add(1);
        self.z
    }

    pub(super) fn is_visible(&self, layer: &LayerNode) -> bool {
        if layer.hidden || !FxEngine::eval_bool(&layer.visible, self.ctx).value {
            return false;
        }
        let ctx = self.ctx;
        match layer.visibility_cond {
            VisibilityCondition::Always | VisibilityCondition::DesktopOnly => true,
            VisibilityCondition::Charging => ctx.system.battery_charging,
            VisibilityCondition::NotCharging => !ctx.system.battery_charging,
            VisibilityCondition::LowBattery => ctx.system.battery_percent <= 20,
            VisibilityCondition::Day => ctx.weather.is_day,
            VisibilityCondition::Night => !ctx.weather.is_day,
            VisibilityCondition::MediaPlaying => ctx.media.playing,
        }
    }

    pub(super) fn resolve_asset(&self, raw: &str) -> Option<PathBuf> {
        resolve_asset_path(self.scene, self.asset_root, raw)
    }

    // ── Vidro óptico ──

    /// Backdrop cover sobre a viewport (mesmo mapeamento do quad de fundo).
    pub(super) fn backdrop_for(
        &self,
        texture: TextureId,
        size: (u32, u32),
        content_hash: u64,
        live: bool,
        opaque: bool,
    ) -> GlassBackdrop {
        let (iw, ih) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let k = (self.vw / iw).max(self.vh / ih).max(1e-3);
        GlassBackdrop {
            texture,
            size,
            uv_rect: cover_uv(size.0, size.1, self.vw, self.vh),
            src_per_screen_px: 1.0 / k,
            content_hash,
            live,
            opaque,
        }
    }

    pub(super) fn is_glass(layer: &LayerNode) -> bool {
        matches!(
            layer.fill,
            FillMode::Adaptive | FillMode::ClassicBlur | FillMode::OpticalCrystal
        )
    }

    /// Mapeia `OpticalCrystalParams` para os parâmetros do shader (px físicos), em paridade 1:1
    /// com `OpticalGlassSurface.tsx` / `opticalGlassWebGL.ts`:
    /// - No modo `Glyph`, `pigment` recebe a cor + alpha reais do texto (`layer.color`), garantindo
    ///   cor e opacidade fiéis mesmo com `frost == 0`.
    /// - `brightness`, `contrast`, `saturation`, `light_angle_deg`, `refraction`, `bevel_width`,
    ///   dispersão (`×400`), especular (`fresnel_intensity`) e borda (`prismatic_rim_intensity`).
    pub(super) fn glass_params(
        &self,
        layer: &LayerNode,
        mode: GlassMode,
        opacity: f32,
        radius: f32,
    ) -> GlassParams {
        let c = &layer.crystal;
        let ui = self.ui;
        let tint = self.color(&c.tint_rgba);
        let g = self.fx.fx(layer.id.as_str()).map(|f| f.glass).unwrap_or_default();
        let pigment = if mode == GlassMode::Glyph {
            self.color(&layer.color)
        } else {
            [1.0, 1.0, 1.0, 0.0]
        };
        let mut p = GlassParams {
            blur_px: ((self.f(&c.blur_px) + g.blur_px) * ui).clamp(0.0, 80.0 * ui),
            frost: (self.f(&c.frost_amount) + g.frost).clamp(0.0, 0.94),
            tint: [tint[0], tint[1], tint[2]],
            noise: c.grain.clamp(0.0, 0.4),
            brightness: self.f(&c.brightness).clamp(0.5, 1.8),
            contrast: self.f(&c.contrast).clamp(0.6, 1.8),
            saturation: self.f(&c.saturation).clamp(0.0, 2.6),
            opacity: opacity.clamp(0.0, 1.0),
            radius_px: radius,
            bevel_px: self.f(&c.bevel_width).clamp(4.0, 80.0) * ui,
            light_angle_deg: self.f(&c.light_angle_deg),
            pigment,
            ..GlassParams::default()
        };
        if layer.fill == FillMode::OpticalCrystal {
            p.refraction_px = (self.f(&c.refraction) + g.refraction).clamp(0.0, 60.0) * ui;
            p.chroma_px = ((self.f(&c.dispersion_rgb) + g.dispersion) * 400.0).clamp(0.0, 6.0);
            p.specular = (self.f(&c.fresnel_intensity) + g.specular).clamp(0.0, 1.0);
            p.border = (self.f(&c.prismatic_rim_intensity) + g.rim).clamp(0.0, 0.8);
        } else if p.frost <= 0.001 && tint[3] > 0.001 {
            // `Adaptive`/`ClassicBlur` legado sem `frost_amount`: aproveita o alpha do tint.
            p.frost = (tint[3] * 0.5).clamp(0.0, 0.94);
        }
        p
    }

    /// Enfileira um vidro-placa ou glifo. Usa sempre o backdrop composto da saída (gradiente,
    /// imagem, vídeo e cena 3D juntos, 1:1 com a viewport e opaco).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_glass(
        &mut self,
        layer: &LayerNode,
        mode: GlassMode,
        half: [f32; 2],
        xf: &Affine,
        center: [f32; 2],
        z: u32,
        opacity: f32,
        radius: f32,
        glyph: Option<GlyphSource>,
    ) -> bool {
        let backdrop = GlassBackdrop {
            texture: TextureId::WHITE,
            size: (self.vw.round().max(1.0) as u32, self.vh.round().max(1.0) as u32),
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            src_per_screen_px: 1.0,
            content_hash: hash_str(self.canvas_key),
            live: true,
            opaque: true,
        };
        let params = self.glass_params(layer, mode, opacity, radius);
        let Some(glass) = self.canvas.glass.as_mut() else {
            return false;
        };
        let id = glass.push(GlassDraw {
            mode,
            backdrop,
            params,
            center,
            half,
            basis: xf.m,
            glyph,
        });
        self.canvas.push_marker(z, id);
        true
    }

    // ── Fundo ──

    pub(super) fn background(&mut self) {
        let (vw, vh) = (self.vw, self.vh);
        let center = [vw * 0.5, vh * 0.5];
        let half = [vw * 0.5, vh * 0.5];

        // Vídeo (frames de `set_video_frame`) ou imagem; carrossel ainda não existe; sem mídia de
        // fundo = cor de clear (e o vidro usa a próxima mídia full-bleed ou cai no tint).
        let scene = self.scene;
        let videos = self.videos;
        if let BackgroundSource::Video { path, .. } = &scene.background {
            let binding = self
                .resolve_asset(path)
                .and_then(|p| videos.get(&*p.to_string_lossy()));
            if let Some((id, v)) = binding.and_then(|v| v.alias.map(|id| (id, v))) {
                let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                    .kind(QuadKind::Opaque)
                    .uv(cover_uv(v.size.0, v.size.1, vw, vh));
                self.canvas.push(0, BlendKind::Normal, id, inst);
                self.backdrop = Some(self.backdrop_for(id, v.size, v.content_hash, true, true));
            }
        } else if let BackgroundSource::Image { path } = &scene.background {
            if let Some(path) = self.resolve_asset(path) {
                if let Some(img) = self
                    .images
                    .get(&path, (self.vw as u32, self.vh as u32), self.compositor, self.device, self.queue)
                {
                    let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                        .kind(QuadKind::Texture)
                        .uv(cover_uv(img.w, img.h, vw, vh));
                    self.canvas.push(0, BlendKind::Normal, img.id, inst);
                    let hash = hash_str(&path.to_string_lossy());
                    self.backdrop = Some(self.backdrop_for(img.id, (img.w, img.h), hash, false, true));
                }
            }
        }

        let dim = scene.dim.clamp(0.0, 1.0);
        if dim > 0.001 {
            let inst = QuadInstance::new(center, half, Affine::IDENTITY.m, 1.0)
                .color([0.0, 0.0, 0.0, dim]);
            self.canvas.push(0, BlendKind::Normal, TextureId::WHITE, inst);
        }
    }

    // ── Medição e layout ──

    pub(super) fn measure(&mut self, layer: &LayerNode) -> Size {
        match layer.kind {
            LayerKind::Text => {
                let draw = self.layout_text(layer);
                Size {
                    w: draw.layout.width,
                    h: draw.layout.height,
                }
            }
            LayerKind::Stack => self.stack_size(layer),
            _ => Size {
                w: (self.f(&layer.width_px) * self.ui).max(0.0),
                h: (self.f(&layer.height_px) * self.ui).max(0.0),
            },
        }
    }

    pub(super) fn stack_size(&mut self, layer: &LayerNode) -> Size {
        let spacing = layer.stack_spacing_px * self.ui;
        let horizontal = layer.stack_dir == StackDir::Horizontal;
        let (mut main, mut cross, mut count) = (0.0f32, 0.0f32, 0u32);
        for child in &layer.children {
            if !self.is_visible(child) {
                continue;
            }
            let s = self.measure(child);
            let (m, c) = if horizontal { (s.w, s.h) } else { (s.h, s.w) };
            main += m;
            cross = cross.max(c);
            count += 1;
        }
        if count > 1 {
            main += spacing * (count - 1) as f32;
        }
        if horizontal {
            Size { w: main, h: cross }
        } else {
            Size { w: cross, h: main }
        }
    }

    pub(super) fn layout_text(&mut self, layer: &LayerNode) -> TextDraw {
        self.scratch.clear();
        self.scratch.push_str(&layer.id);
        self.scratch.push('@');
        self.scratch.push_str(self.canvas_key);

        let content = FxEngine::eval_text(&layer.text.text, self.ctx).value;
        let content_hash = hash_str(&content);
        let size_px = (self.f(&layer.text.size_px) * self.ui).max(1.0);
        let spec = TextSpec {
            text: &content,
            font_id: &layer.text.font_id,
            size_px,
            weight: layer.text.weight,
            line_spacing: layer.text.line_spacing,
            letter_spacing_em: layer.text.letter_spacing_em,
            align: match layer.text.align {
                TextAlign::Start => TextAlignment::Start,
                TextAlign::Center => TextAlignment::Center,
                TextAlign::End => TextAlignment::End,
            },
            max_width_px: (layer.text.max_width_px.max(0.0)) * self.ui,
        };
        let layout = self.text.update(self.device, self.scratch.as_str(), &spec);

        let mut tex = None;
        let mut slot_key = None;
        if !layout.empty {
            let existing = self.bindings.get(self.scratch.as_str());
            match existing {
                Some(b) if b.gen == layout.tex_gen => {
                    tex = Some(b.id);
                    slot_key = Some(Arc::clone(&b.key));
                }
                other => {
                    let prev_id = other.map(|b| b.id);
                    let reused_key = other.map(|b| Arc::clone(&b.key));
                    if let Some(view) = self.text.texture_view(self.scratch.as_str()) {
                        let id = self.compositor.bind_view(self.device, prev_id, view);
                        let key = reused_key.unwrap_or_else(|| Arc::from(self.scratch.as_str()));
                        self.bindings.insert(
                            self.scratch.clone(),
                            TextBinding {
                                id,
                                gen: layout.tex_gen,
                                key: Arc::clone(&key),
                            },
                        );
                        tex = Some(id);
                        slot_key = Some(key);
                    }
                }
            }
        }
        TextDraw {
            layout,
            tex,
            content_hash,
            slot_key,
        }
    }

    // ── Camadas ──

    pub(super) fn layer(
        &mut self,
        layer: &LayerNode,
        parent: &ParentBox,
        parent_xf: Affine,
        inherited_opacity: f32,
        forced_center: Option<[f32; 2]>,
        parent_moving: bool,
    ) {
        let bound = self.fx.has_binding(layer.id.as_str());
        if !self.is_visible(layer) {
            // Binding pode trazê-la de volta: o caminho rápido não tem itens dela.
            self.canvas.bound_skipped |= bound;
            return;
        }
        // Base (estática/FX) + delta dos bindings: o delta é recalculado por quadro, sem deriva.
        let fx = self.fx.fx(layer.id.as_str()).unwrap_or_default();
        let own_moving = self.fx.is_layer_animating(layer.id.as_str());
        let moving = parent_moving || own_moving;
        let opacity = (self.f(&layer.opacity) + fx.opacity).clamp(0.0, 1.0) * inherited_opacity;
        if opacity <= 0.003 {
            self.canvas.bound_skipped |= bound;
            return;
        }
        let base_scale = self.f(&layer.scale);
        let scale = base_scale * (1.0 + fx.scale);
        if scale.abs() < 1e-3 {
            self.canvas.bound_skipped |= bound;
            return;
        }
        // Início da subárvore (caminho rápido dos bindings: `Canvas::bound`).
        let seq0 = self.canvas.next_seq();
        let hits0 = self.canvas.hits.len();
        let markers0 = self.canvas.marker_count();
        let had_scene3d = self.canvas.scene3d.is_some();
        let ui = self.ui;

        let mut text_draw = None;
        let size = match layer.kind {
            LayerKind::Text => {
                let draw = self.layout_text(layer);
                let sz = Size {
                    w: draw.layout.width,
                    h: draw.layout.height,
                };
                text_draw = Some(draw);
                sz
            }
            LayerKind::Stack => self.stack_size(layer),
            LayerKind::Shape
                if layer.id.ends_with("__pill")
                    && layer.children.len() == 1
                    && layer.children[0].kind == LayerKind::Text =>
            {
                let child = &layer.children[0];
                let draw = self.layout_text(child);
                let font_px = (self.f(&child.text.size_px) * ui).max(1.0);
                let pad_x = (font_px * 0.34).max(12.0 * ui);
                let pad_y = (font_px * 0.16).max(6.0 * ui);
                Size {
                    w: draw.layout.width + pad_x * 2.0,
                    h: draw.layout.height + pad_y * 2.0,
                }
            }
            _ if layer.fill_parent => Size {
                w: parent.w.max(0.0),
                h: parent.h.max(0.0),
            },
            _ => Size {
                w: (self.f(&layer.width_px) * ui).max(0.0),
                h: (self.f(&layer.height_px) * ui).max(0.0),
            },
        };

        let offset = [
            (self.f(&layer.offset_x_px) + fx.offset[0]) * ui,
            (self.f(&layer.offset_y_px) + fx.offset[1]) * ui,
        ];
        let center = match forced_center {
            Some(c) => [c[0] + offset[0], c[1] + offset[1]],
            None => {
                let ax = parent.left + self.f(&layer.x) * parent.w + offset[0];
                let ay = parent.top + self.f(&layer.y) * parent.h + offset[1];
                let (fx, fy) = anchor_factors(layer.anchor);
                [ax + (0.5 - fx) * size.w, ay + (0.5 - fy) * size.h]
            }
        };
        let rot = (self.f(&layer.rotation_deg) + fx.rotation_deg).to_radians();
        let xf = parent_xf.child(center, rot, scale);
        // Parâmetros sem o delta do binding (o caminho rápido recompõe com o delta novo).
        let bound_base = bound.then(|| {
            (
                [center[0] - fx.offset[0] * ui, center[1] - fx.offset[1] * ui],
                rot - fx.rotation_deg.to_radians(),
            )
        });

        // Controle de mídia tocável: mesma caixa orientada do desenho (mín. 10 px lógicos).
        let action = DesktopAction::from_touch(layer.touch.action_type).or_else(|| {
            self.touch_index
                .get(layer.id.as_str())
                .map(|&i| DesktopAction::Custom(i))
        });
        if let Some(action) = action {
            let min = 10.0 * ui;
            let half = [(size.w * 0.5).max(min), (size.h * 0.5).max(min)];
            if let Some(region) = HitRegion::new(xf.t, xf.m, half, action) {
                self.canvas.hits.push(region);
            }
        }

        let z = self.next_z();
        let blend = map_blend(layer.blend);
        match layer.kind {
            LayerKind::Text => self.draw_text(layer, text_draw, &xf, z, blend, opacity, moving),
            LayerKind::Shape => self.draw_shape(layer, size, &xf, z, blend, opacity),
            LayerKind::Image => self.draw_image(layer, size, &xf, z, blend, opacity),
            LayerKind::Scene3d => self.draw_scene3d(size, &xf, z, blend, opacity, layer.fill_parent),
            LayerKind::Video => self.draw_video(layer, size, &xf, z, blend, opacity),
            LayerKind::Svg | LayerKind::Icon => self.draw_svg(layer, size, &xf, z, blend, opacity),
            // DepthSubject (segmentação) ainda não tem renderer.
            LayerKind::Stack | LayerKind::Group | LayerKind::DepthSubject => {}
        }

        if !layer.children.is_empty() {
            if layer.kind == LayerKind::Stack {
                self.stack_children(layer, size, &xf, opacity, moving);
            } else {
                let pb = ParentBox {
                    left: -size.w * 0.5,
                    top: -size.h * 0.5,
                    w: size.w,
                    h: size.h,
                };
                for child in &layer.children {
                    self.layer(child, &pb, xf, opacity, None, moving);
                }
            }
        }

        // Fim da subárvore: pós-ordem (filhos já gravados antes deste pai).
        if let Some((center_base, rot_base)) = bound_base {
            let rigid = !had_scene3d && self.canvas.scene3d.is_some();
            let entry = BoundSubtree {
                key: Arc::from(layer.id.as_str()),
                seq: seq0..self.canvas.next_seq(),
                markers: (markers0 as usize)..(self.canvas.marker_count() as usize),
                hits: hits0..self.canvas.hits.len(),
                parent_m: parent_xf.m,
                parent_t: parent_xf.t,
                center_base,
                rot_base,
                scale_base: base_scale,
                ui,
                fx0: fx,
                moving0: own_moving,
                rigid,
            };
            self.canvas.bound.push(entry);
        }
    }

    pub(super) fn stack_children(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        opacity: f32,
        moving: bool,
    ) {
        let spacing = layer.stack_spacing_px * self.ui;
        let horizontal = layer.stack_dir == StackDir::Horizontal;
        let pb = ParentBox {
            left: -size.w * 0.5,
            top: -size.h * 0.5,
            w: size.w,
            h: size.h,
        };
        let mut cursor = if horizontal { -size.w * 0.5 } else { -size.h * 0.5 };
        for child in &layer.children {
            if !self.is_visible(child) {
                continue;
            }
            let s = self.measure(child);
            let (center, advance) = if horizontal {
                ([cursor + s.w * 0.5, 0.0], s.w)
            } else {
                ([0.0, cursor + s.h * 0.5], s.h)
            };
            self.layer(child, &pb, *xf, opacity, Some(center), moving);
            cursor += advance + spacing;
        }
    }

    // ── Preenchimento ──

    pub(super) fn fill_style(&self, layer: &LayerNode, opacity: f32) -> FillStyle {
        let base = with_alpha(self.color(&layer.color), opacity);
        match layer.fill {
            FillMode::Solid => FillStyle {
                color: base,
                gradient: None,
                hollow: false,
            },
            FillMode::Gradient => {
                let kind = match layer.gradient_kind {
                    SchemaGradient::Linear => GradientKind::Linear,
                    SchemaGradient::Radial => GradientKind::Radial,
                    SchemaGradient::Sweep => GradientKind::Sweep,
                };
                // Ângulo estilo CSS: 0° aponta para cima, 90° para a direita.
                let a = layer.gradient_angle_deg.to_radians();
                FillStyle {
                    color: base,
                    gradient: Some((
                        kind,
                        with_alpha(parse_color(&layer.color2), opacity),
                        [a.sin(), -a.cos()],
                    )),
                    hollow: false,
                }
            }
            FillMode::Hollow => FillStyle {
                color: base,
                gradient: None,
                hollow: true,
            },
            // Fallback sem backdrop de mídia: texto preserva sua cor/alpha reais; placa usa o tint/frost.
            FillMode::Adaptive | FillMode::ClassicBlur | FillMode::OpticalCrystal => {
                let fallback = if layer.kind == LayerKind::Text {
                    let c = self.color(&layer.color);
                    if c[3] > 0.003 {
                        c
                    } else {
                        self.color(&layer.crystal.tint_rgba)
                    }
                } else {
                    let mut c = self.color(&layer.crystal.tint_rgba);
                    let frost = self.f(&layer.crystal.frost_amount).clamp(0.0, 0.94);
                    if frost > 0.002 {
                        c[3] = frost;
                    }
                    c
                };
                FillStyle {
                    color: with_alpha(fallback, opacity),
                    gradient: None,
                    hollow: false,
                }
            }
        }
    }

    pub(super) fn push_shadow(
        &mut self,
        layer: &LayerNode,
        quad: ShadowQuad,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if layer.shadow.radius_px <= 0.0 {
            return;
        }
        let ui = self.ui;
        let color = with_alpha(parse_color(&layer.shadow.color_rgba), opacity);
        if color[3] <= 0.003 {
            return;
        }
        let center = [
            quad.center[0] + layer.shadow.dx_px * ui,
            quad.center[1] + layer.shadow.dy_px * ui,
        ];
        let blur_local = layer.shadow.radius_px * ui / quad.scale.max(1e-3);
        let inst = QuadInstance::new(center, quad.half, quad.basis, quad.scale)
            .radius(quad.radius)
            .color(color)
            .shadow(blur_local);
        self.canvas.push(z, blend, TextureId::WHITE, inst);
    }

    // ── Desenho por tipo ──

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_text(
        &mut self,
        layer: &LayerNode,
        draw: Option<TextDraw>,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
        moving: bool,
    ) {
        let Some(draw) = draw else {
            return;
        };
        let (Some(tex), false) = (draw.tex, draw.layout.empty) else {
            return;
        };
        let stretch_y = self.f(&layer.text.stem_stretch.stretch).clamp(1.0, 2.8);
        let text_xf = if (stretch_y - 1.0).abs() > 1e-3 {
            Affine {
                m: [xf.m[0], xf.m[1], xf.m[2] * stretch_y, xf.m[3] * stretch_y],
                t: xf.t,
            }
        } else {
            *xf
        };
        let scale = text_xf.scale();
        let half = [draw.layout.quad_width * 0.5, draw.layout.quad_height * 0.5];
        let mut center = text_xf.t;
        let glass_glyph = Self::is_glass(layer);
        // Sem rotação/escala e em repouso (fora de vidro/parallax): alinha à grade de pixels (1:1).
        // Em glifos de vidro ou durante parallax, mantém subpixel contínuo para movimento liso 60+ FPS.
        if !glass_glyph && !moving && text_xf.is_identity_linear() {
            center[0] = (center[0] - half[0]).round() + half[0];
            center[1] = (center[1] - half[1]).round() + half[1];
        }
        let uv = [0.0, 0.0, draw.layout.uv_max[0], draw.layout.uv_max[1]];

        // Vidro nos glifos: normais geradas na GPU a partir da máscara do slot de texto.
        if glass_glyph {
            if let Some(slot_key) = draw.slot_key {
                let mut token = hash_combine(draw.content_hash, hash_str(&layer.text.font_id));
                token = hash_combine(token, u64::from(draw.layout.quad_width.to_bits()));
                token = hash_combine(token, u64::from(draw.layout.quad_height.to_bits()));
                token = hash_combine(token, u64::from(layer.text.weight));
                token = hash_combine(token, u64::from(layer.text.letter_spacing_em.to_bits()));
                token = hash_combine(token, draw.layout.tex_gen);
                let source = GlyphSource {
                    key: slot_key,
                    token,
                    size: (
                        draw.layout.quad_width.ceil().max(1.0) as u32,
                        draw.layout.quad_height.ceil().max(1.0) as u32,
                    ),
                    bevel_px: self.f(&layer.crystal.bevel_width).clamp(4.0, 80.0) * self.ui,
                };
                if self.push_glass(
                    layer,
                    GlassMode::Glyph,
                    half,
                    &text_xf,
                    center,
                    z,
                    opacity,
                    0.0,
                    Some(source),
                ) {
                    return;
                }
            }
        }

        let fill = self.fill_style(layer, opacity);
        // Sombra do texto proporcional ao alpha real da cor do texto (evita silhueta preta em alpha 0%).
        if layer.shadow.radius_px > 0.0 && fill.color[3] > 0.003 {
            let ui = self.ui;
            let color = with_alpha(parse_color(&layer.shadow.color_rgba), fill.color[3]);
            if color[3] > 0.003 {
                let c = [
                    center[0] + layer.shadow.dx_px * ui,
                    center[1] + layer.shadow.dy_px * ui,
                ];
                let inst = QuadInstance::new(c, half, text_xf.m, scale)
                    .kind(QuadKind::Mask)
                    .color(color)
                    .uv(uv);
                self.canvas.push(z, blend, tex, inst);
            }
        }

        if fill.color[3] <= 0.003 && fill.gradient.is_none() {
            return;
        }
        let mut inst = QuadInstance::new(center, half, text_xf.m, scale)
            .kind(QuadKind::Mask)
            .color(fill.color)
            .uv(uv);
        if let Some((kind, color2, dir)) = fill.gradient {
            inst = inst.gradient(kind, color2, dir);
        }
        self.canvas.push(z, blend, tex, inst);
    }

    pub(super) fn draw_shape(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 {
            return;
        }
        let ui = self.ui;
        let scale = xf.scale();
        let half = [size.w * 0.5, size.h * 0.5];
        let min_half = half[0].min(half[1]);
        let radius = match layer.shape_kind {
            ShapeKind::Rect | ShapeKind::Bar => layer.corner_radius_px * ui,
            ShapeKind::Circle | ShapeKind::Ring | ShapeKind::Line => min_half,
        };
        let quad = ShadowQuad {
            center: xf.t,
            half,
            basis: xf.m,
            scale,
            radius,
        };
        self.push_shadow(layer, quad, z, blend, opacity);

        let fill = self.fill_style(layer, opacity);
        let stroke_w = layer.stroke.width_px.max(0.0) * ui;
        let stroke_color = with_alpha(parse_color(&layer.stroke.color_rgba), opacity);

        let mut base = QuadInstance::new(xf.t, half, xf.m, scale).radius(radius);

        match layer.shape_kind {
            ShapeKind::Ring => {
                let thickness = if stroke_w > 0.0 {
                    stroke_w
                } else {
                    (min_half * 0.12).max(2.0 * ui)
                };
                let inst = base.color([0.0; 4]).border(thickness, fill.color);
                self.canvas.push(z, blend, TextureId::WHITE, inst);
            }
            ShapeKind::Bar => {
                let track = with_alpha(parse_color(&layer.track_color), opacity);
                self.canvas
                    .push(z, blend, TextureId::WHITE, base.color(track));

                let pct = FxEngine::eval_expr(&layer.progress_expr, self.ctx)
                    .value
                    .trim()
                    .trim_end_matches('%')
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(0.0);
                let progress = if pct.is_finite() {
                    (pct / 100.0).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let fw = size.w * progress;
                if fw >= 0.5 {
                    let local = [-size.w * 0.5 + fw * 0.5, 0.0];
                    let c = xf.apply(local);
                    let mut bar = QuadInstance::new(c, [fw * 0.5, half[1]], xf.m, scale)
                        .radius(radius)
                        .color(fill.color);
                    if let Some((kind, color2, dir)) = fill.gradient {
                        bar = bar.gradient(kind, color2, dir);
                    }
                    self.canvas.push(z, blend, TextureId::WHITE, bar);
                }
            }
            ShapeKind::Rect | ShapeKind::Circle | ShapeKind::Line => {
                if Self::is_glass(layer)
                    && self.push_glass(
                        layer,
                        GlassMode::Plate,
                        half,
                        xf,
                        xf.t,
                        z,
                        opacity,
                        radius,
                        None,
                    )
                {
                    return;
                }
                if fill.hollow {
                    let w = if stroke_w > 0.0 { stroke_w } else { 2.0 * ui };
                    let c = if stroke_w > 0.0 { stroke_color } else { fill.color };
                    base = base.color([0.0; 4]).border(w, c);
                } else {
                    base = base.color(fill.color);
                    if let Some((kind, color2, dir)) = fill.gradient {
                        base = base.gradient(kind, color2, dir);
                    }
                    if stroke_w > 0.0 {
                        base = base.border(stroke_w, stroke_color);
                    }
                }
                self.canvas.push(z, blend, TextureId::WHITE, base);
            }
        }
    }

    /// Camada 3D: registra a caixa (só a primeira da cena; um `Scene3dRenderer` por motor) e
    /// desenha a textura de saída já renderizada (pré-multiplicada, mesmo aspecto da caixa).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_scene3d(
        &mut self,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
        fill_parent: bool,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 || self.canvas.scene3d.is_some() {
            return;
        }
        let half = [size.w * 0.5, size.h * 0.5];
        self.canvas.scene3d = Some(Scene3dPlacement {
            center: xf.t,
            half,
            basis: xf.m,
        });
        let Some(out) = self.scene3d_out else {
            return; // 1º quadro: o 3D ainda não renderizou; a lista é refeita ao registrar.
        };
        let inst = QuadInstance::new(xf.t, half, xf.m, xf.scale())
            .kind(QuadKind::Texture)
            .color([1.0, 1.0, 1.0, opacity])
            .uv(cover_uv(out.size.0, out.size.1, size.w, size.h));
        self.canvas.push(z, blend, out.id, inst);
        // 3D full-bleed: vira o backdrop do vidro desenhado por cima (preserva alpha da cena 3D).
        if fill_parent {
            self.backdrop = Some(self.backdrop_for(out.id, out.size, SCENE3D_BACKDROP_HASH, true, false));
        }
    }

    /// Camada de vídeo: frame atual do decoder (zero-copy), cover na caixa, cantos arredondados.
    pub(super) fn draw_video(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 {
            return;
        }
        let raw = FxEngine::eval_string(&layer.asset, self.ctx).value;
        let Some(path) = self.resolve_asset(&raw) else {
            return;
        };
        let videos = self.videos;
        let Some(v) = videos.get(&*path.to_string_lossy()) else {
            return;
        };
        let Some(id) = v.alias else {
            return; // 1º frame ainda não decodificado; `set_video_frame` refaz a lista
        };
        let scale = xf.scale();
        let half = [size.w * 0.5, size.h * 0.5];
        let radius = layer.corner_radius_px * self.ui;
        let quad = ShadowQuad {
            center: xf.t,
            half,
            basis: xf.m,
            scale,
            radius,
        };
        self.push_shadow(layer, quad, z, blend, opacity);
        let inst = QuadInstance::new(xf.t, half, xf.m, scale)
            .kind(QuadKind::Opaque)
            .radius(radius)
            .color([1.0, 1.0, 1.0, opacity])
            .uv(cover_uv(v.size.0, v.size.1, size.w, size.h));
        self.canvas.push(z, blend, id, inst);
        if layer.fill_parent {
            self.backdrop = Some(self.backdrop_for(id, v.size, v.content_hash, true, true));
        }
    }

    /// Ícone/SVG: `svg_markup` rasterizado (resvg) no tamanho físico da caixa, tingido por `color`
    /// (`currentColor` do markup = branco; SVG multicolorido usa `color` branco).
    pub(super) fn draw_svg(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if layer.svg_markup.trim().is_empty() || size.w <= 0.5 || size.h <= 0.5 {
            return;
        }
        let scale = xf.scale();
        let (pw, ph) = (svg_bucket(size.w * scale), svg_bucket(size.h * scale));
        let Some(img) = self
            .svgs
            .get(&layer.svg_markup, pw, ph, self.compositor, self.device, self.queue)
        else {
            return;
        };
        let color = with_alpha(self.color(&layer.color), opacity);
        let half = [size.w * 0.5, size.h * 0.5];
        let inst = QuadInstance::new(xf.t, half, xf.m, scale)
            .kind(QuadKind::Texture)
            .color(color);
        self.canvas.push(z, blend, img.id, inst);
    }

    pub(super) fn draw_image(
        &mut self,
        layer: &LayerNode,
        size: Size,
        xf: &Affine,
        z: u32,
        blend: BlendKind,
        opacity: f32,
    ) {
        if size.w <= 0.0 || size.h <= 0.0 {
            return;
        }
        let raw = FxEngine::eval_string(&layer.asset, self.ctx).value;
        let Some(path) = self.resolve_asset(&raw) else {
            return;
        };
        let Some(img) = self
            .images
            .get(&path, (self.vw as u32, self.vh as u32), self.compositor, self.device, self.queue)
        else {
            return;
        };

        let scale = xf.scale();
        let half = [size.w * 0.5, size.h * 0.5];
        let radius = layer.corner_radius_px * self.ui;
        let quad = ShadowQuad {
            center: xf.t,
            half,
            basis: xf.m,
            scale,
            radius,
        };
        self.push_shadow(layer, quad, z, blend, opacity);

        let inst = QuadInstance::new(xf.t, half, xf.m, scale)
            .kind(QuadKind::Texture)
            .radius(radius)
            .color([1.0, 1.0, 1.0, opacity])
            .uv(cover_uv(img.w, img.h, size.w, size.h));
        self.canvas.push(z, blend, img.id, inst);
        if layer.fill_parent {
            let hash = hash_str(&path.to_string_lossy());
            self.backdrop = Some(self.backdrop_for(img.id, (img.w, img.h), hash, false, true));
        }
    }
}

/// Geometria usada para a sombra de uma camada.
#[derive(Clone, Copy)]
pub(super) struct ShadowQuad {
    pub(super) center: [f32; 2],
    pub(super) half: [f32; 2],
    pub(super) basis: [f32; 4],
    pub(super) scale: f32,
    pub(super) radius: f32,
}

#[cfg(test)]
pub(super) mod fast_path_tests {
    use super::Affine;

    pub(super) fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    /// O delta `xf1 ∘ xf0⁻¹` leva qualquer ponto desenhado com `xf0` ao mesmo ponto com `xf1`
    /// (o que o caminho rápido aplica às instâncias em vez de refazer o layout).
    #[test]
    pub(super) fn delta_maps_old_world_to_new_world() {
        let parent = Affine::IDENTITY.child([400.0, 300.0], 0.3, 1.5);
        let xf0 = parent.child([12.0, -8.0], 0.1, 1.0);
        let xf1 = parent.child([20.0, 4.0], 0.25, 1.2);
        let delta = xf1.then_apply(&xf0.inverse().expect("inversível"));
        for local in [[0.0, 0.0], [50.0, -20.0], [-7.5, 33.0]] {
            assert!(close(delta.apply(xf0.apply(local)), xf1.apply(local)));
        }
    }

    #[test]
    pub(super) fn image_is_reduced_to_cover_the_output_only() {
        use super::image_target_scale;
        // 6000×4000 num 1080p: cobre com 1920×1280 (k = 0,32).
        let k = image_target_scale((6000, 4000), (1920, 1080), 16384);
        assert!((k - 0.32).abs() < 0.01, "k = {k}");
        // Imagem menor que a saída nunca é ampliada aqui.
        assert_eq!(image_target_scale((800, 600), (1920, 1080), 16384), 1.0);
        // Teto do device vence.
        assert!(image_target_scale((40000, 100), (100, 100), 16384) <= 16384.0 / 40000.0);
    }

    #[test]
    pub(super) fn degenerate_affine_has_no_inverse() {
        let flat = Affine::IDENTITY.child([0.0, 0.0], 0.0, 0.0);
        assert!(flat.inverse().is_none());
    }
}
