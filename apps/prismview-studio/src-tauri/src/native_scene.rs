//! Contrato Estúdio → motor nativo: converte o projeto do Studio (`PrismProject`, TS/camelCase)
//! no `PrismScene` do `prism-core` (snake_case, lido por `prismview-engine`).
//!
//! Este crate não depende do `prism-core`: a cena é montada como `serde_json::Value` no formato
//! serde de `prism_core::schema::PrismScene` (enums em SCREAMING_SNAKE_CASE, `StaticOrFx` untagged
//! = número/texto puro ou `{ "fallback", "expr" }`).
//!
//! Mapeamento (`SceneLayer.kind` → `LayerNode.kind`) — nenhum tipo bloqueia a aplicação:
//! - `TEXT` (relógio/telemetria = fórmulas `$df()$`, `$sys()$`…, mesmo dialeto do FX nativo) → `TEXT`
//!   com `font_path` (arquivo .ttf/.otf real); vidro `GLYPH` → fill `OPTICAL_CRYSTAL` no texto;
//!   vidro `PILL` → placa `SHAPE` com o texto filho.
//! - `GLASS` → `SHAPE` retângulo com fill `OPTICAL_CRYSTAL` (vidro desligado = só os filhos).
//! - `GROUP` → `GROUP` (contêiner livre) ou placa de vidro `SHAPE` com os filhos.
//! - `IMAGE` → `IMAGE` (`MEDIA_COVER` = `$mi(cover)$`).
//! - `VIDEO` → `VIDEO` (decoder Media Foundation zero-copy no motor).
//! - Camada visível mais baixa full-bleed `VIDEO`/`IMAGE` → `background` (backdrop do vidro).
//! - `PROGRESS` → `SHAPE` `BAR` com `progress_expr` (`$mi(pct)$`, `$sys(cpu)$`…).
//! - `SVG` → `SVG` com `svg_markup` (ícone embutido resolvido pelo Estúdio em `svg.resolvedSvg`,
//!   ou SVG próprio), rasterizado pelo motor (resvg). Play/Pause alternado = 2 camadas com
//!   `visible` por `$mi(playing)$`.
//! - `SCENE_3D` → `SCENE3D` com `scene3d` = `Scene3DConfig` (camelCase, bindings 3D 1:1).
//!
//! Interação: `clickAction` (mídia SMTC, `OPEN_URL`, `OPEN_APP`, `TOGGLE_GLOBAL`, `SET_GLOBAL`,
//! `NEXT_GLOBAL_LIST`) → `touch`; parallax `parallaxX/Y` → bindings nativos `CURSOR_X/Y` →
//! `OFFSET_X/Y` (36/26 px lógicos × fator, a fórmula do Estúdio); `bindings` do projeto passam
//! direto. Bindings 3D (áudio, teclado, hover/clique de malha, clipes) vão dentro do `scene3d`.
//!
//! Layout: o Estúdio posiciona pelo centro (`x/y` 0..1 da caixa do pai) em px lógicos escalados por
//! `containerScale`. A cena leva `logical_size` (wallpaper = 1080×1080: `min(w,h)/1080` reproduz o
//! `containerScale` em paisagem e retrato; widget = tamanho do widget) e camadas full-bleed usam
//! `fill_parent`.
//!
//! Assets: `materialize_assets` resolve todo caminho (fundo, imagens, vídeos, fontes, .glb, HDRI,
//! texturas dinâmicas) para um arquivo absoluto existente e o espelha em `native/assets/`
//! (hard link, ou cópia), para a cena ativa não depender de onde o Estúdio guardou a mídia.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Tint padrão do vidro nativo (≈ 0,15 de alpha), o Estúdio guarda só RGB.
const GLASS_TINT_ALPHA: f64 = 0.15;
/// Espaço lógico de wallpapers: `min(w, h) / 1080` = `containerScale` do Estúdio.
const WALLPAPER_LOGICAL: f64 = 1080.0;
/// Caixa (px lógicos) que cobre qualquer aspecto ≤ 32:9 com escala `h/1080`.
const FULLSCREEN_LOGICAL: f64 = 4096.0;
/// Deslocamento máximo do parallax do Estúdio (px lógicos por unidade de `parallaxX/Y`).
const PARALLAX_X_PX: f64 = 36.0;
const PARALLAX_Y_PX: f64 = 26.0;
/// Acima disso, mídia não é copiada (só hard link; senão usa o caminho original).
const MAX_COPY_BYTES: u64 = 512 * 1024 * 1024;

pub fn project_to_scene(project: &Value) -> Result<Value, String> {
    // Já é um `PrismScene` (importado/feito à mão): passa direto.
    if project.get("package_type").is_some() {
        return Ok(project.clone());
    }
    let layers = project
        .get("layers")
        .and_then(Value::as_array)
        .ok_or("studio_project_invalid: sem 'layers'")?;

    let is_widget = text(project, "packageType").eq_ignore_ascii_case("WIDGET");
    let mut cx = Ctx {
        is_widget,
        bindings: Vec::new(),
    };

    // 1) Fundo de mídia: a camada visível mais baixa, se for full-bleed vídeo/imagem.
    let background_index = layers
        .iter()
        .position(|l| !boolean(l, "hidden", false))
        .filter(|&i| cx.is_full_bleed(&layers[i], true) && matches!(kind(&layers[i]), "VIDEO" | "IMAGE"))
        .filter(|&i| media_background(&layers[i]).is_some());
    let background = match background_index {
        Some(i) => media_background(&layers[i]).unwrap_or_else(|| json!({ "type": "NONE" })),
        None => json!({ "type": "NONE" }),
    };

    // 2) Camadas (cor/gradiente de fundo como 1ª camada quando não há mídia nem widget transparente).
    let mut out_layers = Vec::with_capacity(layers.len() + 1);
    if background_index.is_none() {
        if let Some(fill) = background_fill(project, is_widget, layers) {
            out_layers.push(fill);
        }
    }
    for (i, layer) in layers.iter().enumerate() {
        if Some(i) == background_index {
            continue;
        }
        out_layers.extend(cx.layer(layer, true));
    }

    // 3) Cabeçalho, globais, widget, espaço lógico, bindings nativos.
    let widget = project.get("widget").cloned().unwrap_or(Value::Null);
    let (ww, wh) = (number(&widget, "width", 480.0).max(1.0), number(&widget, "height", 240.0).max(1.0));
    let logical = if is_widget {
        [ww, wh]
    } else {
        [WALLPAPER_LOGICAL, WALLPAPER_LOGICAL]
    };
    let globals: Vec<Value> = project
        .get("globals")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(convert_global).collect())
        .unwrap_or_default();
    let mut bindings = project
        .get("bindings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    bindings.append(&mut cx.bindings);

    let mut scene = json!({
        "id": non_empty(text(project, "id"), "studio-project"),
        "title": non_empty(text(project, "title"), "PrismView"),
        "package_type": if is_widget { "WIDGET" } else { "WALLPAPER" },
        "dim": number(project, "dim", 0.0).clamp(0.0, 1.0),
        "background": background,
        "widget": {
            "aspect_ratio": ww / wh,
            "pos_x": number(&widget, "posX", 96.0).round() as i64,
            "pos_y": number(&widget, "posY", 96.0).round() as i64,
            "width_px": ww.round() as u64,
            "height_px": wh.round() as u64,
            "pinned_to_desktop": true,
            "click_through": boolean(&widget, "locked", false),
        },
        "globals": globals,
        "layers": out_layers,
        "logical_size": logical,
    });
    if !bindings.is_empty() {
        scene["bindings"] = Value::Array(bindings);
    }
    Ok(scene)
}

