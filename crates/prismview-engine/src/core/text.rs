//! Texto nativo (Fase 1): glyphon + cosmic-text rasterizando na GPU.
//!
//! Modelo: cada camada de texto tem um *slot* com `Buffer` (layout/shaping do cosmic-text) e uma
//! textura `Rgba8Unorm` própria onde os glifos são desenhados **em branco, alpha pré-multiplicado**.
//! O compositor 2D desenha essa textura como máscara (`QuadKind::Mask`) com cor/gradiente,
//! opacidade, rotação e escala da camada, então o texto herda todo o pipeline 2D sem que o
//! glyphon precise saber de transformações.
//!
//! Dirty-flag: `update` compara o `TextSpec` com a última chave rasterizada; se nada mudou
//! (relógio no mesmo minuto, mesma fonte/tamanho), retorna o layout em cache sem shaping nem
//! raster. Só texto alterado é re-rasterizado, e a textura do slot é reaproveitada enquanto o novo
//! texto couber nela (cresce em passos; encolhe só se sobrar 4× de área).
//!
//! Fontes: `register_font_dir/file/data` carregam as fontes do projeto (`%APPDATA%\PrismView\fonts`,
//! pacotes `.prism`). O banco começa **vazio** (RAM mínima); fontes do sistema são carregadas
//! **sob demanda e apenas para a família pedida** via `FontResolver` injetado pela plataforma
//! (ex.: lookup pontual no Registro do Windows), sem nunca varrer `C:\Windows\Fonts`.
//!
//! Limitações atuais (cosmic-text 0.12): sem eixos de fonte variável (`axes`); `(weight,
//! stretch, style)` são ajustados para a face real mais próxima da família para que fontes
//! condensadas (ex.: `Impact`) ou pesos intermediários (ex.: `500`) nunca sejam rejeitados pelo
//! matcher estrito do cosmic-text 0.12. `letter_spacing_em` é aplicado por glifo (paridade 1:1
//! com CSS `letter-spacing` e `glassLens.ts`). Glifos coloridos (emoji) saem como máscara.
//!
//! Regras: sem `unwrap/expect`; nenhum código de SO (resolução de caminhos vem do chamador).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cosmic_text::{
    fontdb, Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight,
    Wrap,
};
use super::stem_text::{self, GlyphSource, StemCache};
use glyphon::{Cache, ColorMode, Resolution, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};

/// Callback de plataforma que resolve apenas os arquivos de fonte (`.ttf/.otf/.ttc/.otc`) de uma
/// família específica, sem carregar todas as fontes do sistema operacional.
pub type FontResolver = Box<dyn Fn(&str) -> Vec<PathBuf> + Send>;

/// Formato das texturas dos slots. Sem sRGB: o pipeline inteiro trabalha na codificação da tela.
pub const TEXT_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const TEXT_ATLAS_LOCALE: &str = "en-US";
/// Famílias genéricas tentadas (em ordem) quando as fontes do sistema são carregadas.
const SANS_FALLBACKS: [&str; 5] = ["Segoe UI", "Arial", "Helvetica", "Noto Sans", "DejaVu Sans"];
const MIN_FONT_PX: f32 = 1.0;
const MAX_FONT_PX: f32 = 4096.0;

// ───────────────────────── Tipos públicos ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlignment {
    Start,
    #[default]
    Center,
    End,
}

/// Descrição do texto a rasterizar. Tamanhos em **pixels físicos** (já com a escala de DPI).
#[derive(Clone, Copy, Debug)]
pub struct TextSpec<'a> {
    pub text: &'a str,
    pub font_id: &'a str,
    pub size_px: f32,
    pub weight: u16,
    /// Espaço extra entre linhas, como fração do tamanho (0 = entrelinha padrão 1.06× do Estúdio).
    pub line_spacing: f32,
    /// Espaçamento entre letras em `em` (paridade 1:1 com CSS `letter-spacing`).
    pub letter_spacing_em: f32,
    pub align: TextAlignment,
    /// Largura máxima em px físicos (0 = largura automática sem quebra de linha).
    pub max_width_px: f32,
    /// Alongamento por haste (1 = normal). >1 troca a rasterização por contornos vetoriais.
    pub stretch: f32,
    /// Compensação de espessura do alongamento (0..1).
    pub stem_compensation: f32,
}

/// Resultado do layout de um slot.
#[derive(Clone, Copy, Debug)]
pub struct TextLayout {
    /// Caixa do texto (px físicos), usada para ancorar/centralizar a camada.
    pub width: f32,
    pub height: f32,
    /// Região útil da textura (a caixa + margem), em px: tamanho do quad a desenhar.
    pub quad_width: f32,
    pub quad_height: f32,
    /// Fração da textura ocupada (`quad / capacidade`), para o `uv_rect` = `[0, 0, u, v]`.
    pub uv_max: [f32; 2],
    /// Muda quando a textura foi (re)alocada: o compositor deve refazer o bind group.
    pub tex_gen: u64,
    /// Sem nada a desenhar (texto vazio): não há textura.
    pub empty: bool,
    /// A textura já vem alongada (altura × stretch): o compositor NÃO deve esticar de novo.
    pub stretched: bool,
}

impl TextLayout {
    const EMPTY: Self = Self {
        width: 0.0,
        height: 0.0,
        quad_width: 0.0,
        quad_height: 0.0,
        uv_max: [0.0, 0.0],
        tex_gen: 0,
        empty: true,
        stretched: false,
    };
}

// ───────────────────────── Slot ─────────────────────────

/// Última especificação rasterizada. Strings reaproveitadas (`clear` + `push_str`).
#[derive(Default)]
struct TextKey {
    text: String,
    font_id: String,
    size_bits: u32,
    weight: u16,
    spacing_bits: u32,
    letter_spacing_bits: u32,
    align: TextAlignment,
    max_width_bits: u32,
    stretch_bits: u32,
    comp_bits: u32,
}

impl TextKey {
    fn matches(&self, spec: &TextSpec<'_>) -> bool {
        self.size_bits == spec.size_px.to_bits()
            && self.weight == spec.weight
            && self.spacing_bits == spec.line_spacing.to_bits()
            && self.letter_spacing_bits == spec.letter_spacing_em.to_bits()
            && self.align == spec.align
            && self.max_width_bits == spec.max_width_px.to_bits()
            && self.stretch_bits == spec.stretch.to_bits()
            && self.comp_bits == spec.stem_compensation.to_bits()
            && self.text == spec.text
            && self.font_id == spec.font_id
    }

