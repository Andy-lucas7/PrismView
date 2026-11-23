use crate::schema::{DirtyFlags, LayerNode, PrismScene, StaticOrFx, WeatherConfig};
use chrono::{DateTime, Datelike, Local, Timelike};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct SystemTelemetry {
    pub cpu_percent: f32,
    pub ram_percent: f32,
    pub battery_percent: u8,
    pub battery_charging: bool,
    pub cpu_temp_c: f32,
    pub volume_percent: u8,
    pub uptime_secs: u64,
    pub hostname: String,
    /// RAM usada/total em GB (`rm(ram_gb)` / `rm(ram_total_gb)`).
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
}

impl Default for SystemTelemetry {
    fn default() -> Self {
        Self {
            cpu_percent: 0.0,
            ram_percent: 0.0,
            battery_percent: 100,
            battery_charging: true,
            cpu_temp_c: 0.0,
            volume_percent: 50,
            uptime_secs: 0,
            hostname: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows".to_string()),
            ram_used_gb: 0.0,
            ram_total_gb: 0.0,
        }
    }
}

#[cfg(windows)]
mod win_telemetry {
    use super::Mutex;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn as_u64(self) -> u64 {
            ((self.high as u64) << 32) | (self.low as u64)
        }
    }

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
        fn GetTickCount64() -> u64;
    }

    static PREV_CPU: Mutex<Option<(u64, u64, f32)>> = Mutex::new(None);

    pub fn sample() -> super::SystemTelemetry {
        let mut t = super::SystemTelemetry::default();
        unsafe {
            let mut idle = FileTime::default();
            let mut kernel = FileTime::default();
            let mut user = FileTime::default();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0 {
                let idle_v = idle.as_u64();
                let total_v = kernel.as_u64().saturating_add(user.as_u64());
                if let Ok(mut guard) = PREV_CPU.lock() {
                    if let Some((prev_idle, prev_total, last_pct)) = *guard {
                        let d_idle = idle_v.saturating_sub(prev_idle);
                        let d_total = total_v.saturating_sub(prev_total);
                        if d_total > 0 {
                            let busy = d_total.saturating_sub(d_idle) as f64 / d_total as f64;
                            let pct = (busy * 100.0).clamp(0.0, 100.0) as f32;
                            t.cpu_percent = (pct * 10.0).round() / 10.0;
                            *guard = Some((idle_v, total_v, t.cpu_percent));
                        } else {
                            t.cpu_percent = last_pct;
                        }
                    } else {
                        *guard = Some((idle_v, total_v, 0.0));
                    }
                }
            }

            let mut mem = MemoryStatusEx {
                length: std::mem::size_of::<MemoryStatusEx>() as u32,
                memory_load: 0,
                total_phys: 0,
                avail_phys: 0,
                total_page_file: 0,
                avail_page_file: 0,
                total_virtual: 0,
                avail_virtual: 0,
                avail_extended_virtual: 0,
            };
            if GlobalMemoryStatusEx(&mut mem) != 0 {
                t.ram_percent = mem.memory_load.min(100) as f32;
                const GB: f64 = 1024.0 * 1024.0 * 1024.0;
                t.ram_total_gb = (mem.total_phys as f64 / GB) as f32;
                t.ram_used_gb = (mem.total_phys.saturating_sub(mem.avail_phys) as f64 / GB) as f32;
            }

            let mut power = SystemPowerStatus::default();
            if GetSystemPowerStatus(&mut power) != 0 {
                if power.battery_life_percent <= 100 {
                    t.battery_percent = power.battery_life_percent;
                }
                t.battery_charging = power.ac_line_status == 1;
            }

            t.uptime_secs = GetTickCount64() / 1000;
        }
        t
    }
}

impl SystemTelemetry {
    pub fn sample_live() -> Self {
        #[cfg(windows)]
        {
            win_telemetry::sample()
        }
        #[cfg(not(windows))]
        {
            Self::default()
        }
    }
}