// ───────────────────────── Camadas ─────────────────────────

struct Ctx {
    is_widget: bool,
    /// Bindings nativos gerados (parallax por cursor).
    bindings: Vec<Value>,
}

impl Ctx {
    /// Mesma regra do `isLayerFullBleed` do `UnifiedCompositor`.
    fn is_full_bleed(&self, layer: &Value, top_level: bool) -> bool {
        !self.is_widget
            && top_level
            && (number(layer, "x", 0.5) - 0.5).abs() < 0.02
            && (number(layer, "y", 0.5) - 0.5).abs() < 0.02
            && number(layer, "width", 0.0) >= 1600.0
            && number(layer, "height", 0.0) >= 900.0
            && matches!(kind(layer), "VIDEO" | "IMAGE" | "SCENE_3D")
    }

    /// Uma camada do Estúdio vira 0, 1 ou 2 nós (Play/Pause alternado).
    fn layer(&mut self, layer: &Value, top_level: bool) -> Vec<Value> {
        let glass = layer.get("glass").cloned().unwrap_or(Value::Null);
        let glass_on = boolean(&glass, "enabled", false);
        let mut node = self.base(layer, top_level);

        match kind(layer) {
            "TEXT" => {
                let t = layer.get("text").cloned().unwrap_or(Value::Null);
                self.fill_text(&mut node, layer, &t);
                if glass_on {
                    if text(&glass, "textMode").eq_ignore_ascii_case("PILL") {
                        return vec![self.pill(node, layer, &glass)];
                    }
                    node["fill"] = json!("OPTICAL_CRYSTAL");
                    node["crystal"] = crystal(&glass);
                }
            }
            "GLASS" => {
                let children = self.children(layer);
                if !glass_on {
                    // O Estúdio só desenha a placa com vidro ligado; filhos continuam visíveis.
                    if children.is_empty() {
                        return Vec::new();
                    }
                    node["kind"] = json!("GROUP");
                } else {
                    glass_plate(&mut node, &glass);
                }
                node["children"] = Value::Array(children);
            }
            "GROUP" => {
                if glass_on {
                    glass_plate(&mut node, &glass);
                } else {
                    node["kind"] = json!("GROUP");
                }
                node["children"] = Value::Array(self.children(layer));
            }
            "IMAGE" => {
                let img = layer.get("image").cloned().unwrap_or(Value::Null);
                node["kind"] = json!("IMAGE");
                node["corner_radius_px"] = json!(number(&img, "borderRadius", 0.0).max(0.0));
                node["asset"] = if text(&img, "sourceMode").eq_ignore_ascii_case("MEDIA_COVER") {
                    json!({ "fallback": "", "expr": "$mi(cover)$" })
                } else {
                    match local_path(text(&img, "src")) {
                        Some(path) => json!(path),
                        None => return Vec::new(), // sem arquivo local: o Estúdio também não mostra nada
                    }
                };
            }
            "VIDEO" => {
                let v = layer.get("video").cloned().unwrap_or(Value::Null);
                let Some(path) = local_path(text(&v, "src")) else {
                    return Vec::new();
                };
                node["kind"] = json!("VIDEO");
                node["asset"] = json!(path);
                node["corner_radius_px"] = json!(number(&v, "clipRadius", 0.0).max(0.0));
            }
            "PROGRESS" => {
                let p = layer.get("progress").cloned().unwrap_or(Value::Null);
                node["kind"] = json!("SHAPE");
                node["shape_kind"] = json!("BAR");
                node["fill"] = json!("SOLID");
                node["color"] = json!(css_to_argb(text(&p, "fillColor"), number(&p, "fillAlpha", 1.0)));
                node["track_color"] = json!(css_to_argb(text(&p, "trackColor"), number(&p, "trackAlpha", 0.2)));
                node["corner_radius_px"] = json!(number(&p, "borderRadius", 0.0).max(0.0));
                node["progress_expr"] = json!(match text(&p, "source") {
                    "CPU" => "$sys(cpu)$",
                    "RAM" => "$sys(ram)$",
                    "BATTERY" => "$bi(level)$",
                    _ => "$mi(pct)$",
                });
            }
            "SVG" => return self.svg(node, layer),
            "SCENE_3D" => {
                let Some(config) = layer.get("scene3d") else {
                    return Vec::new();
                };
                node["kind"] = json!("SCENE3D");
                node["scene3d"] = scene3d(config);
            }
            // Tipo desconhecido (versão futura do Estúdio): filhos continuam visíveis.
            _ => {
                let children = self.children(layer);
                if children.is_empty() {
                    return Vec::new();
                }
                node["kind"] = json!("GROUP");
                node["children"] = Value::Array(children);
            }
        }
        vec![node]
    }