    fn store(&mut self, spec: &TextSpec<'_>) {
        self.text.clear();
        self.text.push_str(spec.text);
        self.font_id.clear();
        self.font_id.push_str(spec.font_id);
        self.size_bits = spec.size_px.to_bits();
        self.weight = spec.weight;
        self.spacing_bits = spec.line_spacing.to_bits();
        self.letter_spacing_bits = spec.letter_spacing_em.to_bits();
        self.align = spec.align;
        self.max_width_bits = spec.max_width_px.to_bits();
        self.stretch_bits = spec.stretch.to_bits();
        self.comp_bits = spec.stem_compensation.to_bits();
    }
}

struct SlotTexture {
    /// Mantém a textura viva (a view a referencia internamente, mas o dono é o slot).
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    cap: (u32, u32),
}

struct TextSlot {
    buffer: Buffer,
    /// Preenchido apenas quando `|letter_spacing_em| >= 0.001`: `(buffer_glifo, x, y)`.
    spaced_glyphs: Vec<(Buffer, f32, f32)>,
    key: TextKey,
    font_gen: u64,
    valid: bool,
    layout: TextLayout,
    pad: u32,
    texture: Option<SlotTexture>,
    tex_gen: u64,
    needs_raster: bool,
    used_serial: u64,
    /// Alongamento aplicado (>1 = textura vinda de `stem_text`).
    stretch: f32,
    stem_comp: f32,
    weight: u16,
}

// ───────────────────────── TextSystem ─────────────────────────

pub struct TextSystem {
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: TextRenderer,
    /// `font_id` → nome real da família no banco (`None` = usar a sans-serif genérica).
    families: HashMap<String, Option<String>>,
    /// `font_id` → família do arquivo de fonte do projeto (`TextContent::font_path`). Sobrevive a
    /// `fonts_changed` (é a origem, não um cache).
    aliases: HashMap<String, String>,
    /// Incrementa quando o banco de fontes muda: todo slot refaz o shaping.
    font_generation: u64,
    /// Resolução pontual de fontes do SO (injetada pela plataforma).
    resolver: Option<FontResolver>,
    /// Famílias normalizadas já consultadas no `resolver`.
    resolved_queries: HashSet<String>,
    /// Caminhos já carregados no banco (evita duplicar faces).
    loaded_files: HashSet<String>,
    /// Fallback sans-serif leve (`Segoe UI` / `Arial`) já inicializado.
    fallback_loaded: bool,
    slots: HashMap<String, TextSlot>,
    stem_cache: StemCache,
    serial: u64,
    max_texture_dim: u32,
}

