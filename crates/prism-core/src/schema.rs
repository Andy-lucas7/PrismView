use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub const IPC_PIPE_NAME: &str = r"\\.\pipe\prismview_ipc";
pub const MANIFEST_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PackageKind {
    #[default]
    Wallpaper,
    Widget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StaticOrFx<T> {
    Fx { fallback: T, expr: String },
    Static(T),
}

impl<T: Clone> StaticOrFx<T> {
    pub fn static_val(value: T) -> Self {
        Self::Static(value)
    }

    pub fn fx(fallback: T, expr: impl Into<String>) -> Self {
        let e = expr.into();
        if e.trim().is_empty() {
            Self::Static(fallback)
        } else {
            Self::Fx { fallback, expr: e }
        }
    }

    pub fn fallback(&self) -> &T {
        match self {
            Self::Static(v) => v,
            Self::Fx { fallback, .. } => fallback,
        }
    }

    pub fn expr(&self) -> Option<&str> {
        match self {
            Self::Static(_) => None,
            Self::Fx { expr, .. } if !expr.trim().is_empty() => Some(expr.as_str()),
            Self::Fx { .. } => None,
        }
    }

    pub fn is_fx(&self) -> bool {
        self.expr().is_some()
    }

    pub fn with_expr(&self, expr: impl Into<String>) -> Self {
        Self::fx(self.fallback().clone(), expr)
    }

    pub fn clear_fx(&self) -> Self {
        Self::Static(self.fallback().clone())
    }
}

impl<T: Default> Default for StaticOrFx<T> {
    fn default() -> Self {
        Self::Static(T::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GlobalType {
    #[default]
    Switch,
    Number,
    Text,
    Color,
    List,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalVar {
    pub name: String,
    #[serde(default)]
    pub kind: GlobalType,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub exposed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FillMode {
    #[default]
    Solid,
    Gradient,
    Adaptive,
    ClassicBlur,
    OpticalCrystal,
    Hollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GradientKind {
    #[default]
    Linear,
    Radial,
    Sweep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    SoftLight,
    HardLight,
    Difference,
    Exclusion,
    Plus,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    #[default]
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShapeKind {
    #[default]
    Rect,
    Circle,
    Line,
    Bar,
    Ring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StackDir {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TextAlign {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VisibilityCondition {
    #[default]
    Always,
    DesktopOnly,
    Charging,
    NotCharging,
    LowBattery,
    Day,
    Night,
    MediaPlaying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TouchActionType {
    #[default]
    None,
    OpenApp,
    OpenUrl,
    ToggleGlobal,
    SetGlobal,
    NextGlobalList,
    NextWallpaper,
    MediaPlayPause,
    MediaNext,
    MediaPrev,
    OpenPrismView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TouchAction {
    #[serde(default)]
    pub action_type: TouchActionType,
    #[serde(default)]
    pub target_exec: String,
    #[serde(default)]
    pub target_label: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub global_name: String,
    #[serde(default)]
    pub global_value: String,
}

impl TouchAction {
    pub fn is_active(&self) -> bool {
        self.action_type != TouchActionType::None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpticalCrystalParams {
    #[serde(default = "default_blur_px")]
    pub blur_px: StaticOrFx<f32>,
    #[serde(default = "default_refraction")]
    pub refraction: StaticOrFx<f32>,
    #[serde(default = "default_bevel_width")]
    pub bevel_width: StaticOrFx<f32>,
    #[serde(default = "default_dispersion_rgb")]
    pub dispersion_rgb: StaticOrFx<f32>,
    #[serde(default = "default_ior_red")]
    pub ior_red: f32,
    #[serde(default = "default_ior_green")]
    pub ior_green: f32,
    #[serde(default = "default_ior_blue")]
    pub ior_blue: f32,
    #[serde(default = "default_frost_amount")]
    pub frost_amount: StaticOrFx<f32>,
    #[serde(default = "default_light_angle")]
    pub light_angle_deg: StaticOrFx<f32>,
    #[serde(default = "default_fresnel")]
    pub fresnel_intensity: StaticOrFx<f32>,
    #[serde(default = "default_prismatic_rim")]
    pub prismatic_rim_intensity: StaticOrFx<f32>,
    #[serde(default = "default_saturation")]
    pub saturation: StaticOrFx<f32>,
    #[serde(default = "default_tint")]
    pub tint_rgba: StaticOrFx<String>,
    #[serde(default = "default_grain")]
    pub grain: f32,
}

fn default_blur_px() -> StaticOrFx<f32> {
    StaticOrFx::Static(26.0)
}
fn default_refraction() -> StaticOrFx<f32> {
    StaticOrFx::Static(36.0)
}
fn default_bevel_width() -> StaticOrFx<f32> {
    StaticOrFx::Static(22.0)
}
fn default_dispersion_rgb() -> StaticOrFx<f32> {
    StaticOrFx::Static(0.006)
}
fn default_ior_red() -> f32 {
    1.48
}
fn default_ior_green() -> f32 {
    1.50
}
fn default_ior_blue() -> f32 {
    1.525
}
fn default_frost_amount() -> StaticOrFx<f32> {
    StaticOrFx::Static(0.18)
}
fn default_light_angle() -> StaticOrFx<f32> {
    StaticOrFx::Static(225.0)
}
fn default_fresnel() -> StaticOrFx<f32> {
    StaticOrFx::Static(0.85)
}
fn default_prismatic_rim() -> StaticOrFx<f32> {
    StaticOrFx::Static(0.42)
}
fn default_saturation() -> StaticOrFx<f32> {
    StaticOrFx::Static(1.35)
}
fn default_tint() -> StaticOrFx<String> {
    StaticOrFx::Static("#26FFFFFF".to_string())
}
fn default_grain() -> f32 {
    0.03
}

impl Default for OpticalCrystalParams {
    fn default() -> Self {
        Self {
            blur_px: default_blur_px(),
            refraction: default_refraction(),
            bevel_width: default_bevel_width(),
            dispersion_rgb: default_dispersion_rgb(),
            ior_red: default_ior_red(),
            ior_green: default_ior_green(),
            ior_blue: default_ior_blue(),
            frost_amount: default_frost_amount(),
            light_angle_deg: default_light_angle(),
            fresnel_intensity: default_fresnel(),
            prismatic_rim_intensity: default_prismatic_rim(),
            saturation: default_saturation(),
            tint_rgba: default_tint(),
            grain: default_grain(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StemStretchParams {
    #[serde(default = "default_one_fx")]
    pub stretch: StaticOrFx<f32>,
    #[serde(default = "default_one")]
    pub stroke_compensation: f32,
    #[serde(default = "default_curve_alpha")]
    pub curve_alpha: f32,
    #[serde(default = "default_stretch_top")]
    pub stretch_top: f32,
    #[serde(default = "default_stretch_bottom")]
    pub stretch_bottom: f32,
}

fn default_one() -> f32 {
    1.0
}
fn default_one_fx() -> StaticOrFx<f32> {
    StaticOrFx::Static(1.0)
}
fn default_curve_alpha() -> f32 {
    0.34
}
fn default_stretch_top() -> f32 {
    0.42
}
fn default_stretch_bottom() -> f32 {
    0.82
}

impl Default for StemStretchParams {
    fn default() -> Self {
        Self {
            stretch: default_one_fx(),
            stroke_compensation: default_one(),
            curve_alpha: default_curve_alpha(),
            stretch_top: default_stretch_top(),
            stretch_bottom: default_stretch_bottom(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(default = "default_clock_text")]
    pub text: String,
    #[serde(default = "default_font_id")]
    pub font_id: String,
    #[serde(default = "default_font_size")]
    pub size_px: StaticOrFx<f32>,
    #[serde(default = "default_font_weight")]
    pub weight: u16,
    #[serde(default)]
    pub letter_spacing_em: f32,
    #[serde(default)]
    pub line_spacing: f32,
    #[serde(default)]
    pub align: TextAlign,
    #[serde(default)]
    pub axes: BTreeMap<String, f32>,
    #[serde(default)]
    pub stem_stretch: StemStretchParams,
}

fn default_clock_text() -> String {
    "$df(HH)$:$df(mm)$".to_string()
}
fn default_font_id() -> String {
    "Outfit".to_string()
}
fn default_font_size() -> StaticOrFx<f32> {
    StaticOrFx::Static(180.0)
}
fn default_font_weight() -> u16 {
    600
}

impl Default for TextContent {
    fn default() -> Self {
        Self {
            text: default_clock_text(),
            font_id: default_font_id(),
            size_px: default_font_size(),
            weight: default_font_weight(),
            letter_spacing_em: 0.0,
            line_spacing: 0.0,
            align: TextAlign::Center,
            axes: BTreeMap::new(),
            stem_stretch: StemStretchParams::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowParams {
    #[serde(default)]
    pub radius_px: f32,
    #[serde(default = "default_shadow_color")]
    pub color_rgba: String,
    #[serde(default)]
    pub dx_px: f32,
    #[serde(default = "default_shadow_dy")]
    pub dy_px: f32,
}

fn default_shadow_color() -> String {
    "#80000000".to_string()
}
fn default_shadow_dy() -> f32 {
    4.0
}

impl Default for ShadowParams {
    fn default() -> Self {
        Self {
            radius_px: 0.0,
            color_rgba: default_shadow_color(),
            dx_px: 0.0,
            dy_px: default_shadow_dy(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokeParams {
    #[serde(default)]
    pub width_px: f32,
    #[serde(default = "default_stroke_color")]
    pub color_rgba: String,
}

fn default_stroke_color() -> String {
    "#FF000000".to_string()
}

impl Default for StrokeParams {
    fn default() -> Self {
        Self {
            width_px: 0.0,
            color_rgba: default_stroke_color(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParallaxParams {
    #[serde(default)]
    pub depth_z: StaticOrFx<f32>,
    #[serde(default)]
    pub mouse_strength: f32,
    #[serde(default)]
    pub tilt_max_deg: f32,
}

impl Default for ParallaxParams {
    fn default() -> Self {
        Self {
            depth_z: StaticOrFx::Static(0.0),
            mouse_strength: 0.0,
            tilt_max_deg: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LayerKind {
    #[default]
    Text,
    Stack,
    Shape,
    Icon,
    Svg,
    Image,
    DepthSubject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerNode {
    #[serde(default = "new_id")]
    pub id: String,
    #[serde(default = "default_layer_name")]
    pub name: String,
    #[serde(default)]
    pub kind: LayerKind,
    #[serde(default = "default_half")]
    pub x: StaticOrFx<f32>,
    #[serde(default = "default_half")]
    pub y: StaticOrFx<f32>,
    #[serde(default)]
    pub anchor: Anchor,
    #[serde(default)]
    pub offset_x_px: StaticOrFx<f32>,
    #[serde(default)]
    pub offset_y_px: StaticOrFx<f32>,
    #[serde(default)]
    pub rotation_deg: StaticOrFx<f32>,
    #[serde(default = "default_one_fx")]
    pub scale: StaticOrFx<f32>,
    #[serde(default = "default_one_fx")]
    pub opacity: StaticOrFx<f32>,
    #[serde(default = "default_true_fx")]
    pub visible: StaticOrFx<bool>,
    #[serde(default)]
    pub visibility_cond: VisibilityCondition,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub fill: FillMode,
    #[serde(default = "default_white_fx")]
    pub color: StaticOrFx<String>,
    #[serde(default = "default_secondary_color")]
    pub color2: String,
    #[serde(default)]
    pub gradient_kind: GradientKind,
    #[serde(default = "default_grad_angle")]
    pub gradient_angle_deg: f32,
    #[serde(default)]
    pub crystal: OpticalCrystalParams,
    #[serde(default)]
    pub shadow: ShadowParams,
    #[serde(default)]
    pub stroke: StrokeParams,
    #[serde(default)]
    pub parallax: ParallaxParams,
    #[serde(default = "default_width")]
    pub width_px: StaticOrFx<f32>,
    #[serde(default = "default_height")]
    pub height_px: StaticOrFx<f32>,
    #[serde(default = "default_corner")]
    pub corner_radius_px: f32,
    #[serde(default)]
    pub shape_kind: ShapeKind,
    #[serde(default = "default_progress_expr")]
    pub progress_expr: String,
    #[serde(default = "default_track_color")]
    pub track_color: String,
    #[serde(default)]
    pub text: TextContent,
    #[serde(default = "default_asset_fx")]
    pub asset: StaticOrFx<String>,
    #[serde(default)]
    pub svg_paths: Vec<String>,
    #[serde(default = "default_svg_dim")]
    pub svg_viewbox_w: f32,
    #[serde(default = "default_svg_dim")]
    pub svg_viewbox_h: f32,
    #[serde(default)]
    pub stack_dir: StackDir,
    #[serde(default = "default_spacing")]
    pub stack_spacing_px: f32,
    #[serde(default)]
    pub touch: TouchAction,
    #[serde(default)]
    pub children: Vec<LayerNode>,
}

fn new_id() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}
fn default_layer_name() -> String {
    "Camada".to_string()
}
fn default_half() -> StaticOrFx<f32> {
    StaticOrFx::Static(0.5)
}
fn default_true_fx() -> StaticOrFx<bool> {
    StaticOrFx::Static(true)
}
fn default_white_fx() -> StaticOrFx<String> {
    StaticOrFx::Static("#FFFFFFFF".to_string())
}
fn default_secondary_color() -> String {
    "#FF00D2FF".to_string()
}
fn default_grad_angle() -> f32 {
    90.0
}
fn default_width() -> StaticOrFx<f32> {
    StaticOrFx::Static(320.0)
}
fn default_height() -> StaticOrFx<f32> {
    StaticOrFx::Static(140.0)
}
fn default_corner() -> f32 {
    24.0
}
fn default_progress_expr() -> String {
    "$sys(cpu)$".to_string()
}
fn default_track_color() -> String {
    "#33FFFFFF".to_string()
}
fn default_asset_fx() -> StaticOrFx<String> {
    StaticOrFx::Static(String::new())
}
fn default_svg_dim() -> f32 {
    24.0
}
fn default_spacing() -> f32 {
    12.0
}

impl Default for LayerNode {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: default_layer_name(),
            kind: LayerKind::Text,
            x: default_half(),
            y: default_half(),
            anchor: Anchor::Center,
            offset_x_px: StaticOrFx::Static(0.0),
            offset_y_px: StaticOrFx::Static(0.0),
            rotation_deg: StaticOrFx::Static(0.0),
            scale: default_one_fx(),
            opacity: default_one_fx(),
            visible: default_true_fx(),
            visibility_cond: VisibilityCondition::Always,
            blend: BlendMode::Normal,
            hidden: false,
            locked: false,
            fill: FillMode::Solid,
            color: default_white_fx(),
            color2: default_secondary_color(),
            gradient_kind: GradientKind::Linear,
            gradient_angle_deg: default_grad_angle(),
            crystal: OpticalCrystalParams::default(),
            shadow: ShadowParams::default(),
            stroke: StrokeParams::default(),
            parallax: ParallaxParams::default(),
            width_px: default_width(),
            height_px: default_height(),
            corner_radius_px: default_corner(),
            shape_kind: ShapeKind::Rect,
            progress_expr: default_progress_expr(),
            track_color: default_track_color(),
            text: TextContent::default(),
            asset: default_asset_fx(),
            svg_paths: Vec::new(),
            svg_viewbox_w: 24.0,
            svg_viewbox_h: 24.0,
            stack_dir: StackDir::Vertical,
            stack_spacing_px: 12.0,
            touch: TouchAction::default(),
            children: Vec::new(),
        }
    }
}

impl LayerNode {
    pub fn find(&self, target_id: &str) -> Option<&LayerNode> {
        if self.id == target_id {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find(target_id) {
                return Some(found);
            }
        }
        None
    }

    pub fn find_mut(&mut self, target_id: &str) -> Option<&mut LayerNode> {
        if self.id == target_id {
            return Some(self);
        }
        for child in &mut self.children {
            if let Some(found) = child.find_mut(target_id) {
                return Some(found);
            }
        }
        None
    }

    pub fn flatten<'a>(&'a self, depth: usize, out: &mut Vec<(&'a LayerNode, usize)>) {
        out.push((self, depth));
        for child in &self.children {
            child.flatten(depth + 1, out);
        }
    }

    pub fn clone_with_fresh_ids(&self) -> Self {
        let mut cloned = self.clone();
        cloned.id = new_id();
        cloned.children = self.children.iter().map(Self::clone_with_fresh_ids).collect();
        cloned
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BackgroundSource {
    #[default]
    None,
    Image {
        path: String,
    },
    Video {
        path: String,
        #[serde(default = "default_true")]
        muted: bool,
        #[serde(default = "default_true")]
        loop_video: bool,
    },
    CarouselFolder {
        path: String,
        #[serde(default)]
        interval_min: u32,
        #[serde(default = "default_true")]
        change_on_wake: bool,
    },
    SteamWorkshop {
        workshop_id: String,
        project_json_path: String,
        item_type: String,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DepthPostProcess {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bg_blur: f32,
    #[serde(default)]
    pub bg_desaturate: f32,
    #[serde(default)]
    pub invert_color_pop: bool,
    #[serde(default)]
    pub subject_pop: f32,
    #[serde(default)]
    pub vignette: f32,
    #[serde(default)]
    pub parallax_strength: f32,
    #[serde(default)]
    pub depth_overrides: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherConfig {
    #[serde(default)]
    pub city: String,
    #[serde(default = "default_lat")]
    pub lat: f64,
    #[serde(default = "default_lon")]
    pub lon: f64,
    #[serde(default = "default_true")]
    pub celsius: bool,
}

fn default_lat() -> f64 {
    -23.55
}
fn default_lon() -> f64 {
    -46.63
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            city: String::new(),
            lat: default_lat(),
            lon: default_lon(),
            celsius: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetPlacement {
    #[serde(default = "default_widget_aspect")]
    pub aspect_ratio: f32,
    #[serde(default)]
    pub monitor_index: u32,
    #[serde(default = "default_widget_pos")]
    pub pos_x: i32,
    #[serde(default = "default_widget_pos")]
    pub pos_y: i32,
    #[serde(default = "default_widget_w")]
    pub width_px: u32,
    #[serde(default = "default_widget_h")]
    pub height_px: u32,
    #[serde(default = "default_true")]
    pub pinned_to_desktop: bool,
    #[serde(default)]
    pub click_through: bool,
}

fn default_widget_aspect() -> f32 {
    2.0
}
fn default_widget_pos() -> i32 {
    96
}
fn default_widget_w() -> u32 {
    480
}
fn default_widget_h() -> u32 {
    240
}

impl Default for WidgetPlacement {
    fn default() -> Self {
        Self {
            aspect_ratio: default_widget_aspect(),
            monitor_index: 0,
            pos_x: default_widget_pos(),
            pos_y: default_widget_pos(),
            width_px: default_widget_w(),
            height_px: default_widget_h(),
            pinned_to_desktop: true,
            click_through: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrismScene {
    #[serde(default = "new_id")]
    pub id: String,
    #[serde(default = "default_scene_title")]
    pub title: String,
    #[serde(default)]
    pub package_type: PackageKind,
    #[serde(default = "default_dim")]
    pub dim: f32,
    #[serde(default)]
    pub background: BackgroundSource,
    #[serde(default)]
    pub depth: DepthPostProcess,
    #[serde(default)]
    pub weather: WeatherConfig,
    #[serde(default)]
    pub widget: WidgetPlacement,
    #[serde(default)]
    pub globals: Vec<GlobalVar>,
    #[serde(default)]
    pub custom_assets: BTreeMap<String, String>,
    #[serde(default)]
    pub layers: Vec<LayerNode>,
}

fn default_scene_title() -> String {
    "Optical Crystal".to_string()
}
fn default_dim() -> f32 {
    0.12
}

impl Default for PrismScene {
    fn default() -> Self {
        Self {
            id: new_id(),
            title: default_scene_title(),
            package_type: PackageKind::Wallpaper,
            dim: default_dim(),
            background: BackgroundSource::None,
            depth: DepthPostProcess::default(),
            weather: WeatherConfig::default(),
            widget: WidgetPlacement::default(),
            globals: Vec::new(),
            custom_assets: BTreeMap::new(),
            layers: Vec::new(),
        }
    }
}

impl PrismScene {
    pub fn is_widget(&self) -> bool {
        self.package_type == PackageKind::Widget
    }

    pub fn globals_map(&self) -> HashMap<String, String> {
        self.globals
            .iter()
            .map(|g| (g.name.to_lowercase(), g.value.clone()))
            .collect()
    }

    pub fn set_global(&mut self, name: &str, value: impl Into<String>) {
        let key = name.trim();
        if key.is_empty() {
            return;
        }
        let val = value.into();
        if let Some(existing) = self.globals.iter_mut().find(|g| g.name.eq_ignore_ascii_case(key)) {
            existing.value = val;
        } else {
            self.globals.push(GlobalVar {
                name: key.to_string(),
                kind: GlobalType::Text,
                value: val,
                options: Vec::new(),
                label: key.to_string(),
                exposed: false,
            });
        }
    }

    pub fn toggle_global(&mut self, name: &str) {
        let key = name.trim();
        if key.is_empty() {
            return;
        }
        if let Some(g) = self.globals.iter_mut().find(|g| g.name.eq_ignore_ascii_case(key)) {
            g.value = match g.kind {
                GlobalType::Switch => {
                    if g.value == "1" || g.value.eq_ignore_ascii_case("true") {
                        "0".to_string()
                    } else {
                        "1".to_string()
                    }
                }
                GlobalType::List | GlobalType::Color => {
                    if g.options.is_empty() {
                        g.value.clone()
                    } else {
                        let idx = g.options.iter().position(|o| o == &g.value).unwrap_or(0);
                        g.options[(idx + 1) % g.options.len()].clone()
                    }
                }
                GlobalType::Number => {
                    let next = g.value.parse::<f64>().unwrap_or(0.0) + 1.0;
                    if next.fract().abs() < 1e-9 {
                        format!("{}", next as i64)
                    } else {
                        next.to_string()
                    }
                }
                GlobalType::Text => {
                    if g.value.is_empty() {
                        "1".to_string()
                    } else {
                        String::new()
                    }
                }
            };
        } else {
            self.globals.push(GlobalVar {
                name: key.to_string(),
                kind: GlobalType::Switch,
                value: "1".to_string(),
                options: Vec::new(),
                label: key.to_string(),
                exposed: false,
            });
        }
    }

    pub fn cycle_global(&mut self, name: &str, step: i32) {
        let key = name.trim();
        if key.is_empty() {
            return;
        }
        if let Some(g) = self.globals.iter_mut().find(|g| g.name.eq_ignore_ascii_case(key)) {
            g.value = match g.kind {
                GlobalType::List if !g.options.is_empty() => {
                    let len = g.options.len() as i32;
                    let cur = g.options.iter().position(|o| o == &g.value).unwrap_or(0) as i32;
                    let idx = ((cur + step).rem_euclid(len)) as usize;
                    g.options[idx].clone()
                }
                GlobalType::Switch => {
                    if g.value == "1" {
                        "0".to_string()
                    } else {
                        "1".to_string()
                    }
                }
                GlobalType::Number => {
                    let next = g.value.parse::<f64>().unwrap_or(0.0) + f64::from(step);
                    if next.fract().abs() < 1e-9 {
                        format!("{}", next as i64)
                    } else {
                        next.to_string()
                    }
                }
                _ => g.value.clone(),
            };
        } else {
            self.toggle_global(key);
        }
    }

    pub fn find_layer(&self, id: &str) -> Option<&LayerNode> {
        self.layers.iter().find_map(|l| l.find(id))
    }

    pub fn find_layer_mut(&mut self, id: &str) -> Option<&mut LayerNode> {
        self.layers.iter_mut().find_map(|l| l.find_mut(id))
    }

    pub fn flatten_layers(&self) -> Vec<(&LayerNode, usize)> {
        let mut out = Vec::new();
        for layer in &self.layers {
            layer.flatten(0, &mut out);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackagedMediaItem {
    pub entry_path: String,
    #[serde(default)]
    pub original_key: String,
    #[serde(default = "default_true")]
    pub depth_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrismManifest {
    #[serde(default = "default_manifest_version")]
    pub version: u32,
    #[serde(default)]
    pub package_type: PackageKind,
    #[serde(default)]
    pub is_widget: bool,
    #[serde(default = "default_widget_aspect")]
    pub widget_aspect: f32,
    #[serde(default = "default_scene_title")]
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub preview_path: Option<String>,
    #[serde(default)]
    pub has_folder: bool,
    #[serde(default)]
    pub media: Vec<PackagedMediaItem>,
    #[serde(default)]
    pub font_map: BTreeMap<String, String>,
    #[serde(default)]
    pub custom_asset_map: BTreeMap<String, String>,
    #[serde(default)]
    pub layer_image_map: BTreeMap<String, String>,
    #[serde(default)]
    pub exposed_globals: Vec<GlobalVar>,
}

fn default_manifest_version() -> u32 {
    MANIFEST_VERSION
}

impl Default for PrismManifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            package_type: PackageKind::Wallpaper,
            is_widget: false,
            widget_aspect: default_widget_aspect(),
            title: default_scene_title(),
            author: String::new(),
            preview_path: None,
            has_folder: false,
            media: Vec::new(),
            font_map: BTreeMap::new(),
            custom_asset_map: BTreeMap::new(),
            layer_image_map: BTreeMap::new(),
            exposed_globals: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DirtyFlags(pub u32);

impl DirtyFlags {
    pub const NONE: Self = Self(0);
    pub const TIME_MINUTE: Self = Self(1 << 0);
    pub const TIME_SECOND: Self = Self(1 << 1);
    pub const SYSTEM_METRICS: Self = Self(1 << 2);
    pub const MEDIA: Self = Self(1 << 3);
    pub const WEATHER: Self = Self(1 << 4);
    pub const GLOBALS: Self = Self(1 << 5);
    pub const POINTER: Self = Self(1 << 6);
    pub const GEOMETRY: Self = Self(1 << 7);
    pub const BACKGROUND: Self = Self(1 << 8);
    pub const FULL_REBUILD: Self = Self(0xFFFF_FFFF);

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IpcRequest {
    Ping,
    GetStatus,
    LoadScene {
        scene: Box<PrismScene>,
        #[serde(default)]
        monitor_index: u32,
    },
    PatchLayer {
        scene_id: String,
        layer: Box<LayerNode>,
    },
    SetGlobal {
        scene_id: String,
        name: String,
        value: String,
    },
    ToggleGlobal {
        scene_id: String,
        name: String,
    },
    TriggerTouch {
        scene_id: String,
        layer_id: String,
    },
    ApplyPackage {
        package_path: String,
        #[serde(default)]
        monitor_index: u32,
    },
    PinWidget {
        scene: Box<PrismScene>,
    },
    UnpinWidget {
        widget_id: String,
    },
    SetPaused {
        paused: bool,
    },
    SetStudioPreviewMode {
        active: bool,
    },
    /// Modo nativo (`--native`): pinta o desktop (WorkerW) com `color` RGBA 0..1; `None` = preto.
    ClearDesktop {
        #[serde(default)]
        color: Option<[f32; 4]>,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EngineStatus {
    pub running: bool,
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub active_wallpaper_id: Option<String>,
    pub pinned_widget_ids: Vec<String>,
    pub active_monitor_count: u32,
    pub rendered_frames: u64,
    pub gpu_adapter: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IpcResponse {
    Ok,
    Pong { protocol_version: u32 },
    Status { engine: EngineStatus },
    Error { message: String },
}