    /// Campos comuns de `LayerNode` (+ bindings de parallax e ação de toque).
    fn base(&mut self, layer: &Value, top_level: bool) -> Value {
        let id = non_empty(text(layer, "id"), "layer").to_string();
        let mut node = json!({
            "id": id,
            "name": non_empty(text(layer, "name"), "Camada"),
            "kind": "SHAPE",
            "x": fx_num(layer, "x", number(layer, "x", 0.5)),
            "y": fx_num(layer, "y", number(layer, "y", 0.5)),
            "anchor": "CENTER",
            "rotation_deg": fx_num(layer, "rotation", number(layer, "rotation", 0.0)),
            "opacity": fx_num(layer, "opacity", number(layer, "opacity", 1.0).clamp(0.0, 1.0)),
            "visible": fx_bool(layer, "visible"),
            "blend": blend(text(layer, "blendMode")),
            "hidden": boolean(layer, "hidden", false),
            "locked": boolean(layer, "locked", false),
            "width_px": fx_num(layer, "width", number(layer, "width", 320.0).max(0.0)),
            "height_px": fx_num(layer, "height", number(layer, "height", 140.0).max(0.0)),
            "corner_radius_px": 0.0,
            "children": [],
        });
        if self.is_full_bleed(layer, top_level) {
            node["fill_parent"] = json!(true);
        }
        if let Some(touch) = touch_action(layer) {
            node["touch"] = touch;
        }
        // Parallax do Estúdio: deslocamento = (cursor - 0,5) × 2 × 36/26 px × fator.
        let (px, py) = (number(layer, "parallaxX", 0.0), number(layer, "parallaxY", 0.0));
        if !boolean(layer, "hidden", false) {
            for (factor, source, target, max_px) in [
                (px, "CURSOR_X", "OFFSET_X", PARALLAX_X_PX),
                (py, "CURSOR_Y", "OFFSET_Y", PARALLAX_Y_PX),
            ] {
                if factor.abs() > 1e-4 {
                    self.bindings.push(json!({
                        "id": format!("{id}__parallax_{}", &target[7..].to_ascii_lowercase()),
                        "layerId": id,
                        "source": source,
                        "target": target,
                        "strength": factor * max_px,
                        "smoothing": 0.12,
                    }));
                }
            }
        }
        node
    }

    fn children(&mut self, layer: &Value) -> Vec<Value> {
        layer
            .get("children")
            .and_then(Value::as_array)
            .map(|list| list.iter().flat_map(|c| self.layer(c, false)).collect())
            .unwrap_or_default()
    }

    fn fill_text(&self, node: &mut Value, layer: &Value, t: &Value) {
        node["kind"] = json!("TEXT");
        node["fill"] = json!("SOLID");
        node["color"] = fx_str(layer, "color", css_to_argb(text(t, "color"), number(t, "colorAlpha", 1.0)));
        let mut content = json!({
            "text": text(t, "content"),
            "font_id": non_empty(text(t, "fontFamily"), "Outfit"),
            "size_px": number(t, "fontSize", 96.0).max(1.0),
            "weight": number(t, "fontWeight", 600.0).clamp(100.0, 900.0).round() as u64,
            "letter_spacing_em": number(t, "letterSpacing", 0.0),
            "align": match text(t, "align") {
                "left" => "START",
                "right" => "END",
                _ => "CENTER",
            },
            "stem_stretch": { "stretch": number(t, "stemStretch", 1.0).clamp(1.0, 2.8) },
        });
        if let Some(path) = local_path(text(t, "fontPath")) {
            content["font_path"] = json!(path);
        }
        node["text"] = content;
        let shadow = number(t, "shadowBlur", 0.0);
        if shadow > 0.0 {
            node["shadow"] = json!({ "radius_px": shadow, "color_rgba": "#80000000", "dx_px": 0.0, "dy_px": 0.0 });
        }
    }

    /// Vidro `PILL`: placa de vidro do tamanho da camada com o texto centralizado como filho.
    fn pill(&mut self, text_node: Value, layer: &Value, glass: &Value) -> Value {
        let mut plate = text_node.clone();
        plate["id"] = json!(format!("{}__pill", text(layer, "id")));
        plate["name"] = json!(format!("{} (pílula)", non_empty(text(layer, "name"), "Texto")));
        if let Some(map) = plate.as_object_mut() {
            map.remove("text");
            map.remove("color");
            map.remove("shadow");
        }
        glass_plate(&mut plate, glass);
        let mut child = text_node;
        for key in ["x", "y"] {
            child[key] = json!(0.5);
        }
        child["rotation_deg"] = json!(0.0);
        child["opacity"] = json!(1.0);
        child["fill_parent"] = json!(false);
        if let Some(map) = child.as_object_mut() {
            map.remove("touch");
        }
        // O parallax (binding pelo id da camada) passa para a placa, que leva o texto junto.
        let child_id = text(&child, "id").to_string();
        let plate_id = text(&plate, "id").to_string();
        for b in self.bindings.iter_mut() {
            if text(b, "layerId") == child_id {
                b["layerId"] = json!(plate_id);
            }
        }
        plate["children"] = json!([child]);
        plate
    }

    /// Ícone embutido (markup resolvido pelo Estúdio) ou SVG próprio → `svg_markup`.
    fn svg(&mut self, mut node: Value, layer: &Value) -> Vec<Value> {
        let svg = layer.get("svg").cloned().unwrap_or(Value::Null);
        let custom = text(&svg, "customSvg").trim();
        let primary = if custom.is_empty() { text(&svg, "resolvedSvg") } else { custom };
        let Some(markup) = prepare_svg(primary) else {
            return Vec::new();
        };
        let tintable = markup.contains("currentColor");
        let alpha = number(&svg, "colorAlpha", 1.0);
        node["kind"] = json!("SVG");
        node["color"] = if tintable {
            fx_str(layer, "color", css_to_argb(text(&svg, "color"), alpha))
        } else {
            json!(css_to_argb("#FFFFFF", alpha))
        };
        node["svg_markup"] = json!(markup);
        let (vw, vh) = svg_viewbox(&markup).unwrap_or((24.0, 24.0));
        node["svg_viewbox_w"] = json!(vw);
        node["svg_viewbox_h"] = json!(vh);

        // Play/Pause do Estúdio troca o ícone pelo estado da mídia: 2 nós com `visible` oposto.
        let alt = if custom.is_empty() { prepare_svg(text(&svg, "resolvedSvgAlt")) } else { None };
        match alt {
            Some(alt_markup) if text(layer, "clickAction") == "MEDIA_PLAY_PAUSE" => {
                let mut paused = node.clone();
                paused["visible"] = json!({ "fallback": true, "expr": "$if(mi(playing), 0, 1)$" });
                let mut playing = node;
                playing["id"] = json!(format!("{}__playing", text(layer, "id")));
                playing["svg_markup"] = json!(alt_markup);
                playing["visible"] = json!({ "fallback": false, "expr": "$mi(playing)$" });
                vec![paused, playing]
            }
            _ => vec![node],
        }
    }
}

/// `clickAction` do Estúdio → `TouchAction` nativo.
fn touch_action(layer: &Value) -> Option<Value> {
    let action = text(layer, "clickAction");
    match action {
        "MEDIA_PLAY_PAUSE" | "MEDIA_NEXT" | "MEDIA_PREV" => Some(json!({ "action_type": action })),
        "OPEN_URL" => Some(json!({ "action_type": action, "url": text(layer, "clickUrl") })),
        "OPEN_APP" => Some(json!({
            "action_type": action,
            "target_exec": text(layer, "clickTarget"),
            "target_label": text(layer, "clickLabel"),
        })),
        "TOGGLE_GLOBAL" | "SET_GLOBAL" | "NEXT_GLOBAL_LIST" => Some(json!({
            "action_type": action,
            "global_name": text(layer, "clickGlobal"),
            "global_value": text(layer, "clickValue"),
        })),
        _ => None,
    }
}