impl TextSystem {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let font_system =
            FontSystem::new_with_locale_and_db(TEXT_ATLAS_LOCALE.to_string(), fontdb::Database::new());
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        // `Web`: valores já na codificação da tela, em textura linear (sem conversão de gama).
        let mut atlas =
            TextAtlas::with_color_mode(device, queue, &cache, TEXT_TEXTURE_FORMAT, ColorMode::Web);
        let renderer = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        Self {
            font_system,
            swash: SwashCache::new(),
            atlas,
            viewport,
            renderer,
            families: HashMap::new(),
            aliases: HashMap::new(),
            font_generation: 0,
            resolver: None,
            resolved_queries: HashSet::new(),
            loaded_files: HashSet::new(),
            fallback_loaded: false,
            slots: HashMap::new(),
            stem_cache: StemCache::default(),
            serial: 0,
            max_texture_dim: device.limits().max_texture_dimension_2d,
        }
    }

    // ── Fontes ──

    /// Instala o resolvedor pontual da plataforma (ex.: consulta ao Registro do Windows).
    pub fn set_font_resolver(&mut self, resolver: FontResolver) {
        self.resolver = Some(resolver);
        self.resolved_queries.clear();
        self.fallback_loaded = false;
        self.fonts_changed();
    }

    /// Carrega todas as fontes (`.ttf/.otf/.ttc`) de `dir`, recursivamente.
    pub fn register_font_dir(&mut self, dir: &Path) {
        self.font_system.db_mut().load_fonts_dir(dir);
        for face in self.font_system.db().faces() {
            if let fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) = &face.source {
                self.loaded_files.insert(p.to_string_lossy().to_ascii_lowercase());
            }
        }
        self.fonts_changed();
    }

    fn load_font_file_dedup(&mut self, path: &Path) -> bool {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if self.loaded_files.contains(&key) {
            return false;
        }
        if self.font_system.db_mut().load_font_file(path).is_ok() {
            self.loaded_files.insert(key);
            true
        } else {
            false
        }
    }

    /// Carrega uma fonte de arquivo. `false` se não foi possível ler.
    pub fn register_font_file(&mut self, path: &Path) -> bool {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if self.loaded_files.contains(&key) {
            return true;
        }
        let ok = self.load_font_file_dedup(path);
        if ok {
            self.fonts_changed();
        }
        ok
    }

    /// Carrega uma fonte de memória (ex.: extraída de um `.prism`).
    pub fn register_font_data(&mut self, data: Vec<u8>) {
        self.font_system.db_mut().load_font_data(data);
        self.fonts_changed();
    }

    /// Registra o arquivo `path` (se ainda não estiver no banco) e faz `font_id` resolver para a
    /// família contida nele — o nome que o Estúdio dá à fonte importada não precisa coincidir
    /// com o nome interno do arquivo. `false` se o arquivo não pôde ser lido.
    pub fn register_font_alias(&mut self, font_id: &str, path: &Path) -> bool {
        let mut family = self.family_of_file(path);
        if family.is_none() && self.load_font_file_dedup(path) {
            family = self.family_of_file(path);
        }
        let Some(family) = family else {
            return false;
        };
        if self.aliases.get(font_id) != Some(&family) {
            self.aliases.insert(font_id.to_owned(), family);
            self.fonts_changed();
        }
        true
    }

    fn family_of_file(&self, path: &Path) -> Option<String> {
        let wanted = path.to_string_lossy();
        self.font_system.db().faces().find_map(|face| {
            let src = match &face.source {
                fontdb::Source::File(p) => p.as_path(),
                fontdb::Source::SharedFile(p, _) => p.as_path(),
                _ => return None,
            };
            if src == path || src.to_string_lossy().eq_ignore_ascii_case(&wanted) {
                face.families.first().map(|(name, _)| name.clone())
            } else {
                None
            }
        })
    }

    fn fonts_changed(&mut self) {
        self.families.clear();
        self.font_generation += 1;
    }

    /// Garante 1 família sans-serif leve (ex.: `Segoe UI` sem Emoji/Symbol) como fallback para
    /// famílias ausentes e separadores (`·`, `•`), sem nunca chamar `db.load_system_fonts()`.
    fn ensure_fallback_font(&mut self) {
        if self.fallback_loaded {
            return;
        }
        self.fallback_loaded = true;
        if let Some(existing) = SANS_FALLBACKS
            .iter()
            .find_map(|name| find_family(self.font_system.db(), name))
        {
            self.font_system.db_mut().set_sans_serif_family(existing.as_str());
            return;
        }
        let mut paths = Vec::new();
        if let Some(resolver) = self.resolver.as_ref() {
            for name in SANS_FALLBACKS {
                let found = resolver(name);
                if !found.is_empty() {
                    paths = found;
                    break;
                }
            }
        }
        let mut loaded_any = false;
        for path in paths {
            if self.load_font_file_dedup(&path) {
                loaded_any = true;
            }
        }
        if let Some(sans) = SANS_FALLBACKS
            .iter()
            .find_map(|name| find_family(self.font_system.db(), name))
        {
            self.font_system.db_mut().set_sans_serif_family(sans.as_str());
        }
        if loaded_any {
            self.fonts_changed();
        }
    }

    /// Nome de família no banco para `font_id`, ou `None` (sans-serif genérica).
    /// Busca por alias, no banco local e, se faltar, consulta o `resolver` pontual da plataforma.
    fn resolve_family(&mut self, font_id: &str) -> Option<String> {
        self.ensure_fallback_font();
        if let Some(hit) = self.families.get(font_id) {
            return hit.clone();
        }
        if let Some(alias) = self.aliases.get(font_id) {
            if let Some(found) = find_family(self.font_system.db(), alias) {
                self.families.insert(font_id.to_owned(), Some(found.clone()));
                return Some(found);
            }
        }
        let primary = primary_font_name(font_id);
        if let Some(alias) = self.aliases.get(primary) {
            if let Some(found) = find_family(self.font_system.db(), alias) {
                self.families.insert(font_id.to_owned(), Some(found.clone()));
                return Some(found);
            }
        }
        let mut found = find_family(self.font_system.db(), primary);
        if found.is_none() {
            let needle = normalize(primary);
            if !needle.is_empty() && self.resolved_queries.insert(needle) {
                let paths = self
                    .resolver
                    .as_ref()
                    .map(|r| r(primary))
                    .unwrap_or_default();
                let mut loaded_any = false;
                for path in &paths {
                    if self.load_font_file_dedup(path) {
                        loaded_any = true;
                    }
                }
                if loaded_any {
                    self.fonts_changed();
                }
                found = find_family(self.font_system.db(), primary)
                    .or_else(|| paths.iter().find_map(|p| self.family_of_file(p)));
                // Persiste o alias (sobrevive a `fonts_changed()` quando outra camada carregar outra fonte).
                if let Some(ref resolved_name) = found {
                    if normalize(resolved_name) != normalize(primary) {
                        self.aliases.insert(primary.to_owned(), resolved_name.clone());
                        self.aliases.insert(font_id.to_owned(), resolved_name.clone());
                    }
                }
            }
        }
        self.families.insert(font_id.to_owned(), found.clone());
        found
    }

    // ── Layout / raster ──

    /// Marca o início de uma reconstrução de cena e devolve o número da passada: slots tocados
    /// por `update` recebem esse número.
    pub fn begin_pass(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// Libera os slots (e texturas) que `belongs` reconhece como desta saída e que não foram
    /// usados na passada `serial`. Slots de outras saídas (outros monitores) ficam intactos.
    pub fn sweep(&mut self, serial: u64, belongs: impl Fn(&str) -> bool) {
        self.slots
            .retain(|key, slot| !belongs(key) || slot.used_serial == serial);
    }

    pub fn has_slot(&self, id: &str) -> bool {
        self.slots.contains_key(id)
    }

    /// Descarta todos os slots (hibernação de VRAM / troca de cena).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.stem_cache = StemCache::default();
        self.atlas.trim();
        self.swash = SwashCache::new();
    }

    /// Faz o layout do slot `id` para `spec`. Sem mudança desde a última chamada devolve o layout
    /// em cache (sem shaping, sem alocar). Mudou: refaz shaping, ajusta a textura e marca o slot
    /// para `flush`.
    pub fn update(&mut self, device: &wgpu::Device, id: &str, spec: &TextSpec<'_>) -> TextLayout {
        let serial = self.serial;

        if !self.slots.contains_key(id) {
            let buffer = Buffer::new(&mut self.font_system, Metrics::new(16.0, 20.0));
            self.slots.insert(
                id.to_owned(),
                TextSlot {
                    buffer,
                    spaced_glyphs: Vec::new(),
                    key: TextKey::default(),
                    font_gen: u64::MAX,
                    valid: false,
                    layout: TextLayout::EMPTY,
                    pad: 0,
                    texture: None,
                    tex_gen: 0,
                    needs_raster: false,
                    used_serial: serial,
                    stretch: 1.0,
                    stem_comp: 1.0,
                    weight: 400,
                },
            );
        }

        if let Some(slot) = self.slots.get_mut(id) {
            slot.used_serial = serial;
            if slot.valid && slot.font_gen == self.font_generation && slot.key.matches(spec) {
                return slot.layout;
            }
        }

        // Pode carregar fontes do sistema (e mudar `font_generation`) antes do shaping.
        let family = self.resolve_family(spec.font_id);
        let font_gen = self.font_generation;
        let max_dim = self.max_texture_dim;

        let Some(slot) = self.slots.get_mut(id) else {
            return TextLayout::EMPTY;
        };
        let fs = &mut self.font_system;

        let size = if spec.size_px.is_finite() {
            spec.size_px.clamp(MIN_FONT_PX, MAX_FONT_PX)
        } else {
            MIN_FONT_PX
        };
        let spacing = if spec.line_spacing.is_finite() {
            spec.line_spacing.clamp(-0.5, 4.0)
        } else {
            0.0
        };
        // Paridade 1:1 com `UnifiedCompositor.tsx` (`lineHeight: 1.06`) e `glassLens.ts`.
        let line_height = (size * (1.06 + spacing)).max(1.0);
        let letter_spacing_em = if spec.letter_spacing_em.is_finite() {
            spec.letter_spacing_em.clamp(-0.5, 2.0)
        } else {
            0.0
        };
        let letter_spacing_px = letter_spacing_em * size;

        let (matched_family, matched_weight, matched_stretch, matched_style) =
            match_face_attrs(fs.db(), family.as_deref(), spec.weight);
        let family_ref = match matched_family.as_deref().or(family.as_deref()) {
            Some(name) => Family::Name(name),
            None => Family::SansSerif,
        };
        let attrs = Attrs::new()
            .family(family_ref)
            .weight(matched_weight)
            .stretch(matched_stretch)
            .style(matched_style)
            .color(Color::rgb(255, 255, 255));

        let wrap_width = if spec.max_width_px.is_finite() && spec.max_width_px >= 1.0 {
            Some(spec.max_width_px.min(max_dim as f32))
        } else {
            None
        };

        let (width, height) = shape_layout(
            fs,
            &mut slot.buffer,
            &mut slot.spaced_glyphs,
            &ShapeParams { spec, size, line_height, letter_spacing_px, attrs, wrap_width },
        );

        slot.key.store(spec);
        slot.font_gen = font_gen;
        slot.valid = true;

        if width < 0.5 {
            // Texto vazio: sem textura (e libera a que existia).
            slot.texture = None;
            slot.needs_raster = false;
            slot.layout = TextLayout {
                width: 0.0,
                height,
                ..TextLayout::EMPTY
            };
            return slot.layout;
        }

        let stretch = if spec.stretch.is_finite() { spec.stretch.clamp(1.0, 2.8) } else { 1.0 };
        let stretched = stretch > 1.004;
        slot.stretch = if stretched { stretch } else { 1.0 };
        slot.stem_comp = if spec.stem_compensation.is_finite() { spec.stem_compensation } else { 1.0 };
        slot.weight = spec.weight;
        // Altura da caixa já alongada: o bloco todo cresce `stretch`× a partir do topo.
        let height = if stretched { height * stretch } else { height };

        let pad = ((size * 0.08).ceil() as u32).max(2);
        slot.pad = pad;
        let need_w = (width.ceil() as u32).saturating_add(pad * 2).clamp(1, max_dim);
        let need_h = (height.ceil() as u32).saturating_add(pad * 2).clamp(1, max_dim);

        let realloc = match slot.texture.as_ref() {
            None => true,
            Some(t) => {
                t.cap.0 < need_w
                    || t.cap.1 < need_h
                    || u64::from(t.cap.0) * u64::from(t.cap.1)
                        > u64::from(need_w) * u64::from(need_h) * 4
            }
        };
        if realloc {
            let cap_w = round_up(need_w, 64).min(max_dim);
            let cap_h = round_up(need_h, 16).min(max_dim);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("prismview_text_slot"),
                size: wgpu::Extent3d {
                    width: cap_w,
                    height: cap_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TEXT_TEXTURE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            slot.texture = Some(SlotTexture {
                _texture: texture,
                view,
                cap: (cap_w, cap_h),
            });
            slot.tex_gen += 1;
        }

        let cap = slot.texture.as_ref().map_or((need_w, need_h), |t| t.cap);
        slot.layout = TextLayout {
            width,
            height,
            quad_width: need_w as f32,
            quad_height: need_h as f32,
            uv_max: [need_w as f32 / cap.0 as f32, need_h as f32 / cap.1 as f32],
            tex_gen: slot.tex_gen,
            empty: false,
            stretched,
        };
        slot.needs_raster = true;
        slot.layout
    }

    /// View da textura do slot (para `Compositor2D::bind_view`).
    pub fn texture_view(&self, id: &str) -> Option<&wgpu::TextureView> {
        self.slots
            .get(id)
            .and_then(|s| s.texture.as_ref())
            .map(|t| &t.view)
    }

    /// Há slots esperando raster?
    #[allow(dead_code)] // API reservada (ainda sem chamador)
    pub fn has_pending(&self) -> bool {
        self.slots.values().any(|s| s.needs_raster)
    }

    /// Rasteriza os slots alterados. Um submit por slot: `prepare` regrava o vertex buffer e o
    /// uniform de viewport compartilhados, e a ordem queue-write → submit garante que cada passe
    /// veja os seus dados. Roda só quando o texto mudou (tipicamente 1× por minuto/segundo).
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut any = false;
        for slot in self.slots.values_mut() {
            if !slot.needs_raster {
                continue;
            }
            slot.needs_raster = false;
            let Some(tex) = slot.texture.as_ref() else {
                continue;
            };
            any = true;

            // Alongamento por haste: contornos vetoriais → textura (CPU), sem passar pelo glyphon.
            if slot.stretch > 1.004 {
                let (qw, qh) = (
                    (slot.layout.quad_width as u32).min(tex.cap.0),
                    (slot.layout.quad_height as u32).min(tex.cap.1),
                );
                let raster = if slot.spaced_glyphs.is_empty() {
                    let srcs = [GlyphSource { buffer: &slot.buffer, ox: 0.0, oy: 0.0 }];
                    stem_text::rasterize(
                        &mut self.font_system,
                        &mut self.stem_cache,
                        &srcs,
                        slot.stretch,
                        slot.stem_comp,
                        slot.weight,
                        qw,
                        qh,
                        slot.pad as f32,
                    )
                } else {
                    let srcs: Vec<GlyphSource<'_>> = slot
                        .spaced_glyphs
                        .iter()
                        .map(|(b, gx, gy)| GlyphSource { buffer: b, ox: *gx, oy: *gy })
                        .collect();
                    stem_text::rasterize(
                        &mut self.font_system,
                        &mut self.stem_cache,
                        &srcs,
                        slot.stretch,
                        slot.stem_comp,
                        slot.weight,
                        qw,
                        qh,
                        slot.pad as f32,
                    )
                };
                if let Some(r) = raster {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &tex._texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &r.rgba,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(r.width * 4),
                            rows_per_image: Some(r.height),
                        },
                        wgpu::Extent3d { width: r.width, height: r.height, depth_or_array_layers: 1 },
                    );
                    continue;
                }
                // Sem contornos (fonte bitmap/emoji): cai no raster comum abaixo.
            }

            self.viewport.update(
                queue,
                Resolution {
                    width: tex.cap.0,
                    height: tex.cap.1,
                },
            );

            let bounds = TextBounds {
                left: 0,
                top: 0,
                right: i32::try_from(tex.cap.0).unwrap_or(i32::MAX),
                bottom: i32::try_from(tex.cap.1).unwrap_or(i32::MAX),
            };
            let mut prepared = false;
            for attempt in 0..2 {
                let result = if slot.spaced_glyphs.is_empty() {
                    let area = TextArea {
                        buffer: &slot.buffer,
                        left: slot.pad as f32,
                        top: slot.pad as f32,
                        scale: 1.0,
                        bounds,
                        default_color: Color::rgb(255, 255, 255),
                        custom_glyphs: &[],
                    };
                    self.renderer.prepare(
                        device,
                        queue,
                        &mut self.font_system,
                        &mut self.atlas,
                        &self.viewport,
                        [area],
                        &mut self.swash,
                    )
                } else {
                    let areas: Vec<TextArea<'_>> = slot
                        .spaced_glyphs
                        .iter()
                        .map(|(buf, gx, gy)| TextArea {
                            buffer: buf,
                            left: slot.pad as f32 + *gx,
                            top: slot.pad as f32 + *gy,
                            scale: 1.0,
                            bounds,
                            default_color: Color::rgb(255, 255, 255),
                            custom_glyphs: &[],
                        })
                        .collect();
                    self.renderer.prepare(
                        device,
                        queue,
                        &mut self.font_system,
                        &mut self.atlas,
                        &self.viewport,
                        areas,
                        &mut self.swash,
                    )
                };
                if result.is_ok() {
                    prepared = true;
                    break;
                }
                if attempt == 0 {
                    self.atlas.trim(); // atlas cheio: libera glifos não usados e tenta de novo
                }
            }

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismview_text_raster"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("prismview_text_raster_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &tex.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                if prepared {
                    // Erro de render (glifo removido do atlas): fica a textura transparente até a
                    // próxima mudança de texto.
                    let _ = self.renderer.render(&self.atlas, &self.viewport, &mut pass);
                }
            }
            queue.submit(std::iter::once(encoder.finish()));
        }
        if any {
            self.atlas.trim();
            // Glifos já rasterizados nas texturas GPU dos slots: solta bitmaps CPU do swash.
            self.swash = SwashCache::new();
        }
    }
}