impl WeatherTelemetry {
    pub fn from_config(cfg: &WeatherConfig) -> Self {
        Self {
            city: cfg.city.clone(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MediaTelemetry {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_path: String,
    pub playing: bool,
    /// Progresso (`mi(pos|dur|pct)`): posição/duração em s e instante (ms Unix) da amostra.
    pub position_secs: f64,
    pub duration_secs: f64,
    pub updated_at_ms: f64,
}

#[derive(Debug, Clone)]
pub struct WeatherTelemetry {
    pub temp_c: Option<f32>,
    pub condition: String,
    pub code: i32,
    pub city: String,
    pub is_day: bool,
}

impl Default for WeatherTelemetry {
    fn default() -> Self {
        Self {
            temp_c: None,
            condition: "Céu limpo".to_string(),
            code: 0,
            city: String::new(),
            is_day: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EvalContext {
    pub now: DateTime<Local>,
    pub system: SystemTelemetry,
    pub media: MediaTelemetry,
    pub weather: WeatherTelemetry,
    pub globals: HashMap<String, String>,
    pub pointer_norm: [f32; 2],
}

impl Default for EvalContext {
    fn default() -> Self {
        Self {
            now: Local::now(),
            system: SystemTelemetry::default(),
            media: MediaTelemetry::default(),
            weather: WeatherTelemetry::default(),
            globals: HashMap::new(),
            pointer_norm: [0.5, 0.5],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalOutcome<T> {
    pub value: T,
    pub deps: DirtyFlags,
}

// ═════════════════════════════════════════════════════════════════════════════════════════════
// Avaliador FX — tokenizer/parser de passada única.
//
// ESPELHO 1:1 de `apps/prismview-studio/src/engine/fx/fxEngine.ts`. Qualquer mudança aqui DEVE
// ser replicada lá. Casos compartilhados: `fx_parity_cases.json` (rodados pelos dois lados).
//
// Regras: ver o cabeçalho do arquivo TS. Resumo: `$expr$` (com `$$` = "$"); itens justapostos
// concatenam preservando o espaço original; operadores só valem com operando à direita;
// aritmética com texto vira concatenação; `df(...)` recebe o argumento cru (vírgulas incluídas)
// e tokeniza o padrão por corridas de letras (EEEE/MMMM têm prioridade, nada é re-processado).
// ═════════════════════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct KVal {
    text: String,
    num: Option<f64>,
}

const MAX_DEPTH: u32 = 64;
const MAX_FL_DEPTH: u32 = 2;
const MAX_FL_ITERS: usize = 64;

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

fn trim_ws(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C'))
}

/// `^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$` (mesma regex do TS) + finito.
fn parse_num(s: &str) -> Option<f64> {
    let t = trim_ws(s);
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let fs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - fs;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let es = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == es {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[inline]
fn clamp_abs(n: f64) -> f64 {
    if !n.is_finite() {
        return 0.0;
    }
    n.max(-1e12).min(1e12)
}

#[inline]
fn round_half_up(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Inteiro quando possível; senão até 2 casas, sem zeros à direita.
fn format_num(n: f64) -> String {
    if !n.is_finite() {
        return "0".to_string();
    }
    let cents = (clamp_abs(n) * 100.0 + 0.5).floor() as i64;
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let int = abs / 100;
    let frac = abs % 100;
    let mut s = String::with_capacity(16);
    if neg {
        s.push('-');
    }
    s.push_str(&int.to_string());
    if frac != 0 {
        s.push('.');
        if frac % 10 == 0 {
            s.push_str(&(frac / 10).to_string());
        } else {
            s.push_str(&format!("{frac:02}"));
        }
    }
    s
}

/// Sempre `d` casas (meio-para-cima, idêntico ao TS).
fn fixed(n: f64, d: u32) -> String {
    let p = 10i64.pow(d);
    let v = (clamp_abs(n) * p as f64 + 0.5).floor() as i64;
    let neg = v < 0;
    let abs = v.unsigned_abs();
    let pu = p as u64;
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&(abs / pu).to_string());
    if d > 0 {
        s.push('.');
        s.push_str(&format!("{:0width$}", abs % pu, width = d as usize));
    }
    s
}

impl KVal {
    fn empty() -> Self {
        Self {
            text: String::new(),
            num: None,
        }
    }

    fn of_text(s: impl Into<String>) -> Self {
        let text = s.into();
        let num = parse_num(&text);
        Self { text, num }
    }

    fn of_num(n: f64) -> Self {
        if !n.is_finite() {
            return Self {
                text: "0".to_string(),
                num: Some(0.0),
            };
        }
        Self {
            text: format_num(n),
            num: Some(n),
        }
    }

    fn of_bool(b: bool) -> Self {
        Self {
            text: if b { "1" } else { "0" }.to_string(),
            num: Some(if b { 1.0 } else { 0.0 }),
        }
    }

    fn is_truthy(&self) -> bool {
        match self.num {
            Some(n) => n != 0.0,
            None => {
                let t = trim_ws(&self.text);
                !t.is_empty() && t.to_lowercase() != "false"
            }
        }
    }
}

pub struct FxEngine;

impl FxEngine {
    pub fn eval_text(input: &str, ctx: &EvalContext) -> EvalOutcome<String> {
        if !input.contains('$') {
            return EvalOutcome {
                value: input.to_string(),
                deps: DirtyFlags::NONE,
            };
        }
        let mut out = String::with_capacity(input.len() + 16);
        let mut deps = DirtyFlags::NONE;
        let mut i = 0;
        while i < input.len() {
            let Some(rel) = input[i..].find('$') else { break };
            let start = i + rel;
            out.push_str(&input[i..start]);
            match find_closing_dollar(input, start + 1) {
                Some(close) => {
                    if close == start + 1 {
                        out.push('$');
                    } else {
                        let (v, d) = eval_expression_raw(&input[start + 1..close], ctx);
                        out.push_str(&v.text);
                        deps.insert(d);
                    }
                    i = close + 1;
                }
                None => {
                    out.push_str(&input[start..]);
                    i = input.len();
                    break;
                }
            }
        }
        if i < input.len() {
            out.push_str(&input[i..]);
        }
        EvalOutcome { value: out, deps }
    }

    pub fn eval_expr(expr: &str, ctx: &EvalContext) -> EvalOutcome<String> {
        let trimmed = trim_ws(expr);
        if trimmed.is_empty() {
            return EvalOutcome {
                value: String::new(),
                deps: DirtyFlags::NONE,
            };
        }
        if trimmed.contains('$') {
            return Self::eval_text(trimmed, ctx);
        }
        let (v, deps) = eval_expression_raw(trimmed, ctx);
        EvalOutcome {
            value: v.text,
            deps,
        }
    }

    pub fn eval_f32(prop: &StaticOrFx<f32>, ctx: &EvalContext) -> EvalOutcome<f32> {
        match prop {
            StaticOrFx::Static(v) => EvalOutcome {
                value: *v,
                deps: DirtyFlags::NONE,
            },
            StaticOrFx::Fx { fallback, expr } => {
                let res = Self::eval_expr(expr, ctx);
                let parsed = res
                    .value
                    .trim()
                    .trim_end_matches('%')
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(*fallback);
                EvalOutcome {
                    value: parsed,
                    deps: res.deps,
                }
            }
        }
    }

    pub fn eval_bool(prop: &StaticOrFx<bool>, ctx: &EvalContext) -> EvalOutcome<bool> {
        match prop {
            StaticOrFx::Static(v) => EvalOutcome {
                value: *v,
                deps: DirtyFlags::NONE,
            },
            StaticOrFx::Fx { fallback, expr } => {
                if expr.trim().is_empty() {
                    return EvalOutcome {
                        value: *fallback,
                        deps: DirtyFlags::NONE,
                    };
                }
                let res = Self::eval_expr(expr, ctx);
                let v = KVal::of_text(res.value).is_truthy();
                EvalOutcome {
                    value: v,
                    deps: res.deps,
                }
            }
        }
    }

    pub fn eval_string(prop: &StaticOrFx<String>, ctx: &EvalContext) -> EvalOutcome<String> {
        match prop {
            StaticOrFx::Static(v) => {
                if v.contains('$') {
                    Self::eval_text(v, ctx)
                } else {
                    EvalOutcome {
                        value: v.clone(),
                        deps: DirtyFlags::NONE,
                    }
                }
            }
            StaticOrFx::Fx { fallback, expr } => {
                let res = Self::eval_expr(expr, ctx);
                let value = if res.value.is_empty() {
                    fallback.clone()
                } else {
                    res.value
                };
                EvalOutcome {
                    value,
                    deps: res.deps,
                }
            }
        }
    }

    pub fn collect_layer_dependencies(layer: &LayerNode, ctx: &EvalContext) -> DirtyFlags {
        let mut deps = DirtyFlags::NONE;
        if layer.hidden {
            return deps;
        }
        deps.insert(Self::eval_f32(&layer.x, ctx).deps);
        deps.insert(Self::eval_f32(&layer.y, ctx).deps);
        deps.insert(Self::eval_f32(&layer.offset_x_px, ctx).deps);
        deps.insert(Self::eval_f32(&layer.offset_y_px, ctx).deps);
        deps.insert(Self::eval_f32(&layer.rotation_deg, ctx).deps);
        deps.insert(Self::eval_f32(&layer.scale, ctx).deps);
        deps.insert(Self::eval_f32(&layer.opacity, ctx).deps);
        deps.insert(Self::eval_bool(&layer.visible, ctx).deps);
        deps.insert(Self::eval_string(&layer.color, ctx).deps);
        deps.insert(Self::eval_f32(&layer.width_px, ctx).deps);
        deps.insert(Self::eval_f32(&layer.height_px, ctx).deps);
        deps.insert(Self::eval_string(&layer.asset, ctx).deps);
        deps.insert(Self::eval_text(&layer.text.text, ctx).deps);
        deps.insert(Self::eval_f32(&layer.text.size_px, ctx).deps);
        deps.insert(Self::eval_f32(&layer.text.stem_stretch.stretch, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.blur_px, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.frost_amount, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.dispersion_rgb, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.refraction, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.bevel_width, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.light_angle_deg, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.fresnel_intensity, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.prismatic_rim_intensity, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.brightness, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.contrast, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.saturation, ctx).deps);
        deps.insert(Self::eval_string(&layer.crystal.tint_rgba, ctx).deps);
        deps.insert(Self::eval_text(&layer.progress_expr, ctx).deps);
        if layer.parallax.mouse_strength.abs() > 1e-4 || layer.parallax.tilt_max_deg.abs() > 1e-4 {
            deps.insert(DirtyFlags::POINTER);
        }
        for child in &layer.children {
            deps.insert(Self::collect_layer_dependencies(child, ctx));
        }
        deps
    }

    pub fn collect_scene_dependencies(scene: &PrismScene, ctx: &EvalContext) -> DirtyFlags {
        let mut deps = DirtyFlags::NONE;
        if scene.depth.parallax_strength.abs() > 1e-4 {
            deps.insert(DirtyFlags::POINTER);
        }
        for layer in &scene.layers {
            deps.insert(Self::collect_layer_dependencies(layer, ctx));
        }
        deps
    }

    pub fn parse_rgba_hex(raw: &str) -> Option<[f32; 4]> {
        let argb = parse_argb_u32(raw)?;
        let a = ((argb >> 24) & 0xFF) as f32 / 255.0;
        let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
        let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
        let b = (argb & 0xFF) as f32 / 255.0;
        Some([r, g, b, a])
    }
}

fn eval_expression_raw(expr: &str, ctx: &EvalContext) -> (KVal, DirtyFlags) {
    let mut p = Parser::new(expr, ctx, Vec::new(), 0, 0);
    let v = p.parse_seq(false, false);
    (v, p.deps)
}

/// Próximo `$` fora de aspas.
fn find_closing_dollar(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
        } else {
            match b {
                b'"' | b'\'' => quote = Some(b),
                b'$' => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn parse_argb_u32(raw: &str) -> Option<u32> {
    let s = trim_ws(raw).trim_start_matches('#');
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok().map(|rgb| 0xFF00_0000 | rgb),
        8 => u32::from_str_radix(s, 16).ok(),
        _ => None,
    }
}

fn format_argb_hex(c: u32) -> String {
    format!("#{:08X}", c)
}

// ───────────────────────── padrões de data/duração ─────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum PatTok {
    Run(char, usize),
    Lit(String),
}

/// Tokens de `df()`: yyyy yy MMMM MMM MM M EEEE EEE dd d HH H hh h mm m ss s a.
fn is_date_token(ch: char, n: usize) -> bool {
    match ch {
        'y' => n == 2 || n == 4,
        'M' => (1..=4).contains(&n),
        'E' => n == 3 || n == 4,
        'd' | 'H' | 'h' | 'm' | 's' => n == 1 || n == 2,
        'a' => n == 1,
        _ => false,
    }
}

/// Tokens de `tf()`: D DD H HH m mm s ss.
fn is_duration_token(ch: char, n: usize) -> bool {
    matches!(ch, 'D' | 'H' | 'm' | 's') && (n == 1 || n == 2)
}

/// Passada única. Uma PALAVRA (corrida máxima de letras Unicode, `char::is_alphabetic`) só vira
/// token se for inteiramente um token válido; senão sai literal ("de", "em", "às", "da"...).
/// Separadores e `'texto'` (`''` = aspa) são literais. Tokens colados (`HHmm`) precisam de
/// separador. Espelho de `tokenizePattern` no TS.
fn tokenize_pattern(p: &str, is_token: fn(char, usize) -> bool) -> Vec<PatTok> {
    let chars: Vec<char> = p.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                lit.push('\'');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j] == '\'' {
                    if chars.get(j + 1) == Some(&'\'') {
                        lit.push('\'');
                        j += 2;
                        continue;
                    }
                    break;
                }
                lit.push(chars[j]);
                j += 1;
            }
            i = j + 1;
            continue;
        }
        if ch.is_alphabetic() {
            let mut n = 1;
            while i + n < chars.len() && chars[i + n].is_alphabetic() {
                n += 1;
            }
            let word = &chars[i..i + n];
            let first = word[0];
            if first.is_ascii_alphabetic() && word.iter().all(|&c| c == first) && is_token(first, n) {
                if !lit.is_empty() {
                    out.push(PatTok::Lit(std::mem::take(&mut lit)));
                }
                out.push(PatTok::Run(first, n));
            } else {
                lit.extend(word.iter());
            }
            i += n;
            continue;
        }
        lit.push(ch);
        i += 1;
    }
    if !lit.is_empty() {
        out.push(PatTok::Lit(lit));
    }
    out
}

const DAYS_SHORT: [&str; 7] = ["Dom", "Seg", "Ter", "Qua", "Qui", "Sex", "Sáb"];
const DAYS_FULL: [&str; 7] = [
    "Domingo",
    "Segunda-feira",
    "Terça-feira",
    "Quarta-feira",
    "Quinta-feira",
    "Sexta-feira",
    "Sábado",
];
const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Fev", "Mar", "Abr", "Mai", "Jun", "Jul", "Ago", "Set", "Out", "Nov", "Dez",
];
const MONTHS_FULL: [&str; 12] = [
    "Janeiro", "Fevereiro", "Março", "Abril", "Maio", "Junho", "Julho", "Agosto", "Setembro",
    "Outubro", "Novembro", "Dezembro",
];

fn push_num(out: &mut String, v: u32, n: usize) {
    if n >= 2 {
        out.push_str(&format!("{v:02}"));
    } else {
        out.push_str(&v.to_string());
    }
}

fn push_repeat(out: &mut String, ch: char, n: usize) {
    for _ in 0..n {
        out.push(ch);
    }
}

fn format_date(toks: &[PatTok], now: &DateTime<Local>) -> String {
    let mut out = String::new();
    for t in toks {
        let (ch, n) = match t {
            PatTok::Lit(s) => {
                out.push_str(s);
                continue;
            }
            PatTok::Run(ch, n) => (*ch, *n),
        };
        match ch {
            'y' => {
                let y = now.year();
                if n == 2 {
                    out.push_str(&format!("{:02}", y.rem_euclid(100)));
                } else {
                    out.push_str(&y.to_string());
                }
            }
            'M' => {
                let m = now.month0() as usize;
                if n >= 4 {
                    out.push_str(MONTHS_FULL[m % 12]);
                } else if n == 3 {
                    out.push_str(MONTHS_SHORT[m % 12]);
                } else {
                    push_num(&mut out, m as u32 + 1, n);
                }
            }
            'd' => push_num(&mut out, now.day(), n),
            'E' => {
                let wd = now.weekday().num_days_from_sunday() as usize % 7;
                out.push_str(if n >= 4 { DAYS_FULL[wd] } else { DAYS_SHORT[wd] });
            }
            'H' => push_num(&mut out, now.hour(), n),
            'h' => {
                let h = now.hour() % 12;
                push_num(&mut out, if h == 0 { 12 } else { h }, n);
            }
            'm' => push_num(&mut out, now.minute(), n),
            's' => push_num(&mut out, now.second(), n),
            'a' => out.push_str(if now.hour() < 12 { "AM" } else { "PM" }),
            other => push_repeat(&mut out, other, n),
        }
    }
    out
}

/// Duração: D dias, H horas, m minutos, s segundos (cada unidade absorve as ausentes).
fn format_duration(secs: f64, pattern: &str) -> String {
    let t = clamp_abs(secs).max(0.0).floor() as u64;
    let p = if pattern.is_empty() {
        if t >= 3600 {
            "H:mm:ss"
        } else {
            "mm:ss"
        }
    } else {
        pattern
    };
    let toks = tokenize_pattern(p, is_duration_token);
    let has = |c: char| toks.iter().any(|k| matches!(k, PatTok::Run(ch, _) if *ch == c));
    let mut r = t;
    let (mut dd, mut hh, mut mm) = (0u64, 0u64, 0u64);
    if has('D') {
        dd = r / 86400;
        r %= 86400;
    }
    if has('H') {
        hh = r / 3600;
        r %= 3600;
    }
    if has('m') {
        mm = r / 60;
        r %= 60;
    }
    let ss = r;
    let mut out = String::new();
    for k in &toks {
        match k {
            PatTok::Lit(s) => out.push_str(s),
            PatTok::Run(ch, n) => {
                let v = match ch {
                    'D' => Some(dd),
                    'H' => Some(hh),
                    'm' => Some(mm),
                    's' => Some(ss),
                    _ => None,
                };
                match v {
                    Some(v) if *n >= 2 => out.push_str(&format!("{v:02}")),
                    Some(v) => out.push_str(&v.to_string()),
                    None => push_repeat(&mut out, *ch, *n),
                }
            }
        }
    }
    out
}

fn media_progress(m: &MediaTelemetry, now_ms: f64) -> (f64, f64, f64) {
    let dur = m.duration_secs.max(0.0);
    let base = m.position_secs.max(0.0);
    let updated = m.updated_at_ms;
    let elapsed = if m.playing && updated > 0.0 {
        ((now_ms - updated) / 1000.0).max(0.0)
    } else {
        0.0
    };
    let pos = if dur > 0.0 {
        dur.min(base + elapsed)
    } else {
        base + elapsed
    };
    let ratio = if dur > 0.5 {
        (pos / dur).min(1.0).max(0.0)
    } else {
        0.0
    };
    (pos, dur, ratio)
}

// ───────────────────────── parser ─────────────────────────

#[inline]
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

#[inline]
fn is_ident(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

const CMP_OPS: [&str; 8] = ["<=", ">=", "==", "!=", "<>", "=", "<", ">"];

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    ctx: &'a EvalContext,
    locals: Vec<(String, KVal)>,
    deps: DirtyFlags,
    depth: u32,
    fl_depth: u32,
}

impl<'a> Parser<'a> {
    fn new(
        src: &'a str,
        ctx: &'a EvalContext,
        locals: Vec<(String, KVal)>,
        depth: u32,
        fl_depth: u32,
    ) -> Self {
        Self {
            src,
            b: src.as_bytes(),
            pos: 0,
            ctx,
            locals,
            deps: DirtyFlags::NONE,
            depth,
            fl_depth,
        }
    }

    #[inline]
    fn at(&self, p: usize) -> Option<u8> {
        self.b.get(p).copied()
    }

    fn skip_ws(&mut self) {
        while self.pos < self.b.len() && is_ws(self.b[self.pos]) {
            self.pos += 1;
        }
    }

    fn ws_end(&self, from: usize) -> usize {
        let mut p = from;
        while p < self.b.len() && is_ws(self.b[p]) {
            p += 1;
        }
        p
    }

    fn operand_at(&self, p: usize) -> bool {
        !matches!(self.at(self.ws_end(p)), None | Some(b',') | Some(b')'))
    }

    fn immediate_operand(&self, p: usize) -> bool {
        match self.at(p) {
            Some(c) => {
                is_ident_start(c)
                    || c.is_ascii_digit()
                    || matches!(c, b'(' | b'"' | b'\'' | b'.' | b'-' | b'!')
            }
            None => false,
        }
    }

    /// Consome um caractere UTF-8 inteiro (chamar só com `pos < len`).
    fn take_char(&mut self) -> &'a str {
        let src = self.src;
        let rest = &src[self.pos..];
        let len = rest.chars().next().map_or(rest.len(), char::len_utf8);
        self.pos += len;
        &rest[..len]
    }

    fn try_op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        let save = self.pos;
        self.skip_ws();
        let src = self.src;
        let rest = &src[self.pos..];
        for &op in ops {
            if rest.starts_with(op) {
                let after = self.pos + op.len();
                if self.operand_at(after) {
                    self.pos = after;
                    return Some(op);
                }
                break;
            }
        }
        self.pos = save;
        None
    }

    fn find_local(&self, name: &str) -> Option<KVal> {
        self.locals
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    /// Sequência de itens justapostos (até o fim, ou `,`/`)` quando são delimitadores).
    fn parse_seq(&mut self, stop_comma: bool, stop_paren: bool) -> KVal {
        if self.depth >= MAX_DEPTH {
            self.pos = self.b.len();
            return KVal::empty();
        }
        self.depth += 1;
        let mut first: Option<KVal> = None;
        let mut acc = String::new();
        let mut count = 0usize;
        loop {
            let save = self.pos;
            self.skip_ws();
            let Some(c) = self.at(self.pos) else { break };
            if (c == b',' && stop_comma) || (c == b')' && stop_paren) {
                break;
            }
            let gap_end = self.pos;
            let v = if c == b',' || c == b')' {
                self.pos += 1;
                KVal::of_text(if c == b',' { "," } else { ")" })
            } else {
                let start = self.pos;
                let v = self.parse_or();
                if self.pos == start {
                    KVal::of_text(self.take_char())
                } else {
                    v
                }
            };
            if count == 0 {
                acc.push_str(&v.text);
                first = Some(v);
            } else {
                acc.push_str(&self.src[save..gap_end]);
                acc.push_str(&v.text);
            }
            count += 1;
        }
        self.depth -= 1;
        match count {
            0 => KVal::empty(),
            1 => first.unwrap_or_else(KVal::empty),
            _ => KVal::of_text(acc),
        }
    }

    fn parse_or(&mut self) -> KVal {
        let mut left = self.parse_and();
        while self.try_op(&["||"]).is_some() {
            let right = self.parse_and();
            left = KVal::of_bool(left.is_truthy() || right.is_truthy());
        }
        left
    }

    fn parse_and(&mut self) -> KVal {
        let mut left = self.parse_cmp();
        while self.try_op(&["&&"]).is_some() {
            let right = self.parse_cmp();
            left = KVal::of_bool(left.is_truthy() && right.is_truthy());
        }
        left
    }

    fn parse_cmp(&mut self) -> KVal {
        let left = self.parse_add();
        let Some(op) = self.try_op(&CMP_OPS) else {
            return left;
        };
        let right = self.parse_add();
        let ok = match (left.num, right.num) {
            (Some(a), Some(b)) => match op {
                "<=" => a <= b,
                ">=" => a >= b,
                "==" | "=" => (a - b).abs() < 1e-9,
                "!=" | "<>" => (a - b).abs() >= 1e-9,
                "<" => a < b,
                _ => a > b,
            },
            _ => match op {
                "=" | "==" => left.text.to_lowercase() == right.text.to_lowercase(),
                "!=" | "<>" => left.text.to_lowercase() != right.text.to_lowercase(),
                "<=" => left.text <= right.text,
                ">=" => left.text >= right.text,
                "<" => left.text < right.text,
                _ => left.text > right.text,
            },
        };
        KVal::of_bool(ok)
    }

    fn parse_add(&mut self) -> KVal {
        let mut left = self.parse_mul();
        loop {
            let save = self.pos;
            let Some(op) = self.try_op(&["+", "-"]) else { break };
            let rs = self.ws_end(self.pos);
            let right = self.parse_mul();
            left = match (left.num, right.num) {
                (Some(a), Some(b)) => KVal::of_num(if op == "+" { a + b } else { a - b }),
                _ if op == "+" => KVal::of_text(format!("{}{}", left.text, right.text)),
                _ => KVal::of_text(format!("{}{}{}", left.text, &self.src[save..rs], right.text)),
            };
        }
        left
    }

    fn parse_mul(&mut self) -> KVal {
        let mut left = self.parse_unary();
        loop {
            let save = self.pos;
            let Some(op) = self.try_op(&["*", "/", "%"]) else { break };
            let rs = self.ws_end(self.pos);
            let right = self.parse_unary();
            left = match (left.num, right.num) {
                (Some(a), Some(b)) => {
                    if op == "*" {
                        KVal::of_num(a * b)
                    } else if b.abs() < 1e-9 {
                        KVal::of_num(0.0)
                    } else if op == "/" {
                        KVal::of_num(a / b)
                    } else {
                        KVal::of_num(a % b)
                    }
                }
                _ => KVal::of_text(format!("{}{}{}", left.text, &self.src[save..rs], right.text)),
            };
        }
        left
    }

    fn parse_unary(&mut self) -> KVal {
        self.skip_ws();
        let c = self.at(self.pos);
        if matches!(c, Some(b'-') | Some(b'!')) && self.immediate_operand(self.pos + 1) {
            if self.depth >= MAX_DEPTH {
                self.pos = self.b.len();
                return KVal::empty();
            }
            self.pos += 1;
            self.depth += 1;
            let v = self.parse_unary();
            self.depth -= 1;
            if c == Some(b'!') {
                return KVal::of_bool(!v.is_truthy());
            }
            return match v.num {
                Some(n) => KVal::of_num(-n),
                None => KVal::of_text(format!("-{}", v.text)),
            };
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> KVal {
        self.skip_ws();
        let Some(c) = self.at(self.pos) else {
            return KVal::empty();
        };
        if c == b',' || c == b')' {
            return KVal::empty();
        }
        if c == b'(' {
            self.pos += 1;
            let v = self.parse_seq(false, true);
            if self.at(self.pos) == Some(b')') {
                self.pos += 1;
            }
            return v;
        }
        if c == b'"' || c == b'\'' {
            return KVal::of_text(self.read_string());
        }
        if c.is_ascii_digit() || (c == b'.' && self.at(self.pos + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = self.pos;
            while self.pos < self.b.len() && (self.b[self.pos].is_ascii_digit() || self.b[self.pos] == b'.') {
                self.pos += 1;
            }
            let raw = &self.src[start..self.pos];
            return KVal {
                text: raw.to_string(),
                num: parse_num(raw),
            };
        }
        if is_ident_start(c) {
            let start = self.pos;
            while self.pos < self.b.len() && is_ident(self.b[self.pos]) {
                self.pos += 1;
            }
            let src = self.src;
            let ident = &src[start..self.pos];
            let lower = ident.to_ascii_lowercase();
            if self.at(self.pos) == Some(b'(') {
                self.pos += 1;
                return self.call(&lower);
            }
            return self.find_local(&lower).unwrap_or_else(|| KVal::of_text(ident));
        }
        KVal::of_text(self.take_char())
    }

    fn read_string(&mut self) -> String {
        let quote = self.b[self.pos];
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.b.len() {
            let c = self.b[self.pos];
            if c == b'\\' && self.pos + 1 < self.b.len() {
                self.pos += 1;
                out.push_str(self.take_char());
            } else if c == quote {
                self.pos += 1;
                break;
            } else {
                out.push_str(self.take_char());
            }
        }
        out
    }

    /// Índice do `)` que fecha a chamada atual (respeita aninhamento e aspas).
    fn find_call_end(&self, from: usize) -> usize {
        let mut depth = 0u32;
        let mut quote: Option<u8> = None;
        let mut p = from;
        while p < self.b.len() {
            let c = self.b[p];
            if let Some(q) = quote {
                if c == b'\\' {
                    p += 2;
                    continue;
                }
                if c == q {
                    quote = None;
                }
            } else if c == b'"' || c == b'\'' {
                quote = Some(c);
            } else if c == b'(' {
                depth += 1;
            } else if c == b')' {
                if depth == 0 {
                    return p;
                }
                depth -= 1;
            }
            p += 1;
        }
        self.b.len()
    }

    fn call(&mut self, name: &str) -> KVal {
        if name == "df" {
            let end = self.find_call_end(self.pos).min(self.b.len());
            let src = self.src;
            // `p += 2` após `\` pode cair no meio de um char; recua até uma fronteira válida.
            let mut end_b = end;
            while end_b > self.pos && !src.is_char_boundary(end_b) {
                end_b -= 1;
            }
            let raw = &src[self.pos..end_b];
            self.pos = if end < self.b.len() { end + 1 } else { self.b.len() };
            return self.eval_df(raw);
        }
        let mut args: Vec<KVal> = Vec::with_capacity(4);
        self.skip_ws();
        if self.at(self.pos) == Some(b')') {
            self.pos += 1;
        } else {
            loop {
                let v = self.parse_seq(true, true);
                args.push(v);
                self.skip_ws();
                match self.at(self.pos) {
                    Some(b',') => self.pos += 1,
                    Some(b')') => {
                        self.pos += 1;
                        break;
                    }
                    _ => break,
                }
            }
        }
        self.eval_fn(name, &args)
    }

    fn eval_df(&mut self, raw: &str) -> KVal {
        let trimmed = trim_ws(raw);
        let mut pattern = if trimmed.contains('(') || trimmed.contains('"') {
            let mut sub = Parser::new(
                trimmed,
                self.ctx,
                self.locals.clone(),
                self.depth + 1,
                self.fl_depth,
            );
            let v = sub.parse_seq(false, false);
            self.deps.insert(sub.deps);
            v.text
        } else {
            trimmed.to_string()
        };
        if pattern.is_empty() {
            pattern = "HH:mm".to_string();
        }
        let now = self.ctx.now;
        if pattern == "S" {
            self.deps.insert(DirtyFlags::TIME_SECOND);
            return KVal::of_num(now.timestamp() as f64);
        }
        let toks = tokenize_pattern(&pattern, is_date_token);
        if toks.iter().any(|t| matches!(t, PatTok::Run('s', _))) {
            self.deps.insert(DirtyFlags::TIME_SECOND);
        } else {
            self.deps.insert(DirtyFlags::TIME_MINUTE);
        }
        KVal::of_text(format_date(&toks, &now))
    }

    fn eval_fn(&mut self, name: &str, args: &[KVal]) -> KVal {
        let key = |i: usize, def: &str| -> String {
            match args.get(i) {
                Some(a) => trim_ws(&a.text).to_lowercase(),
                None => def.to_string(),
            }
        };
        match name {
            "tf" => {
                let secs = args.first().and_then(|a| a.num).unwrap_or(0.0);
                let pattern = args.get(1).map(|a| trim_ws(&a.text)).unwrap_or("");
                KVal::of_text(format_duration(secs, pattern))
            }
            "gv" => {
                self.deps.insert(DirtyFlags::GLOBALS);
                let k = key(0, "");
                let found = self.ctx.globals.get(&k).or_else(|| {
                    self.ctx
                        .globals
                        .iter()
                        .find(|(name, _)| name.to_lowercase() == k)
                        .map(|(_, v)| v)
                });
                match found {
                    Some(v) => KVal::of_text(v.clone()),
                    None => KVal::of_text(args.get(1).map(|a| a.text.clone()).unwrap_or_default()),
                }
            }
            "rm" | "sys" | "si" => self.eval_sys(name, &key(0, "cpu")),
            "bi" => self.eval_sys(name, &key(0, "level")),
            "mi" => self.eval_mi(&key(0, "title")),
            "wi" => self.eval_wi(&key(0, "temp")),
            "ai" => self.eval_ai(&key(0, "isday")),
            "if" => {
                let mut i = 0;
                while i + 1 < args.len() {
                    if args[i].is_truthy() {
                        return args[i + 1].clone();
                    }
                    i += 2;
                }
                args.get(i).cloned().unwrap_or_else(KVal::empty)
            }
            "ce" => Self::eval_ce(args),
            "tc" => Self::eval_tc(args),
            "mu" => Self::eval_mu(args),
            "fl" => self.eval_fl(args),
            _ => KVal::empty(),
        }
    }

    fn eval_sys(&mut self, name: &str, k: &str) -> KVal {
        self.deps.insert(DirtyFlags::SYSTEM_METRICS);
        let ctx = self.ctx;
        let t = &ctx.system;
        let rm = || -> Option<KVal> {
            Some(match k {
                "cpu" | "cpuutil" => KVal::of_num(round_half_up(f64::from(t.cpu_percent))),
                "ram" | "mem" | "mused" => KVal::of_num(round_half_up(f64::from(t.ram_percent))),
                "ram_gb" | "mused_gb" => KVal::of_text(fixed(f64::from(t.ram_used_gb), 1)),
                "ram_total_gb" | "mtot_gb" => KVal::of_text(fixed(f64::from(t.ram_total_gb), 1)),
                "host" | "model" => KVal::of_text(t.hostname.clone()),
                _ => return None,
            })
        };
        let bi = || -> Option<KVal> {
            Some(match k {
                "level" | "bat" => KVal::of_num(f64::from(t.battery_percent)),
                "charging" => KVal::of_bool(t.battery_charging),
                "status" => KVal::of_text(if t.battery_charging {
                    "Carregando"
                } else {
                    "Na Bateria"
                }),
                _ => return None,
            })
        };
        let v = match name {
            "rm" => rm(),
            "bi" => bi(),
            _ => rm().or_else(bi),
        };
        v.unwrap_or_else(KVal::empty)
    }

    fn eval_mi(&mut self, k: &str) -> KVal {
        self.deps.insert(DirtyFlags::MEDIA);
        let ctx = self.ctx;
        let m = &ctx.media;
        match k {
            "title" => KVal::of_text(if m.title.is_empty() {
                "Sem mídia".to_string()
            } else {
                m.title.clone()
            }),
            "artist" => KVal::of_text(m.artist.clone()),
            "album" => KVal::of_text(m.album.clone()),
            "cover" => KVal::of_text(m.cover_path.clone()),
            "state" => KVal::of_text(if m.playing { "PLAYING" } else { "PAUSED" }),
            "playing" => KVal::of_bool(m.playing),
            "pos" | "dur" | "pct" => {
                if k != "dur" && m.playing {
                    self.deps.insert(DirtyFlags::TIME_SECOND);
                }
                let (pos, dur, ratio) = media_progress(m, ctx.now.timestamp_millis() as f64);
                match k {
                    "pos" => KVal::of_text(format_duration(pos, "mm:ss")),
                    "dur" => KVal::of_text(format_duration(dur, "mm:ss")),
                    _ => KVal::of_num(round_half_up(ratio * 100.0)),
                }
            }
            _ => KVal::empty(),
        }
    }

    fn eval_wi(&mut self, k: &str) -> KVal {
        self.deps.insert(DirtyFlags::WEATHER);
        let ctx = self.ctx;
        let w = &ctx.weather;
        match k {
            "temp" => match w.temp_c {
                Some(t) => KVal::of_num(round_half_up(f64::from(t))),
                None => KVal::of_text("--"),
            },
            "cond" => KVal::of_text(w.condition.clone()),
            "code" => KVal::of_num(f64::from(w.code)),
            "city" => KVal::of_text(if w.city.is_empty() {
                "Local".to_string()
            } else {
                w.city.clone()
            }),
            "isday" => KVal::of_bool(w.is_day),
            _ => KVal::empty(),
        }
    }

    fn eval_ai(&mut self, k: &str) -> KVal {
        self.deps.insert(DirtyFlags::WEATHER);
        match k {
            "sunrise" => KVal::of_text("06:00"),
            "sunset" => KVal::of_text("18:00"),
            "season" => KVal::of_text(match self.ctx.now.month() {
                12 | 1 | 2 => "Verão",
                3..=5 => "Outono",
                6..=8 => "Inverno",
                _ => "Primavera",
            }),
            _ => KVal::of_bool(self.ctx.weather.is_day),
        }
    }

    fn eval_ce(args: &[KVal]) -> KVal {
        let c1 = args
            .first()
            .and_then(|v| parse_argb_u32(&v.text))
            .unwrap_or(0xFFFF_FFFF);
        let Some(op) = args.get(1).map(|v| v.text.to_lowercase()) else {
            return KVal::of_text(format_argb_hex(c1));
        };
        let a1 = (c1 >> 24) & 0xFF;
        let r1 = (c1 >> 16) & 0xFF;
        let g1 = (c1 >> 8) & 0xFF;
        let b1 = c1 & 0xFF;
        match op.as_str() {
            "alpha" => {
                let pct = args.get(2).and_then(|v| v.num).unwrap_or(100.0).max(0.0).min(100.0);
                let a = round_half_up((pct / 100.0) * 255.0) as u32;
                KVal::of_text(format_argb_hex((c1 & 0x00FF_FFFF) | (a << 24)))
            }
            "mix" => {
                let c2 = args
                    .get(2)
                    .and_then(|v| parse_argb_u32(&v.text))
                    .unwrap_or(0xFFFF_FFFF);
                let t = (args.get(3).and_then(|v| v.num).unwrap_or(50.0) / 100.0)
                    .max(0.0)
                    .min(1.0);
                let lerp = |x: u32, y: u32| -> u32 {
                    round_half_up(f64::from(x) + (f64::from(y) - f64::from(x)) * t)
                        .max(0.0)
                        .min(255.0) as u32
                };
                let out = (lerp(a1, (c2 >> 24) & 0xFF) << 24)
                    | (lerp(r1, (c2 >> 16) & 0xFF) << 16)
                    | (lerp(g1, (c2 >> 8) & 0xFF) << 8)
                    | lerp(b1, c2 & 0xFF);
                KVal::of_text(format_argb_hex(out))
            }
            "contrast" => {
                let lum = (0.2126 * f64::from(r1) + 0.7152 * f64::from(g1) + 0.0722 * f64::from(b1)) / 255.0;
                KVal::of_text(if lum > 0.55 { "#FF000000" } else { "#FFFFFFFF" })
            }
            "lum" => KVal::of_num(
                (0.2126 * f64::from(r1) + 0.7152 * f64::from(g1) + 0.0722 * f64::from(b1)) / 2.55,
            ),
            _ => KVal::of_text(format_argb_hex(c1)),
        }
    }

    fn eval_tc(args: &[KVal]) -> KVal {
        let mode = args.first().map(|v| v.text.to_lowercase()).unwrap_or_default();
        let txt = args.get(1).map(|v| v.text.as_str()).unwrap_or("");
        let count = txt.chars().count();
        let num_arg = |i: usize| args.get(i).and_then(|v| v.num);
        match mode.as_str() {
            "up" | "upper" => KVal::of_text(txt.to_uppercase()),
            "low" | "lower" => KVal::of_text(txt.to_lowercase()),
            "cap" | "capitalize" => {
                let cap = txt
                    .split(' ')
                    .map(|w| {
                        let mut chars = w.chars();
                        match chars.next() {
                            Some(first) => {
                                let mut s: String = first.to_uppercase().collect();
                                s.push_str(&chars.as_str().to_lowercase());
                                s
                            }
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                KVal::of_text(cap)
            }
            "len" => KVal::of_num(count as f64),
            "cut" => {
                let n = num_arg(2).unwrap_or(count as f64).max(0.0).floor() as usize;
                KVal::of_text(txt.chars().take(n).collect::<String>())
            }
            "ell" => {
                let max = num_arg(2).unwrap_or(count as f64).max(1.0).floor() as usize;
                if count > max {
                    let mut s: String = txt.chars().take(max - 1).collect();
                    s.push('…');
                    KVal::of_text(s)
                } else {
                    KVal::of_text(txt)
                }
            }
            "split" => {
                let sep = args.get(2).map(|v| v.text.as_str()).unwrap_or(" ");
                let idx = num_arg(3).unwrap_or(0.0).max(0.0).floor() as usize;
                if sep.is_empty() {
                    KVal::of_text(txt.chars().nth(idx).map(String::from).unwrap_or_default())
                } else {
                    KVal::of_text(txt.split(sep).nth(idx).unwrap_or(""))
                }
            }
            "lpad" | "rpad" => {
                let width = num_arg(2).unwrap_or(0.0).max(0.0).min(64.0).floor() as usize;
                let def = if mode == "lpad" { '0' } else { ' ' };
                let pad = args.get(3).and_then(|v| v.text.chars().next()).unwrap_or(def);
                if count >= width {
                    return KVal::of_text(txt);
                }
                let fill: String = std::iter::repeat(pad).take(width - count).collect();
                if mode == "lpad" {
                    KVal::of_text(format!("{fill}{txt}"))
                } else {
                    KVal::of_text(format!("{txt}{fill}"))
                }
            }
            _ => KVal::of_text(txt),
        }
    }

    fn eval_mu(args: &[KVal]) -> KVal {
        let op = args.first().map(|v| v.text.to_lowercase()).unwrap_or_default();
        let nums: Vec<f64> = args.iter().skip(1).filter_map(|v| v.num).collect();
        let Some(&x) = nums.first() else {
            return KVal::of_num(0.0);
        };
        let val = match op.as_str() {
            "min" => nums.iter().copied().fold(f64::INFINITY, |a, b| if b < a { b } else { a }),
            "max" => nums
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, |a, b| if b > a { b } else { a }),
            "abs" => x.abs(),
            "round" => round_half_up(x),
            "floor" => x.floor(),
            "ceil" => x.ceil(),
            "pow" => x.powf(nums.get(1).copied().unwrap_or(1.0)),
            "sqrt" => x.max(0.0).sqrt(),
            "sin" => (x * (std::f64::consts::PI / 180.0)).sin(),
            "cos" => (x * (std::f64::consts::PI / 180.0)).cos(),
            "clamp" => {
                let lo = nums.get(1).copied().unwrap_or(0.0);
                let hi = nums.get(2).copied().unwrap_or_else(|| lo.max(100.0));
                lo.max(hi).min(lo.min(hi).max(x))
            }
            _ => x,
        };
        KVal::of_num(val)
    }

    fn eval_fl(&mut self, args: &[KVal]) -> KVal {
        if self.fl_depth >= MAX_FL_DEPTH {
            return KVal::empty();
        }
        let int = |i: usize, def: f64| -> i64 {
            round_half_up(args.get(i).and_then(|v| v.num).unwrap_or(def).max(-1e9).min(1e9)) as i64
        };
        let start = int(0, 1.0);
        let end = int(1, 1.0);
        let step = int(2, 1.0).abs().max(1);
        let body = args.get(3).map(|v| v.text.as_str()).unwrap_or("");
        let sep = args.get(4).map(|v| v.text.as_str()).unwrap_or("");
        let dir: i64 = if end >= start { 1 } else { -1 };
        let mut out = String::new();
        let mut i = start;
        let mut count = 0usize;
        while ((dir > 0 && i <= end) || (dir < 0 && i >= end)) && count < MAX_FL_ITERS {
            if count > 0 {
                out.push_str(sep);
            }
            let mut locals = self.locals.clone();
            locals.push(("i".to_string(), KVal::of_num(i as f64)));
            let mut sub = Parser::new(body, self.ctx, locals, self.depth + 1, self.fl_depth + 1);
            out.push_str(&sub.parse_seq(false, false).text);
            self.deps.insert(sub.deps);
            i += step * dir;
            count += 1;
        }
        KVal::of_text(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const PARITY: &str = include_str!("fx_parity_cases.json");

    fn parity_ctx(fixture: &serde_json::Value) -> EvalContext {
        let c = &fixture["context"];
        let n: Vec<u32> = c["now"]
            .as_array()
            .expect("now")
            .iter()
            .map(|v| v.as_u64().expect("now[i]") as u32)
            .collect();
        let mut ctx = EvalContext::default();
        ctx.now = Local
            .with_ymd_and_hms(n[0] as i32, n[1], n[2], n[3], n[4], n[5])
            .single()
            .expect("data local válida");
        let t = &c["telemetry"];
        let f = |k: &str| t[k].as_f64().expect(k);
        ctx.system.cpu_percent = f("cpu_percent") as f32;
        ctx.system.ram_percent = f("ram_percent") as f32;
        ctx.system.ram_used_gb = f("ram_used_gb") as f32;
        ctx.system.ram_total_gb = f("ram_total_gb") as f32;
        ctx.system.battery_percent = f("battery_percent") as u8;
        ctx.system.battery_charging = t["battery_charging"].as_bool().expect("charging");
        ctx.system.hostname = t["hostname"].as_str().expect("hostname").to_string();
        let m = &t["media"];
        ctx.media.title = m["title"].as_str().unwrap_or("").to_string();
        ctx.media.artist = m["artist"].as_str().unwrap_or("").to_string();
        ctx.media.album = m["album"].as_str().unwrap_or("").to_string();
        ctx.media.cover_path = m["cover_path"].as_str().unwrap_or("").to_string();
        ctx.media.playing = m["playing"].as_bool().unwrap_or(false);
        ctx.media.position_secs = m["position_secs"].as_f64().unwrap_or(0.0);
        ctx.media.duration_secs = m["duration_secs"].as_f64().unwrap_or(0.0);
        let ago = c["media_updated_ago_ms"].as_f64().unwrap_or(0.0);
        ctx.media.updated_at_ms = ctx.now.timestamp_millis() as f64 - ago;
        if let Some(g) = c["globals"].as_object() {
            for (k, v) in g {
                ctx.globals
                    .insert(k.to_lowercase(), v.as_str().unwrap_or("").to_string());
            }
        }
        ctx
    }

    #[test]
    fn parity_cases_match_studio() {
        let fixture: serde_json::Value = serde_json::from_str(PARITY).expect("json");
        let ctx = parity_ctx(&fixture);
        let mut failures = Vec::new();
        for case in fixture["cases"].as_array().expect("cases") {
            let input = case[0].as_str().expect("input");
            let expected = case[1].as_str().expect("expected");
            let got = FxEngine::eval_text(input, &ctx).value;
            if got != expected {
                failures.push(format!("{input:?}: esperado {expected:?}, obtido {got:?}"));
            }
        }
        assert!(failures.is_empty(), "falhas de paridade:\n{}", failures.join("\n"));
    }

    #[test]
    fn nested_tc_df_long_tokens_first() {
        let mut ctx = EvalContext::default();
        ctx.now = Local.with_ymd_and_hms(2026, 11, 21, 14, 5, 9).single().expect("data");
        let r = FxEngine::eval_text("$tc(up, df(EEEE, dd MMMM))$", &ctx);
        assert_eq!(r.value, "SÁBADO, 21 NOVEMBRO");
        assert!(r.deps.contains(DirtyFlags::TIME_MINUTE));
        assert!(!r.deps.contains(DirtyFlags::TIME_SECOND));
        // Sem re-processamento: "Sábado"/"Novembro" não são lidos de novo como padrão.
        assert_eq!(FxEngine::eval_text("$df(EEEE MMMM)$", &ctx).value, "Sábado Novembro");
        assert_eq!(FxEngine::eval_text("$df(EEE MMM)$", &ctx).value, "Sáb Nov");
    }

    #[test]
    fn evaluates_fx_and_tracks_dirty_flags() {
        let mut ctx = EvalContext::default();
        ctx.now = Local.with_ymd_and_hms(2026, 11, 18, 18, 45, 30).single().expect("data");
        ctx.system.cpu_percent = 42.0;
        ctx.globals.insert("accent".to_string(), "#FF00D2FF".to_string());

        let r1 = FxEngine::eval_text("$df(HH:mm)$ · $sys(cpu)$%", &ctx);
        assert_eq!(r1.value, "18:45 · 42%");
        assert!(r1.deps.contains(DirtyFlags::TIME_MINUTE));
        assert!(r1.deps.contains(DirtyFlags::SYSTEM_METRICS));
        assert!(!r1.deps.contains(DirtyFlags::TIME_SECOND));

        let r2 = FxEngine::eval_expr("if(sys(cpu) > 30, gv(accent), \"#FFFFFFFF\")", &ctx);
        assert_eq!(r2.value, "#FF00D2FF");
        assert!(r2.deps.contains(DirtyFlags::GLOBALS));

        let r3 = FxEngine::eval_text("$df(ss)$", &ctx);
        assert!(r3.deps.contains(DirtyFlags::TIME_SECOND));
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let ctx = EvalContext::default();
        let deep = format!("${}1{}$", "(".repeat(500), ")".repeat(500));
        let _ = FxEngine::eval_text(&deep, &ctx);
        let neg = format!("${}5$", "-".repeat(500));
        let _ = FxEngine::eval_text(&neg, &ctx);
    }
}