/// SVG pronto para o resvg: `xmlns` obrigatório e `currentColor` = branco (o motor tinge).
fn prepare_svg(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let start = raw.find("<svg")?;
    let mut markup = raw[start..].to_string();
    let tag_end = markup.find('>')?;
    let mut extra = String::new();
    if !markup[..tag_end].contains("xmlns=") {
        extra.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
    }
    if attr(&markup[..tag_end], "color").is_none() {
        extra.push_str(" color=\"#FFFFFF\"");
    }
    markup.insert_str(4, &extra);
    Some(markup)
}

/// `Scene3DConfig` com caminhos de mídia normalizados (URLs `asset:` → caminho local).
fn scene3d(config: &Value) -> Value {
    let mut cfg = config.clone();
    let Some(map) = cfg.as_object_mut() else {
        return Value::Object(Map::new());
    };
    migrate_scene3d_v3(map);
    normalize_path_field(map, "envMapSrc");
    if let Some(objects) = map.get_mut("objects").and_then(Value::as_array_mut) {
        for obj in objects.iter_mut().filter_map(Value::as_object_mut) {
            normalize_path_field(obj, "modelSrc");
            normalize_dynamic_textures(obj);
        }
    }
    cfg
}

const SCENE3D_SCHEMA_VERSION: u64 = 3;
const LEGACY_DEFAULT_AUTO_ROTATE: f64 = 0.22;
const LEGACY_SCENE3D_KEYS: [&str; 7] = [
    "modelSrc",
    "showcaseVariant",
    "autoRotateSpeed",
    "modelScale",
    "modelOffsetY",
    "playback",
    "dynamicTextures",
];

/// v2→v3: 0,22 rad/s (giro padrão antigo) vira 0; outro valor é mantido.
fn migrate_legacy_spin(v: Option<&Value>) -> f64 {
    let n = v.and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0);
    if (n - LEGACY_DEFAULT_AUTO_ROTATE).abs() < 1e-6 {
        0.0
    } else {
        n
    }
}

/// Espelho de `normalizeScene3DConfig` (prism.ts) para o motor: config sempre sai em schema v3
/// com `objects[]` (≥1) e `backgroundMode`. Schema ≤2: modelo único legado → `obj-primary`
/// (escala uniforme `modelScale`, `position[1] = modelOffsetY`, giro 0,22→0, playback/texturas
/// dinâmicas globais), fundo TRANSPARENT; chaves legadas removidas.
fn migrate_scene3d_v3(map: &mut Map<String, Value>) {
    let version = map.get("schemaVersion").and_then(Value::as_u64).unwrap_or(0);
    let migrating = version < SCENE3D_SCHEMA_VERSION;
    let has_objects = map
        .get("objects")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty());

    if has_objects {
        if let Some(objects) = map.get_mut("objects").and_then(Value::as_array_mut) {
            for (i, obj) in objects.iter_mut().filter_map(Value::as_object_mut).enumerate() {
                let id_ok = obj.get("id").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
                if !id_ok {
                    obj.insert("id".into(), json!(format!("obj-{i}")));
                }
                if migrating {
                    let spin = migrate_legacy_spin(obj.get("autoRotateSpeed"));
                    obj.insert("autoRotateSpeed".into(), json!(spin));
                }
            }
        }
    } else {
        let src = map.get("modelSrc").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let has_model = !src.is_empty();
        let scale = map
            .get("modelScale")
            .and_then(Value::as_f64)
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0);
        let offset_y = map
            .get("modelOffsetY")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0);
        let playback = map.get("playback").and_then(Value::as_object);
        let playback = json!({
            "clipName": playback.and_then(|p| p.get("clipName")).and_then(Value::as_str).unwrap_or(""),
            "speed": playback.and_then(|p| p.get("speed")).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(1.0),
            "loop": playback.and_then(|p| p.get("loop")).and_then(Value::as_bool).unwrap_or(true),
        });
        let mut obj = json!({
            "id": "obj-primary",
            "name": "Primary",
            "kind": if has_model { "GLB" } else { "SHOWCASE" },
            "modelSrc": src,
            "position": [0.0, offset_y, 0.0],
            "rotation": [0.0, 0.0, 0.0],
            "scale": [scale, scale, scale],
            "visible": true,
            "autoRotateSpeed": migrate_legacy_spin(map.get("autoRotateSpeed")),
            "playback": playback,
            "dynamicTextures": map.get("dynamicTextures").filter(|v| v.is_array()).cloned().unwrap_or_else(|| json!([])),
        });
        if !has_model {
            let variant = map.get("showcaseVariant").and_then(Value::as_str).unwrap_or("MONOLITH");
            obj["showcaseVariant"] = json!(variant);
        }
        map.insert("objects".into(), json!([obj]));
    }

    let mode_ok = matches!(
        map.get("backgroundMode").and_then(Value::as_str),
        Some("TRANSPARENT" | "SOLID" | "ENVIRONMENT")
    );
    if migrating || !mode_ok {
        map.insert("backgroundMode".into(), json!("TRANSPARENT"));
    }
    if !map.get("backgroundColor").is_some_and(Value::is_string) {
        map.insert("backgroundColor".into(), json!("#07080b"));
    }
    normalize_camera(map);
    for key in LEGACY_SCENE3D_KEYS {
        map.remove(key);
    }
    map.insert("schemaVersion".into(), json!(SCENE3D_SCHEMA_VERSION));
}

/// Espelho do ramo `camera` de `normalizeScene3DConfig`: projeto sem `camera` (ou campos
/// ausentes) mantém o enquadramento padrão do Estúdio (`defaultScene3D`: modelo externo 0°/8°,
/// escultura procedural 22°/14°); `target` sempre preenchido (`targetY` em sincronia).
fn normalize_camera(map: &mut Map<String, Value>) {
    let procedural = map
        .get("objects")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|o| o.get("kind"))
        .and_then(Value::as_str)
        == Some("SHOWCASE");
    let (az, el) = if procedural { (22.0, 14.0) } else { (0.0, 8.0) };
    let mut cam = match map.remove("camera") {
        Some(Value::Object(c)) => c,
        _ => Map::new(),
    };
    for (key, default) in [
        ("fov", 42.0),
        ("distance", 4.8),
        ("azimuthDeg", az),
        ("elevationDeg", el),
        ("orbitParallaxX", 12.0),
        ("orbitParallaxY", 7.0),
        ("damping", 0.08),
    ] {
        if !cam.get(key).and_then(Value::as_f64).is_some_and(f64::is_finite) {
            cam.insert(key.into(), json!(default));
        }
    }
    let target_y = number_in(&cam, "targetY", 0.0);
    let target = match cam.get("target").and_then(Value::as_array) {
        Some(t) if t.len() == 3 && t.iter().all(|v| v.as_f64().is_some_and(f64::is_finite)) => t.clone(),
        _ => vec![json!(0.0), json!(target_y), json!(0.0)],
    };
    cam.insert("targetY".into(), target[1].clone());
    cam.insert("target".into(), Value::Array(target));
    map.insert("camera".into(), Value::Object(cam));
}