// ───────────────────────── Helpers ─────────────────────────

fn round_up(value: u32, step: u32) -> u32 {
    value.div_ceil(step).saturating_mul(step)
}

/// Parâmetros já saneados do layout de um texto (tudo em px físicos).
struct ShapeParams<'a> {
    spec: &'a TextSpec<'a>,
    size: f32,
    line_height: f32,
    letter_spacing_px: f32,
    attrs: Attrs<'a>,
    wrap_width: Option<f32>,
}

/// Layout puro (sem GPU): quebra de linha, espaçamento e alinhamento. Devolve a caixa
/// `(largura, altura)` em px físicos. Separado de `TextSystem::update` para ser testável
/// sem `wgpu::Device` (paridade de caixa com o Estúdio).
fn shape_layout(
    fs: &mut FontSystem,
    buffer: &mut Buffer,
    spaced_glyphs: &mut Vec<(Buffer, f32, f32)>,
    p: &ShapeParams<'_>,
) -> (f32, f32) {
    let ShapeParams { spec, size, line_height, letter_spacing_px, attrs, wrap_width } = *p;
    let use_letter_spacing = letter_spacing_px.abs() >= 0.25;
    spaced_glyphs.clear();
    if use_letter_spacing {
        let metrics = Metrics::new(size, line_height);
        let mut per_line: Vec<(f32, Vec<(Buffer, f32)>)> = Vec::new();
        let mut max_w = wrap_width.unwrap_or(0.0);
        let mut char_buf = [0u8; 4];

        let measure_str = |fs: &mut FontSystem, text: &str| -> f32 {
            let mut cursor = 0.0f32;
            let mut w = 0.0f32;
            let mut cb = [0u8; 4];
            for ch in text.chars() {
                let s = ch.encode_utf8(&mut cb);
                let mut b = Buffer::new(fs, metrics);
                b.set_wrap(fs, Wrap::None);
                b.set_size(fs, None, None);
                b.set_text(fs, s, attrs, Shaping::Advanced);
                b.shape_until_scroll(fs, false);
                let (mut cw, _, _) = measure(&b);
                if cw <= 0.0 && ch.is_whitespace() {
                    cw = size * 0.28;
                }
                let x = cursor.max(0.0);
                w = w.max(x + cw);
                cursor = x + cw + letter_spacing_px;
            }
            w
        };

        let mut wrapped_lines: Vec<String> = Vec::new();
        if let Some(limit) = wrap_width {
            // Quebra só entre palavras (espaços), com a mesma tolerância do caminho sem
            // espaçamento. Palavra que não cabe sozinha fica inteira na linha e estende a
            // caixa (`max_w`), nunca é partida no meio.
            let fit_limit = limit + wrap_tolerance(limit, size);
            for para in spec.text.split('\n') {
                if para.is_empty() {
                    wrapped_lines.push(String::new());
                    continue;
                }
                let mut current = String::new();
                for word in para.split_inclusive(|c: char| c.is_whitespace()) {
                    if current.is_empty() {
                        current.push_str(word);
                        continue;
                    }
                    let mut cand = current.clone();
                    cand.push_str(word.trim_end());
                    if measure_str(fs, &cand) <= fit_limit {
                        current.push_str(word);
                    } else {
                        wrapped_lines.push(current.trim_end().to_string());
                        current.clear();
                        current.push_str(word);
                    }
                }
                if !current.is_empty() {
                    wrapped_lines.push(current.trim_end().to_string());
                }
            }
        } else {
            for line in spec.text.split('\n') {
                wrapped_lines.push(line.to_string());
            }
        }

        for line in &wrapped_lines {
            let mut cursor = 0.0f32;
            let mut line_w = 0.0f32;
            let mut items = Vec::new();
            for ch in line.chars() {
                let s = ch.encode_utf8(&mut char_buf);
                let mut b = Buffer::new(fs, metrics);
                b.set_wrap(fs, Wrap::None);
                b.set_size(fs, None, None);
                b.set_text(fs, s, attrs, Shaping::Advanced);
                b.shape_until_scroll(fs, false);
                let (mut cw, _, _) = measure(&b);
                if cw <= 0.0 && ch.is_whitespace() {
                    cw = size * 0.28;
                }
                let x = cursor.max(0.0);
                line_w = line_w.max(x + cw);
                if !ch.is_whitespace() {
                    items.push((b, x));
                }
                cursor = x + cw + letter_spacing_px;
            }
            // O CSS soma o `letter-spacing` também depois do último caractere da linha:
            // mesma largura de caixa e mesmo centro de alinhamento que o Estúdio.
            if !line.is_empty() {
                line_w = (line_w + letter_spacing_px).max(0.0);
            }
            max_w = max_w.max(line_w);
            per_line.push((line_w, items));
        }
        let total_h = (per_line.len().max(1) as f32) * line_height;
        for (li, (line_w, items)) in per_line.into_iter().enumerate() {
            let align_dx = match spec.align {
                TextAlignment::Start => 0.0,
                TextAlignment::Center => (max_w - line_w) * 0.5,
                TextAlignment::End => max_w - line_w,
            };
            let y = li as f32 * line_height;
            for (b, x) in items {
                spaced_glyphs.push((b, x + align_dx, y));
            }
        }
        (max_w, total_h)
    } else {
        buffer.set_metrics(fs, Metrics::new(size, line_height));
        let align = match spec.align {
            TextAlignment::Start => Align::Left,
            TextAlignment::Center => Align::Center,
            TextAlignment::End => Align::Right,
        };
        if let Some(limit) = wrap_width {
            // `Wrap::Word` (nunca no meio da palavra, ex. "00:06") com tolerância: as métricas
            // do motor saem um pouco maiores que as do navegador (sem `tnum`, arredondamentos).
            // Palavra que não cabe sozinha transborda a linha e estende a caixa.
            let layout_w = limit + wrap_tolerance(limit, size);
            buffer.set_wrap(fs, Wrap::Word);
            buffer.set_size(fs, Some(layout_w), None);
            buffer.set_text(fs, spec.text, attrs, Shaping::Advanced);
            for line in buffer.lines.iter_mut() {
                line.set_align(Some(align));
            }
            buffer.shape_until_scroll(fs, false);
            let (w, mut h, lines) = measure(&buffer);
            if lines == 0 {
                h = line_height;
            }
            let box_w = limit.max(w);
            if (box_w - layout_w).abs() > 0.5 {
                // Alinhar (centro/fim) dentro da caixa final; as quebras não mudam porque
                // nenhuma linha passa de `w <= box_w`.
                buffer.set_size(fs, Some(box_w.ceil()), None);
                buffer.shape_until_scroll(fs, false);
            }
            (box_w, h)
        } else {
            buffer.set_wrap(fs, Wrap::None);
            buffer.set_size(fs, None, None);
            buffer.set_text(fs, spec.text, attrs, Shaping::Advanced);
            buffer.shape_until_scroll(fs, false);

            let (w, mut h, lines) = measure(&buffer);
            if lines == 0 {
                h = line_height;
            }
            if lines > 1 && spec.align != TextAlignment::Start {
                // Alinhar dentro da largura da própria caixa (centro/fim entre linhas).
                buffer.set_size(fs, Some(w.ceil()), None);
                for line in buffer.lines.iter_mut() {
                    line.set_align(Some(align));
                }
                buffer.shape_until_scroll(fs, false);
            }
            (w, h)
        }
    }
}

/// Folga relativa à caixa (`maxWidth`) e em `em` do corpo ao decidir se uma palavra cabe na linha.
/// O navegador do Estúdio usa números tabulares (`tnum`) e arredondamentos próprios; o
/// cosmic-text 0.12 não expõe features OpenType, então as medidas do motor saem levemente maiores.
const WRAP_TOLERANCE_REL: f32 = 0.03;
const WRAP_TOLERANCE_EM: f32 = 0.08;

/// Largura extra aceita além de `limit` antes de mandar a palavra para a linha de baixo.
fn wrap_tolerance(limit: f32, size: f32) -> f32 {
    (limit * WRAP_TOLERANCE_REL).max(size * WRAP_TOLERANCE_EM)
}

/// (largura máxima, altura total, nº de linhas de layout).
fn measure(buffer: &Buffer) -> (f32, f32, usize) {
    let mut width = 0.0f32;
    let mut height = 0.0f32;
    let mut lines = 0usize;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        height = height.max(run.line_top + run.line_height);
        lines += 1;
    }
    (width, height, lines)
}

fn primary_font_name(font_id: &str) -> &str {
    font_id
        .split(',')
        .next()
        .unwrap_or(font_id)
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
}

fn normalize(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn stretch_rank(stretch: fontdb::Stretch) -> u8 {
    match stretch {
        fontdb::Stretch::UltraCondensed => 0,
        fontdb::Stretch::ExtraCondensed => 1,
        fontdb::Stretch::Condensed => 2,
        fontdb::Stretch::SemiCondensed => 3,
        fontdb::Stretch::Normal => 4,
        fontdb::Stretch::SemiExpanded => 5,
        fontdb::Stretch::Expanded => 6,
        fontdb::Stretch::ExtraExpanded => 7,
        fontdb::Stretch::UltraExpanded => 8,
    }
}

/// Verifica se uma face pertence à família `target_norm`: nome exato, nome com sufixo de
/// peso/estilo (ex.: `"Outfit Thin"`, `"Segoe UI Semibold"`, `"Arial Black"`), ou stem do arquivo.
fn face_matches_family(face: &fontdb::FaceInfo, target_norm: &str) -> bool {
    for (name, _) in &face.families {
        let norm = normalize(name);
        if norm == target_norm || stem_matches_family(&norm, target_norm) {
            return true;
        }
    }
    if let fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) = &face.source {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(normalize)
            .unwrap_or_default();
        if stem_matches_family(&stem, target_norm) {
            return true;
        }
    }
    false
}