fn number_in(map: &Map<String, Value>, key: &str, default: f64) -> f64 {
    map.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}

fn glass_plate(node: &mut Value, glass: &Value) {
    node["kind"] = json!("SHAPE");
    node["shape_kind"] = json!("RECT");
    node["fill"] = json!("OPTICAL_CRYSTAL");
    node["crystal"] = crystal(glass);
    node["corner_radius_px"] = json!(number(glass, "borderRadius", 24.0).max(0.0));
    let shadow = number(glass, "shadowBlur", 0.0);
    if shadow > 0.0 {
        node["shadow"] = json!({ "radius_px": shadow, "color_rgba": "#59000000", "dx_px": 0.0, "dy_px": shadow * 0.25 });
    }
}

/// `GlassEffect` → `OpticalCrystalParams`. Hipóteses (mesmas do `scene2d::glass_params`, no
/// sentido inverso): aberração cromática px = dispersão ×400; especular → fresnel; borda → aro.
fn crystal(g: &Value) -> Value {
    json!({
        "blur_px": number(g, "blurPx", 22.0).max(0.0),
        "refraction": number(g, "refractionScale", 16.0),
        "bevel_width": number(g, "bevelWidth", 22.0).clamp(0.0, 80.0),
        "dispersion_rgb": number(g, "chromaticAberration", 0.0).clamp(0.0, 6.0) / 400.0,
        "frost_amount": number(g, "frostOpacity", 0.08).clamp(0.0, 0.94),
        "fresnel_intensity": number(g, "specularIntensity", 0.45).max(0.0),
        "prismatic_rim_intensity": number(g, "borderOpacity", 0.16).max(0.0),
        "saturation": number(g, "saturation", 1.2).max(0.0),
        "tint_rgba": css_to_argb(non_empty(text(g, "tintColor"), "#FFFFFF"), GLASS_TINT_ALPHA),
        "grain": number(g, "noiseOpacity", 0.04).max(0.0),
    })
}

/// Fundo de mídia (`BackgroundSource`) de uma camada VIDEO/IMAGE com arquivo local.
fn media_background(layer: &Value) -> Option<Value> {
    match kind(layer) {
        "VIDEO" => {
            let v = layer.get("video")?;
            let path = local_path(text(v, "src"))?;
            Some(json!({ "type": "VIDEO", "path": path, "muted": boolean(v, "muted", true), "loop_video": true }))
        }
        "IMAGE" => {
            let img = layer.get("image")?;
            if text(img, "sourceMode").eq_ignore_ascii_case("MEDIA_COVER") {
                return None;
            }
            let path = local_path(text(img, "src"))?;
            Some(json!({ "type": "IMAGE", "path": path }))
        }
        _ => None,
    }
}

/// Cor/gradiente de fundo do projeto como retângulo de tela cheia (o `PrismScene` não tem cor de
/// fundo). Gradientes CSS viram fill `GRADIENT` (1ª e última parada; ângulo CSS; radial centrado).
fn background_fill(project: &Value, is_widget: bool, layers: &[Value]) -> Option<Value> {
    let raw = text(project, "backgroundColor").trim();
    if raw.eq_ignore_ascii_case("transparent") {
        return None;
    }
    let has_backdrop_layer = layers.iter().any(|l| {
        !boolean(l, "hidden", false)
            && (matches!(kind(l), "VIDEO" | "SCENE_3D") || (kind(l) == "IMAGE" && number(l, "width", 0.0) >= 1600.0))
    });
    let mut node = json!({
        "id": "__studio_background__",
        "name": "Fundo",
        "kind": "SHAPE",
        "shape_kind": "RECT",
        "fill": "SOLID",
        "corner_radius_px": 0.0,
        "locked": true,
        "width_px": FULLSCREEN_LOGICAL,
        "height_px": FULLSCREEN_LOGICAL,
    });
    match parse_css_gradient(raw) {
        Some(g) => {
            node["fill"] = json!("GRADIENT");
            node["color"] = json!(g.from);
            node["color2"] = json!(g.to);
            node["gradient_kind"] = json!(if g.radial { "RADIAL" } else { "LINEAR" });
            node["gradient_angle_deg"] = json!(g.angle_deg);
        }
        None => {
            // Mesma escolha do `effectiveBackgroundCss`.
            let color = if !raw.is_empty() && (has_backdrop_layer || !raw.eq_ignore_ascii_case("#000000")) {
                css_to_argb(raw, 1.0)
            } else if has_backdrop_layer {
                "#FF060608".to_string()
            } else {
                "#FF09090C".to_string()
            };
            node["color"] = json!(color);
        }
    }
    if !is_widget {
        node["fill_parent"] = json!(true);
    } else {
        let widget = project.get("widget").cloned().unwrap_or(Value::Null);
        node["width_px"] = json!(number(&widget, "width", 480.0));
        node["height_px"] = json!(number(&widget, "height", 240.0));
    }
    Some(node)
}

struct CssGradient {
    radial: bool,
    angle_deg: f64,
    from: String,
    to: String,
}

/// `linear-gradient(145deg, #a 0%, #b 100%)` / `radial-gradient(circle at …, #a, #b)`.
fn parse_css_gradient(raw: &str) -> Option<CssGradient> {
    let lower = raw.trim().to_ascii_lowercase();
    let radial = lower.starts_with("radial-gradient(");
    if !radial && !lower.starts_with("linear-gradient(") {
        return None;
    }
    let colors = css_colors_in(raw);
    let (first, last) = (colors.first()?, colors.last()?);
    let angle_deg = if radial {
        0.0
    } else {
        let inner = &lower[lower.find('(')? + 1..];
        let head = inner.split(',').next().unwrap_or("").trim();
        if let Some(deg) = head.strip_suffix("deg") {
            deg.trim().parse::<f64>().unwrap_or(180.0)
        } else {
            match head {
                "to top" => 0.0,
                "to right" => 90.0,
                "to left" => 270.0,
                "to bottom right" => 135.0,
                "to bottom left" => 225.0,
                "to top right" => 45.0,
                "to top left" => 315.0,
                _ => 180.0,
            }
        }
    };
    Some(CssGradient {
        radial,
        angle_deg,
        from: first.clone(),
        to: last.clone(),
    })
}

/// Cores `#hex` / `rgb()` / `rgba()` de um texto CSS, na ordem, já em `#AARRGGBB`.
fn css_colors_in(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let end = raw[i + 1..]
                .find(|c: char| !c.is_ascii_hexdigit())
                .map_or(raw.len(), |e| i + 1 + e);
            if matches!(end - i - 1, 3 | 4 | 6 | 8) {
                out.push(css_to_argb(&raw[i..end], 1.0));
            }
            i = end.max(i + 1);
            continue;
        }
        let rest = &raw[i..];
        let starts = |p: &str| rest.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p));
        if starts("rgb(") || starts("rgba(") {
            if let Some(close) = rest.find(')') {
                out.push(css_to_argb(&rest[..=close], 1.0));
                i += close + 1;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

// ───────────────────────── Assets ─────────────────────────

/// Resolve todos os caminhos de mídia da cena para arquivos absolutos existentes e os espelha em
/// `cache_dir` (hard link → cópia; vídeos gigantes sem hard link ficam no original). Relativos
/// são resolvidos contra `data_root` (`%APPDATA%\PrismView`). `.gltf` fica no original (recursos
/// externos relativos). Remove de `cache_dir` o que a cena não usa mais. Devolve avisos.
pub fn materialize_assets(scene: &mut Value, data_root: &Path, cache_dir: &Path) -> Vec<String> {
    let mut st = AssetState {
        data_root: data_root.to_path_buf(),
        cache_dir: cache_dir.to_path_buf(),
        used: HashSet::new(),
        warnings: Vec::new(),
    };
    let _ = fs::create_dir_all(cache_dir);
    if let Some(bg) = scene.get_mut("background") {
        if matches!(text(bg, "type"), "IMAGE" | "VIDEO") {
            st.field(bg, "path");
        }
    }
    if let Some(layers) = scene.get_mut("layers").and_then(Value::as_array_mut) {
        for layer in layers.iter_mut() {
            st.layer(layer);
        }
    }
    // Limpeza: arquivos espelhados de cenas anteriores.
    if let Ok(entries) = fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !st.used.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    st.warnings
}

struct AssetState {
    data_root: PathBuf,
    cache_dir: PathBuf,
    used: HashSet<String>,
    warnings: Vec<String>,
}

impl AssetState {
    fn layer(&mut self, layer: &mut Value) {
        // `asset` estático (ou fallback de FX) que não seja fórmula.
        match layer.get_mut("asset") {
            Some(Value::String(_)) => self.field(layer, "asset"),
            Some(Value::Object(fx)) => {
                if let Some(fb) = fx.get_mut("fallback") {
                    if let Some(p) = fb.as_str().and_then(|s| self.resolve(s)) {
                        *fb = Value::String(p);
                    }
                }
            }
            _ => {}
        }
        if let Some(t) = layer.get_mut("text") {
            self.field(t, "font_path");
        }
        if let Some(cfg) = layer.get_mut("scene3d") {
            self.field(cfg, "modelSrc");
            self.field(cfg, "envMapSrc");
            self.textures(cfg);
            if let Some(objects) = cfg.get_mut("objects").and_then(Value::as_array_mut) {
                for obj in objects.iter_mut() {
                    self.field(obj, "modelSrc");
                    self.textures(obj);
                }
            }
        }
        if let Some(children) = layer.get_mut("children").and_then(Value::as_array_mut) {
            for child in children.iter_mut() {
                self.layer(child);
            }
        }
    }

    fn textures(&mut self, owner: &mut Value) {
        if let Some(list) = owner.get_mut("dynamicTextures").and_then(Value::as_array_mut) {
            for tex in list.iter_mut() {
                self.field(tex, "customSrc");
            }
        }
    }

    fn field(&mut self, owner: &mut Value, key: &str) {
        let Some(raw) = owner.get(key).and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        if raw.trim().is_empty() || raw.contains('$') {
            return;
        }
        if let Some(resolved) = self.resolve(&raw) {
            owner[key] = Value::String(resolved);
        }
    }

    /// Caminho absoluto existente (espelhado no cache quando possível). `None` = deixa como está.
    fn resolve(&mut self, raw: &str) -> Option<String> {
        let local = local_path(raw)?;
        let mut path = PathBuf::from(&local);
        if !path.is_absolute() {
            path = self.data_root.join(&path);
        }
        let Ok(meta) = fs::metadata(&path) else {
            self.warnings.push(format!("arquivo ausente: {local}"));
            return None;
        };
        if !meta.is_file() {
            self.warnings.push(format!("não é arquivo: {local}"));
            return None;
        }
        let abs = fs::canonicalize(&path).map(strip_verbatim).unwrap_or(path);
        if abs.starts_with(&self.cache_dir) {
            if let Some(name) = abs.file_name() {
                self.used.insert(name.to_string_lossy().into_owned());
            }
            return Some(abs.to_string_lossy().into_owned());
        }
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext == "gltf" {
            return Some(abs.to_string_lossy().into_owned());
        }
        let name = format!(
            "{:016x}_{}",
            path_hash(&abs, &meta),
            abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        );
        let dest = self.cache_dir.join(&name);
        let ready = dest.is_file()
            || fs::hard_link(&abs, &dest).is_ok()
            || (meta.len() <= MAX_COPY_BYTES && fs::copy(&abs, &dest).is_ok());
        if ready {
            self.used.insert(name);
            Some(dest.to_string_lossy().into_owned())
        } else {
            Some(abs.to_string_lossy().into_owned())
        }
    }
}

/// `\\?\C:\…` (canonicalize no Windows) → `C:\…`.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

/// Identidade do arquivo (caminho + tamanho + mtime): muda quando o original muda.
fn path_hash(path: &Path, meta: &fs::Metadata) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut h);
    meta.len().hash(&mut h);
    if let Ok(modified) = meta.modified() {
        modified.hash(&mut h);
    }
    h.finish()
}

fn convert_global(g: &Value) -> Option<Value> {
    let name = text(g, "name").trim();
    if name.is_empty() {
        return None;
    }
    let kind = match text(g, "kind") {
        k @ ("SWITCH" | "NUMBER" | "TEXT" | "COLOR" | "LIST") => k,
        _ => "TEXT",
    };
    Some(json!({
        "name": name,
        "kind": kind,
        "value": text(g, "value"),
        "label": non_empty(text(g, "label"), name),
        "exposed": boolean(g, "exposed", false),
    }))
}

fn blend(css: &str) -> &'static str {
    match css {
        "screen" => "SCREEN",
        "multiply" => "MULTIPLY",
        "overlay" => "OVERLAY",
        "lighten" => "LIGHTEN",
        "darken" => "DARKEN",
        "soft-light" => "SOFT_LIGHT",
        "difference" => "DIFFERENCE",
        "plus-lighter" => "PLUS",
        _ => "NORMAL",
    }
}