/// Ajusta `(family, weight, stretch, style)` para a face real mais próxima presente na família alvo.
///
/// O `cosmic-text 0.12` rejeita famílias quando `face.stretch != attrs.stretch` (ex.: `Impact` tem
/// `Stretch::Condensed`) ou quando nenhuma face da família tem exatamente `font_weight_diff == 0`
/// (ex.: `weight = 500` em `Segoe UI`), saltando para fontes aleatórias de fallback no banco.
/// Também resolve subfamílias estáticas cujo `Name ID 1` inclui o peso (ex.: `Outfit Thin` = 300,
/// `Outfit Bold` = 700, `Segoe UI Semibold` = 600).
fn match_face_attrs(
    db: &fontdb::Database,
    family: Option<&str>,
    requested_weight: u16,
) -> (Option<String>, Weight, fontdb::Stretch, fontdb::Style) {
    let clamped = requested_weight.clamp(1, 1000);
    let sans = Family::SansSerif;
    let target = match family {
        Some(name) => name,
        None => db.family_name(&sans),
    };
    let target_norm = normalize(target);
    let mut best: Option<((u8, u8, u16, u8, u16), Option<String>, Weight, fontdb::Stretch, fontdb::Style)> = None;
    for face in db.faces() {
        let exact = face.families.iter().any(|(n, _)| n == target || normalize(n) == target_norm);
        if !exact && !face_matches_family(face, &target_norm) {
            continue;
        }
        let style_pen: u8 = match face.style {
            fontdb::Style::Normal => 0,
            fontdb::Style::Oblique => 1,
            fontdb::Style::Italic => 2,
        };
        let stretch_pen = (stretch_rank(face.stretch) as i32 - 4).unsigned_abs() as u8;
        let weight_diff = face.weight.0.abs_diff(clamped);
        let name_pen: u8 = u8::from(!exact);
        let weight_tie = if clamped >= 500 {
            u16::MAX - face.weight.0
        } else {
            face.weight.0
        };
        let score = (style_pen, stretch_pen, weight_diff, name_pen, weight_tie);
        if best.as_ref().map_or(true, |(b, _, _, _, _)| score < *b) {
            let face_family = face
                .families
                .iter()
                .find(|(n, _)| n == target)
                .or_else(|| face.families.first())
                .map(|(n, _)| n.clone());
            best = Some((score, face_family, face.weight, face.stretch, face.style));
        }
    }
    match best {
        Some((_, fam, w, st, sy)) => (fam, w, st, sy),
        None => (
            family.map(str::to_owned),
            Weight(clamped),
            fontdb::Stretch::Normal,
            fontdb::Style::Normal,
        ),
    }
}