// ───────────────────────── FX ─────────────────────────

fn fx_expr<'a>(layer: &'a Value, key: &str) -> Option<&'a str> {
    layer
        .get("fx")
        .and_then(|fx| fx.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|e| !e.is_empty())
}

/// Número estático ou `{fallback, expr}`. O Estúdio descarta `%` do resultado; aqui sai da
/// expressão (`$sys(cpu)$%` → `$sys(cpu)$`).
fn fx_num(layer: &Value, key: &str, fallback: f64) -> Value {
    match fx_expr(layer, key) {
        Some(expr) => {
            let expr = expr.trim_end_matches('%').trim();
            json!({ "fallback": fallback, "expr": expr })
        }
        None => json!(fallback),
    }
}

fn fx_str(layer: &Value, key: &str, fallback: String) -> Value {
    match fx_expr(layer, key) {
        Some(expr) => json!({ "fallback": fallback, "expr": expr }),
        None => json!(fallback),
    }
}

fn fx_bool(layer: &Value, key: &str) -> Value {
    match fx_expr(layer, key) {
        Some(expr) => json!({ "fallback": true, "expr": expr }),
        None => json!(true),
    }
}

// ───────────────────────── Cores ─────────────────────────

/// Cor CSS (`#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()/rgba()`, `transparent`) × `alpha` → `#AARRGGBB`.
fn css_to_argb(raw: &str, alpha: f64) -> String {
    let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
    let (r, g, b, a) = parse_css_color(raw.trim()).unwrap_or((255, 255, 255, 1.0));
    let a = ((a * alpha) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{a:02X}{r:02X}{g:02X}{b:02X}")
}

fn parse_css_color(raw: &str) -> Option<(u8, u8, u8, f64)> {
    if raw.eq_ignore_ascii_case("transparent") {
        return Some((0, 0, 0, 0.0));
    }
    if let Some(hex) = raw.strip_prefix('#') {
        let nib = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok().map(|v| v * 17);
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 => Some((nib(0)?, nib(1)?, nib(2)?, 1.0)),
            4 => Some((nib(0)?, nib(1)?, nib(2)?, f64::from(nib(3)?) / 255.0)),
            6 => Some((byte(0)?, byte(2)?, byte(4)?, 1.0)),
            8 => Some((byte(0)?, byte(2)?, byte(4)?, f64::from(byte(6)?) / 255.0)),
            _ => None,
        };
    }
    let lower = raw.to_ascii_lowercase();
    let inner = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f64> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<f64>().ok())
        .collect();
    let channel = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    match parts.as_slice() {
        [r, g, b] => Some((channel(*r), channel(*g), channel(*b), 1.0)),
        [r, g, b, a, ..] => Some((channel(*r), channel(*g), channel(*b), a.clamp(0.0, 1.0))),
        _ => None,
    }
}

// ───────────────────────── Caminhos ─────────────────────────

/// `src` do Estúdio → caminho local. Aceita caminho puro, `file://`, `asset://localhost/` e
/// `http(s)://asset.localhost/` (o `convertFileSrc` do Tauri). `data:`/`blob:`/web = `None`.
fn local_path(src: &str) -> Option<String> {
    let src = src.trim();
    if src.is_empty() || src.starts_with("data:") || src.starts_with("blob:") {
        return None;
    }
    for prefix in [
        "asset://localhost/",
        "http://asset.localhost/",
        "https://asset.localhost/",
    ] {
        if let Some(rest) = src.strip_prefix(prefix) {
            return Some(percent_decode(rest));
        }
    }
    if let Some(rest) = src.strip_prefix("file:///") {
        return Some(percent_decode(rest));
    }
    if src.starts_with("http://") || src.starts_with("https://") {
        return None;
    }
    Some(src.to_string())
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn normalize_path_field(map: &mut Map<String, Value>, key: &str) {
    let Some(raw) = map.get(key).and_then(Value::as_str) else {
        return;
    };
    if let Some(path) = local_path(raw) {
        if path != raw {
            map.insert(key.to_string(), Value::String(path));
        }
    }
}

fn normalize_dynamic_textures(map: &mut Map<String, Value>) {
    if let Some(list) = map.get_mut("dynamicTextures").and_then(Value::as_array_mut) {
        for tex in list.iter_mut().filter_map(Value::as_object_mut) {
            normalize_path_field(tex, "customSrc");
        }
    }
}

// ───────────────────────── SVG ─────────────────────────

/// Atributos `d` dos `<path>` de um SVG próprio (sem parser XML: só o necessário).
fn svg_paths(markup: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = markup;
    while let Some(start) = rest.find("<path") {
        rest = &rest[start + 5..];
        let end = rest.find('>').unwrap_or(rest.len());
        if let Some(d) = attr(&rest[..end], "d") {
            if !d.trim().is_empty() {
                out.push(d.to_string());
            }
        }
        rest = &rest[end..];
    }
    out
}

fn svg_viewbox(markup: &str) -> Option<(f64, f64)> {
    let start = markup.find("<svg")?;
    let tag_end = markup[start..].find('>').map_or(markup.len(), |e| start + e);
    let vb = attr(&markup[start..tag_end], "viewBox")?;
    let nums: Vec<f64> = vb
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(|p| p.parse().ok())
        .collect();
    match nums.as_slice() {
        [_, _, w, h] if *w > 0.0 && *h > 0.0 => Some((*w, *h)),
        _ => None,
    }
}

/// Valor de `name="..."` (ou `'...'`) dentro de uma tag, com fronteira de atributo.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut search = 0;
    while let Some(pos) = tag[search..].find(name) {
        let at = search + pos;
        let before_ok = at == 0 || tag.as_bytes()[at - 1].is_ascii_whitespace();
        let after = tag[at + name.len()..].trim_start();
        if before_ok {
            if let Some(after_eq) = after.strip_prefix('=') {
                let after_eq = after_eq.trim_start();
                let quote = after_eq.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let body = &after_eq[1..];
                    let end = body.find(quote)?;
                    return Some(&body[..end]);
                }
            }
        }
        search = at + name.len();
    }
    None
}

// ───────────────────────── Acesso ao JSON ─────────────────────────

fn kind(layer: &Value) -> &str {
    text(layer, "kind")
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn number(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(default)
}

fn boolean(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn non_empty<'a>(s: &'a str, default: &'a str) -> &'a str {
    if s.trim().is_empty() {
        default
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(id: &str, kind: &str, extra: Value) -> Value {
        let mut base = json!({
            "id": id, "name": id, "kind": kind, "x": 0.5, "y": 0.5, "width": 400, "height": 200,
            "rotation": 0, "opacity": 1, "blendMode": "normal", "hidden": false, "locked": false,
            "parallaxX": 0, "parallaxY": 0, "glass": { "enabled": false }, "fx": {}
        });
        if let (Some(dst), Some(src)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        base
    }

    #[test]
    fn css_colors() {
        assert_eq!(css_to_argb("#fff", 1.0), "#FFFFFFFF");
        assert_eq!(css_to_argb("#102030", 0.5), "#80102030");
        assert_eq!(css_to_argb("rgba(255, 0, 0, 0.5)", 1.0), "#80FF0000");
        assert_eq!(css_to_argb("transparent", 1.0), "#00000000");
    }

    #[test]
    fn css_gradient_background() {
        let g = parse_css_gradient("linear-gradient(145deg, #16171C 0%, #0B0C10 55%, #060608 100%)")
            .expect("gradiente");
        assert!(!g.radial);
        assert_eq!(g.angle_deg, 145.0);
        assert_eq!(g.from, "#FF16171C");
        assert_eq!(g.to, "#FF060608");
        let r = parse_css_gradient("radial-gradient(circle at 50% 18%, #1A1B21 0%, #050507 100%)")
            .expect("radial");
        assert!(r.radial);
        assert!(parse_css_gradient("#000").is_none());
    }

    #[test]
    fn asset_urls() {
        assert_eq!(
            local_path("http://asset.localhost/C%3A%5CUsers%5Ca%20b.png").as_deref(),
            Some("C:\Users\a b.png")
        );
        assert_eq!(local_path("blob:xyz"), None);
        assert_eq!(local_path("D:\m.glb").as_deref(), Some("D:\m.glb"));
    }

    #[test]
    fn clock_glass_and_3d() {
        let project = json!({
            "id": "p1", "title": "T", "packageType": "WALLPAPER", "dim": 0.1,
            "backgroundColor": "#000000", "widget": { "width": 480, "height": 240, "posX": 0, "posY": 0, "locked": false },
            "globals": [{ "name": "show", "kind": "SWITCH", "value": "1", "label": "", "exposed": true }],
            "layers": [
                layer("bg", "IMAGE", json!({ "width": 1920, "height": 1080,
                    "image": { "src": "C:\bg.jpg", "objectFit": "cover", "borderRadius": 0 } })),
                layer("clock", "TEXT", json!({ "y": 0.4, "parallaxX": 0.5,
                    "glass": { "enabled": true, "blurPx": 20, "tintColor": "#FFFFFF" },
                    "text": { "content": "$df(HH:mm)$", "fontFamily": "Minha", "fontPath": "C:\f.ttf", "fontSize": 180,
                              "fontWeight": 700, "letterSpacing": 0, "align": "center", "color": "#FFFFFF",
                              "stemStretch": 1, "shadowBlur": 0 },
                    "fx": { "opacity": "$gv(show)$" } })),
                layer("room", "SCENE_3D", json!({ "width": 1920, "height": 1080,
                    "scene3d": { "modelSrc": "http://asset.localhost/D%3A%5Cm.glb", "envPreset": "SUNSET",
                                 "schemaVersion": 2, "modelScale": 2, "autoRotateSpeed": 0.22 } }))
            ]
        });
        let scene = project_to_scene(&project).expect("convertível");
        assert_eq!(scene["background"]["type"], "IMAGE");
        let layers = scene["layers"].as_array().expect("layers");
        assert_eq!(layers[0]["kind"], "TEXT");
        assert_eq!(layers[0]["fill"], "OPTICAL_CRYSTAL");
        assert_eq!(layers[0]["text"]["font_path"], "C:\f.ttf");
        assert_eq!(layers[0]["opacity"]["expr"], "$gv(show)$");
        assert_eq!(layers[1]["kind"], "SCENE3D");
        assert_eq!(layers[1]["fill_parent"], true);
        let s3d = &layers[1]["scene3d"];
        assert_eq!(s3d["schemaVersion"], 3);
        assert_eq!(s3d["backgroundMode"], "TRANSPARENT");
        assert!(s3d.get("modelSrc").is_none());
        assert_eq!(s3d["objects"][0]["id"], "obj-primary");
        assert_eq!(s3d["objects"][0]["kind"], "GLB");
        assert_eq!(s3d["objects"][0]["modelSrc"], "D:\m.glb");
        assert_eq!(s3d["objects"][0]["scale"][1], 2.0);
        assert_eq!(s3d["objects"][0]["autoRotateSpeed"], 0.0);
        let b = &scene["bindings"][0];
        assert_eq!(b["source"], "CURSOR_X");
        assert_eq!(b["layerId"], "clock");
        assert_eq!(b["strength"], 18.0);
    }

    #[test]
    fn video_svg_and_actions_never_block() {
        let project = json!({
            "id": "p", "packageType": "WALLPAPER", "layers": [
                layer("v", "VIDEO", json!({ "width": 1920, "height": 1080, "video": { "src": "C:\v.mp4", "muted": true } })),
                layer("clip", "VIDEO", json!({ "video": { "src": "C:\c.mp4", "clipRadius": 12 } })),
                layer("play", "SVG", json!({ "clickAction": "MEDIA_PLAY_PAUSE",
                    "svg": { "iconName": "play", "customSvg": "", "color": "#FF0000", "strokeWidth": 1.6,
                             "resolvedSvg": "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\"><polygon points=\"6 4 20 12 6 20\"/></svg>",
                             "resolvedSvgAlt": "<svg viewBox=\"0 0 24 24\"><rect x=\"6\" y=\"4\" width=\"4\" height=\"16\" fill=\"currentColor\"/></svg>" } })),
                layer("link", "GLASS", json!({ "clickAction": "OPEN_URL", "clickUrl": "https://example.com",
                    "glass": { "enabled": true } }))
            ]
        });
        let scene = project_to_scene(&project).expect("nunca bloqueia");
        assert_eq!(scene["background"]["type"], "VIDEO");
        let layers = scene["layers"].as_array().expect("layers");
        assert_eq!(layers[0]["kind"], "VIDEO");
        assert_eq!(layers[0]["asset"], "C:\c.mp4");
        assert_eq!(layers[1]["kind"], "SVG");
        assert_eq!(layers[2]["id"], "play__playing");
        assert_eq!(layers[2]["visible"]["expr"], "$mi(playing)$");
        let markup = layers[1]["svg_markup"].as_str().expect("markup");
        assert!(markup.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert_eq!(layers[1]["color"], "#FFFF0000");
        assert_eq!(layers[3]["touch"]["action_type"], "OPEN_URL");
        assert_eq!(layers[3]["touch"]["url"], "https://example.com");
    }
}