fn stem_matches_family(stem: &str, needle: &str) -> bool {
    if stem == needle {
        return true;
    }
    let Some(rest) = stem.strip_prefix(needle) else {
        return false;
    };
    let trimmed = rest
        .strip_suffix("italic")
        .or_else(|| rest.strip_suffix("oblique"))
        .or_else(|| rest.strip_suffix("roman"))
        .unwrap_or(rest);
    if !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(weight) = trimmed.parse::<u16>() {
            if (100..=1000).contains(&weight) {
                return true;
            }
        }
    }
    matches!(
        rest,
        "regular"
            | "bold"
            | "semibold"
            | "extrabold"
            | "ultrabold"
            | "demibold"
            | "medium"
            | "light"
            | "extralight"
            | "ultralight"
            | "semilight"
            | "thin"
            | "black"
            | "heavy"
            | "book"
            | "normal"
            | "roman"
            | "variable"
            | "vf"
            | "wght"
            | "italic"
            | "bolditalic"
            | "oblique"
    )
}

/// Família real no banco que corresponde a `font_id`: nome exato (ignorando caixa/espaços/`-`/`_`)
/// e, em seguida, subfamília com sufixo de peso (ex.: `Outfit Thin`/`Outfit Bold`) ou stem do arquivo.
fn find_family(db: &fontdb::Database, font_id: &str) -> Option<String> {
    let primary = primary_font_name(font_id);
    let needle = normalize(primary);
    if needle.is_empty() {
        return None;
    }
    for face in db.faces() {
        for (name, _) in &face.families {
            if normalize(name) == needle {
                return Some(name.clone());
            }
        }
    }
    if db.faces().any(|face| face_matches_family(face, &needle)) {
        return Some(primary.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn impact_weight_200_shapes_with_real_impact_face() {
        let paths = crate::platform::windows::resolve_font_files("Impact");
        if paths.is_empty() {
            return;
        }
        let mut fs = FontSystem::new_with_locale_and_db(
            TEXT_ATLAS_LOCALE.to_string(),
            fontdb::Database::new(),
        );
        for p in crate::platform::windows::resolve_font_files("Segoe UI") {
            let _ = fs.db_mut().load_font_file(p);
        }
        for p in &paths {
            let _ = fs.db_mut().load_font_file(p);
        }
        let fam = find_family(fs.db(), "Impact").expect("Impact family loaded");
        let (matched_fam, w, st, sy) = match_face_attrs(fs.db(), Some(&fam), 200);
        let attrs = Attrs::new()
            .family(Family::Name(matched_fam.as_deref().unwrap_or(&fam)))
            .weight(w)
            .stretch(st)
            .style(sy);
        let mut buf = Buffer::new(&mut fs, Metrics::new(340.0, 340.0 * 1.06));
        buf.set_wrap(&mut fs, Wrap::None);
        buf.set_text(&mut fs, "16:30", attrs, Shaping::Advanced);
        buf.shape_until_scroll(&mut fs, false);
        let run = buf.layout_runs().next().expect("shaped run");
        assert!(!run.glyphs.is_empty());
        let used_face = fs.db().face(run.glyphs[0].font_id).expect("used face");
        assert!(
            used_face.families.iter().any(|(n, _)| n.eq_ignore_ascii_case("Impact")),
            "expected Impact, got {:?}",
            used_face.families
        );
    }

    #[test]
    fn wrap_tolerance_scales_with_box_and_font() {
        // Caixa grande: domina a parte relativa.
        assert!((wrap_tolerance(1000.0, 10.0) - 30.0).abs() < 1e-3);
        // Caixa pequena com fonte grande: domina a parte em em.
        assert!((wrap_tolerance(100.0, 200.0) - 16.0).abs() < 1e-3);
        assert!(wrap_tolerance(0.0, 0.0) >= 0.0);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn word_wrap_keeps_clock_whole_in_a_narrow_box() {
        let paths = crate::platform::windows::resolve_font_files("Segoe UI");
        if paths.is_empty() {
            return;
        }
        let mut fs = FontSystem::new_with_locale_and_db(
            TEXT_ATLAS_LOCALE.to_string(),
            fontdb::Database::new(),
        );
        for p in &paths {
            let _ = fs.db_mut().load_font_file(p);
        }
        let fam = find_family(fs.db(), "Segoe UI").expect("Segoe UI family loaded");
        let attrs = Attrs::new().family(Family::Name(&fam));
        let runs_at = |fs: &mut FontSystem, wrap: Wrap, width: Option<f32>| -> (usize, f32) {
            let mut buf = Buffer::new(fs, Metrics::new(120.0, 120.0 * 1.06));
            buf.set_wrap(fs, wrap);
            buf.set_size(fs, width, None);
            buf.set_text(fs, "00:06", attrs, Shaping::Advanced);
            buf.shape_until_scroll(fs, false);
            let (w, _, lines) = measure(&buf);
            (lines, w)
        };
        let (_, natural) = runs_at(&mut fs, Wrap::None, None);
        assert!(natural > 40.0, "relógio sem largura: {natural}");
        // Caixa bem menor que o texto: `Word` mantém "00:06" inteiro e a caixa passa a ser o texto.
        let (lines_word, w_word) = runs_at(&mut fs, Wrap::Word, Some(natural * 0.5));
        assert_eq!(lines_word, 1, "Word não pode quebrar dentro de 00:06");
        assert!(w_word >= natural * 0.99, "largura {w_word} < natural {natural}");
        // Controle: o modo antigo partia a palavra no meio.
        let (lines_glyph, _) = runs_at(&mut fs, Wrap::WordOrGlyph, Some(natural * 0.5));
        assert!(lines_glyph > 1, "o controle WordOrGlyph deveria quebrar");
    }

    // ── Paridade de caixa Estúdio × motor (casos em `prism-core/src/text_layout_cases.json`) ──

    #[derive(Debug, Clone, Default, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CaseFields {
        name: Option<String>,
        text: Option<String>,
        font_family: Option<String>,
        weight: Option<u16>,
        letter_spacing_em: Option<f32>,
        align: Option<String>,
        size_px: Option<f32>,
        max_width_em: Option<f32>,
        expect_lines: Option<usize>,
        min_lines: Option<usize>,
        max_lines: Option<usize>,
        box_at_least_max: Option<bool>,
        box_at_least_natural: Option<bool>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ScaleInvariance {
        text: String,
        sizes_px: Vec<f32>,
        tolerance: f32,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CasesFile {
        defaults: CaseFields,
        cases: Vec<CaseFields>,
        scale_invariance: ScaleInvariance,
    }

    const LAYOUT_CASES_JSON: &str = include_str!("../../../prism-core/src/text_layout_cases.json");

    /// Sistema de fontes só com as famílias embutidas no Estúdio (resultado igual em qualquer PC).
    fn bundled_font_system() -> FontSystem {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/prismview-studio/src-tauri/fonts");
        let mut fs = FontSystem::new_with_locale_and_db(
            TEXT_ATLAS_LOCALE.to_string(),
            fontdb::Database::new(),
        );
        let mut loaded = 0;
        for entry in std::fs::read_dir(&dir).expect("pasta src-tauri/fonts") {
            let path = entry.expect("entrada").path();
            if path.extension().and_then(|e| e.to_str()) == Some("ttf") {
                fs.db_mut().load_font_file(&path).expect("fonte embutida legível");
                loaded += 1;
            }
        }
        assert!(loaded > 0, "nenhuma fonte embutida em {}", dir.display());
        fs
    }

    /// Mesmo caminho de `TextSystem::update` (família/peso/linha 1.06), sem GPU.
    /// Devolve `(largura da caixa, altura, nº de linhas)`.
    fn shape_case(
        fs: &mut FontSystem,
        family: &str,
        weight: u16,
        text: &str,
        size: f32,
        letter_spacing_em: f32,
        align: TextAlignment,
        wrap_width: Option<f32>,
    ) -> (f32, f32, usize) {
        let fam = find_family(fs.db(), family).unwrap_or_else(|| panic!("família {family}"));
        let (matched, mw, ms, my) = match_face_attrs(fs.db(), Some(&fam), weight);
        let name = matched.unwrap_or(fam);
        let attrs = Attrs::new()
            .family(Family::Name(&name))
            .weight(mw)
            .stretch(ms)
            .style(my)
            .color(Color::rgb(255, 255, 255));
        let line_height = size * 1.06;
        let spec = TextSpec {
            text,
            font_id: family,
            size_px: size,
            weight,
            line_spacing: 0.0,
            letter_spacing_em,
            align,
            max_width_px: wrap_width.unwrap_or(0.0),
            stretch: 1.0,
            stem_compensation: 1.0,
        };
        let mut buffer = Buffer::new(fs, Metrics::new(16.0, 20.0));
        let mut spaced = Vec::new();
        let (w, h) = shape_layout(
            fs,
            &mut buffer,
            &mut spaced,
            &ShapeParams {
                spec: &spec,
                size,
                line_height,
                letter_spacing_px: letter_spacing_em * size,
                attrs,
                wrap_width,
            },
        );
        (w, h, (h / line_height).round().max(1.0) as usize)
    }

    fn parse_align(s: &str) -> TextAlignment {
        match s {
            "left" | "start" => TextAlignment::Start,
            "right" | "end" => TextAlignment::End,
            _ => TextAlignment::Center,
        }
    }

    #[test]
    fn studio_and_engine_text_boxes_stay_in_parity() {
        let file: CasesFile = serde_json::from_str(LAYOUT_CASES_JSON).expect("JSON de casos válido");
        let mut fs = bundled_font_system();
        let d = &file.defaults;
        for c in &file.cases {
            let name = c.name.as_deref().unwrap_or("?");
            let text = c.text.as_deref().expect("text");
            let family = c.font_family.as_deref().or(d.font_family.as_deref()).expect("fontFamily");
            let weight = c.weight.or(d.weight).expect("weight");
            let ls = c.letter_spacing_em.or(d.letter_spacing_em).unwrap_or(0.0);
            let size = c.size_px.or(d.size_px).expect("sizePx");
            let align = parse_align(c.align.as_deref().or(d.align.as_deref()).unwrap_or("center"));
            let max_px = c.max_width_em.map(|em| em * size);

            let (natural_w, _, _) = shape_case(&mut fs, family, weight, text, size, ls, align, None);
            let (w, _, lines) = shape_case(&mut fs, family, weight, text, size, ls, align, max_px);

            if let Some(n) = c.expect_lines {
                assert_eq!(lines, n, "[{name}] linhas");
            }
            if let Some(n) = c.min_lines {
                assert!(lines >= n, "[{name}] {lines} linhas < mínimo {n}");
            }
            if let Some(n) = c.max_lines {
                assert!(lines <= n, "[{name}] {lines} linhas > máximo {n}");
            }
            if c.box_at_least_max == Some(true) {
                let m = max_px.expect("boxAtLeastMax exige maxWidthEm");
                assert!(w >= m - 0.5, "[{name}] caixa {w} menor que maxWidth {m}");
            }
            if c.box_at_least_natural == Some(true) {
                assert!(
                    w >= natural_w * 0.99,
                    "[{name}] caixa {w} não comporta o texto natural {natural_w}"
                );
            }
        }
    }

    #[test]
    fn engine_text_width_scales_linearly_with_font_size() {
        let file: CasesFile = serde_json::from_str(LAYOUT_CASES_JSON).expect("JSON de casos válido");
        let s = &file.scale_invariance;
        let d = &file.defaults;
        let mut fs = bundled_font_system();
        let family = d.font_family.as_deref().expect("fontFamily");
        let weight = d.weight.expect("weight");
        let ratios: Vec<f32> = s
            .sizes_px
            .iter()
            .map(|&size| {
                let (w, _, _) =
                    shape_case(&mut fs, family, weight, &s.text, size, 0.0, TextAlignment::Center, None);
                w / size
            })
            .collect();
        let (lo, hi) = ratios
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &r| (lo.min(r), hi.max(r)));
        assert!(
            (hi - lo) / hi <= s.tolerance,
            "largura/tamanho não é constante: {ratios:?} (tolerância {})",
            s.tolerance
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn bundled_numeric_weight_fonts_shape_with_real_faces() {
        for family in ["Inter", "Outfit", "JetBrains Mono"] {
            let paths = crate::platform::windows::resolve_font_files(family);
            if paths.is_empty() {
                continue;
            }
            let mut fs = FontSystem::new_with_locale_and_db(
                TEXT_ATLAS_LOCALE.to_string(),
                fontdb::Database::new(),
            );
            for p in crate::platform::windows::resolve_font_files("Segoe UI") {
                let _ = fs.db_mut().load_font_file(p);
            }
            for p in &paths {
                let _ = fs.db_mut().load_font_file(p);
            }
            let fam = find_family(fs.db(), family).unwrap_or_else(|| panic!("{family} family loaded"));
            let needle = normalize(family);
            for requested in [300u16, 400, 500, 600, 700] {
                let (matched_fam, w, st, sy) = match_face_attrs(fs.db(), Some(&fam), requested);
                let attrs = Attrs::new()
                    .family(Family::Name(matched_fam.as_deref().unwrap_or(&fam)))
                    .weight(w)
                    .stretch(st)
                    .style(sy);
                let mut buf = Buffer::new(&mut fs, Metrics::new(64.0, 72.0));
                buf.set_wrap(&mut fs, Wrap::None);
                buf.set_text(&mut fs, "PrismView 12:34", attrs, Shaping::Advanced);
                buf.shape_until_scroll(&mut fs, false);
                let run = buf.layout_runs().next().expect("shaped run");
                assert!(!run.glyphs.is_empty());
                let used_face = fs.db().face(run.glyphs[0].font_id).expect("used face");
                assert!(
                    face_matches_family(used_face, &needle),
                    "expected {family} (weight {requested}), got {:?}",
                    used_face.families
                );
            }
        }
    }
}
