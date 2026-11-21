use crate::schema::{DirtyFlags, LayerNode, PrismScene, StaticOrFx, WeatherConfig};
use chrono::{DateTime, Datelike, Local};
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

#[derive(Debug, Clone)]
struct KVal {
    text: String,
    num: Option<f64>,
}

impl KVal {
    fn of_text(s: impl Into<String>) -> Self {
        let text = s.into();
        let num = text.trim().parse::<f64>().ok();
        Self { text, num }
    }

    fn of_num(n: f64) -> Self {
        if !n.is_finite() {
            return Self {
                text: "0".to_string(),
                num: Some(0.0),
            };
        }
        let text = if (n - n.round()).abs() < 1e-9 && n.abs() < 1e12 {
            format!("{}", n.round() as i64)
        } else {
            format!("{:.2}", n)
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        };
        Self { text, num: Some(n) }
    }

    fn of_bool(b: bool) -> Self {
        Self {
            text: if b { "1" } else { "0" }.to_string(),
            num: Some(if b { 1.0 } else { 0.0 }),
        }
    }

    fn is_truthy(&self) -> bool {
        if let Some(n) = self.num {
            n.is_finite() && n != 0.0
        } else {
            let t = self.text.trim();
            !t.is_empty() && !t.eq_ignore_ascii_case("false") && t != "0"
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
        let bytes = input.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                if let Some(close) = find_closing_dollar(input, i + 1) {
                    let expr = &input[i + 1..close];
                    let mut parser = Parser::new(expr, ctx, HashMap::new());
                    let val = parser.parse_expr();
                    deps.insert(parser.deps);
                    out.push_str(&val.text);
                    i = close + 1;
                    continue;
                }
                out.push_str(&input[i..]);
                break;
            }
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        EvalOutcome { value: out, deps }
    }

    pub fn eval_expr(expr: &str, ctx: &EvalContext) -> EvalOutcome<String> {
        let trimmed = expr.trim();
        if trimmed.is_empty() {
            return EvalOutcome {
                value: String::new(),
                deps: DirtyFlags::NONE,
            };
        }
        if trimmed.contains('$') {
            return Self::eval_text(trimmed, ctx);
        }
        let mut parser = Parser::new(trimmed, ctx, HashMap::new());
        let val = parser.parse_expr();
        EvalOutcome {
            value: val.text,
            deps: parser.deps,
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
        deps.insert(Self::eval_f32(&layer.crystal.frost_amount, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.dispersion_rgb, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.refraction, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.bevel_width, ctx).deps);
        deps.insert(Self::eval_f32(&layer.crystal.fresnel_intensity, ctx).deps);
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
    let s = raw.trim().trim_start_matches('#');
    match s.len() {
        6 => u32::from_str_radix(s, 16).ok().map(|rgb| 0xFF00_0000 | rgb),
        8 => u32::from_str_radix(s, 16).ok(),
        _ => None,
    }
}

fn format_argb_hex(c: u32) -> String {
    format!("#{:08X}", c)
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    ctx: &'a EvalContext,
    locals: HashMap<String, KVal>,
    deps: DirtyFlags,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, ctx: &'a EvalContext, locals: HashMap<String, KVal>) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            ctx,
            locals,
            deps: DirtyFlags::NONE,
        }
    }

    fn ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, tok: &str) -> bool {
        self.ws();
        if self.src[self.pos..].starts_with(tok) {
            self.pos += tok.len();
            true
        } else {
            false
        }
    }

    fn peek_op<'b>(&mut self, ops: &[&'b str]) -> Option<&'b str> {
        self.ws();
        let rem = &self.src[self.pos..];
        ops.iter().copied().find(|op| rem.starts_with(op))
    }

    fn parse_expr(&mut self) -> KVal {
        self.parse_or()
    }

    fn parse_or(&mut self) -> KVal {
        let mut left = self.parse_and();
        while self.eat("||") || self.eat("|") {
            let right = self.parse_and();
            left = KVal::of_bool(left.is_truthy() || right.is_truthy());
        }
        left
    }

    fn parse_and(&mut self) -> KVal {
        let mut left = self.parse_cmp();
        while self.eat("&&") || self.eat("&") {
            let right = self.parse_cmp();
            left = KVal::of_bool(left.is_truthy() && right.is_truthy());
        }
        left
    }

    fn parse_cmp(&mut self) -> KVal {
        let left = self.parse_add();
        let Some(op) = self.peek_op(&["<=", ">=", "==", "!=", "=", "<", ">"]) else {
            return left;
        };
        self.pos += op.len();
        let right = self.parse_add();
        let ok = match (left.num, right.num) {
            (Some(a), Some(b)) => match op {
                "<=" => a <= b,
                ">=" => a >= b,
                "==" | "=" => (a - b).abs() < 1e-9,
                "!=" => (a - b).abs() >= 1e-9,
                "<" => a < b,
                _ => a > b,
            },
            _ => match op {
                "<=" => left.text <= right.text,
                ">=" => left.text >= right.text,
                "==" | "=" => left.text == right.text,
                "!=" => left.text != right.text,
                "<" => left.text < right.text,
                _ => left.text > right.text,
            },
        };
        KVal::of_bool(ok)
    }

    fn parse_add(&mut self) -> KVal {
        let mut left = self.parse_mul();
        loop {
            if self.eat("+") {
                let right = self.parse_mul();
                left = match (left.num, right.num) {
                    (Some(a), Some(b)) => KVal::of_num(a + b),
                    _ => KVal::of_text(format!("{}{}", left.text, right.text)),
                };
            } else if self.eat("-") {
                let right = self.parse_mul();
                left = KVal::of_num(left.num.unwrap_or(0.0) - right.num.unwrap_or(0.0));
            } else {
                break;
            }
        }
        left
    }

    fn parse_mul(&mut self) -> KVal {
        let mut left = self.parse_unary();
        loop {
            if self.eat("*") {
                let right = self.parse_unary();
                left = KVal::of_num(left.num.unwrap_or(0.0) * right.num.unwrap_or(0.0));
            } else if self.eat("/") {
                let right = self.parse_unary();
                let r = right.num.unwrap_or(1.0);
                left = KVal::of_num(if r.abs() < 1e-9 {
                    0.0
                } else {
                    left.num.unwrap_or(0.0) / r
                });
            } else if self.eat("%") {
                let right = self.parse_unary();
                let r = right.num.unwrap_or(1.0);
                left = KVal::of_num(if r.abs() < 1e-9 {
                    0.0
                } else {
                    left.num.unwrap_or(0.0) % r
                });
            } else {
                break;
            }
        }
        left
    }

    fn parse_unary(&mut self) -> KVal {
        if self.eat("-") {
            let v = self.parse_unary();
            return KVal::of_num(-v.num.unwrap_or(0.0));
        }
        self.ws();
        if self.src[self.pos..].starts_with('!') && !self.src[self.pos..].starts_with("!=") {
            self.pos += 1;
            return KVal::of_bool(!self.parse_unary().is_truthy());
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> KVal {
        self.ws();
        if self.pos >= self.bytes.len() {
            return KVal::of_text("");
        }
        let b = self.bytes[self.pos];
        if b == b'(' {
            self.pos += 1;
            let v = self.parse_expr();
            self.eat(")");
            return v;
        }
        if b == b'"' || b == b'\'' {
            return KVal::of_text(self.read_string());
        }
        if b.is_ascii_digit()
            || (b == b'.' && self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1].is_ascii_digit())
        {
            let start = self.pos;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'.')
            {
                self.pos += 1;
            }
            let raw = &self.src[start..self.pos];
            return KVal {
                text: raw.to_string(),
                num: raw.parse::<f64>().ok(),
            };
        }
        let start = self.pos;
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_alphanumeric() || self.bytes[self.pos] == b'_')
        {
            self.pos += 1;
        }
        let ident = &self.src[start..self.pos];
        if ident.is_empty() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            self.pos += ch.len_utf8();
            return KVal::of_text(ch.to_string());
        }
        self.ws();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'(' {
            return self.parse_call(&ident.to_ascii_lowercase());
        }
        if let Some(local) = self.locals.get(&ident.to_ascii_lowercase()) {
            return local.clone();
        }
        KVal::of_text(ident)
    }

    fn read_string(&mut self) -> String {
        let quote = self.bytes[self.pos];
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'\\' && self.pos + 1 < self.bytes.len() {
                self.pos += 1;
                let ch = self.src[self.pos..].chars().next().unwrap();
                out.push(ch);
                self.pos += ch.len_utf8();
            } else if b == quote {
                self.pos += 1;
                break;
            } else {
                let ch = self.src[self.pos..].chars().next().unwrap();
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        out
    }

    fn parse_call(&mut self, name: &str) -> KVal {
        self.pos += 1; // '('
        if name == "df" {
            self.ws();
            let raw_arg = if self.pos < self.bytes.len()
                && self.bytes[self.pos] != b'"'
                && self.bytes[self.pos] != b'\''
                && !self.has_nested_call_or_concat()
            {
                self.read_raw_until_delim()
            } else {
                self.parse_expr().text
            };
            self.eat(")");
            return self.eval_df(&raw_arg);
        }
        let mut args = Vec::with_capacity(4);
        self.ws();
        if self.pos < self.bytes.len() && self.bytes[self.pos] != b')' {
            loop {
                args.push(self.parse_expr());
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.eat(")");
        self.eval_function(name, &args)
    }

    fn has_nested_call_or_concat(&self) -> bool {
        for &b in &self.bytes[self.pos..] {
            if b == b'(' || b == b'+' {
                return true;
            }
            if b == b')' {
                return false;
            }
        }
        false
    }

    fn read_raw_until_delim(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.bytes.len()
            && self.bytes[self.pos] != b')'
            && self.bytes[self.pos] != b','
        {
            self.pos += 1;
        }
        self.src[start..self.pos].trim().to_string()
    }

    fn eval_function(&mut self, name: &str, args: &[KVal]) -> KVal {
        match name {
            "df" => self.eval_df(args.first().map(|v| v.text.as_str()).unwrap_or("HH:mm")),
            "gv" => self.eval_gv(args),
            "sys" | "bi" | "si" => self.eval_sys(name, args.first().map(|v| v.text.as_str()).unwrap_or("cpu")),
            "mi" => self.eval_mi(args.first().map(|v| v.text.as_str()).unwrap_or("state")),
            "wi" => self.eval_wi(args.first().map(|v| v.text.as_str()).unwrap_or("temp")),
            "ai" => self.eval_ai(args.first().map(|v| v.text.as_str()).unwrap_or("isday")),
            "if" => self.eval_if(args),
            "ce" => self.eval_ce(args),
            "tc" => self.eval_tc(args),
            "mu" => self.eval_mu(args),
            "fl" => self.eval_fl(args),
            _ => KVal::of_text(""),
        }
    }

    fn eval_df(&mut self, pattern: &str) -> KVal {
        if pattern.contains("ss") || pattern == "S" || pattern == "f" {
            self.deps.insert(DirtyFlags::TIME_SECOND);
        } else {
            self.deps.insert(DirtyFlags::TIME_MINUTE);
        }
        let now = self.ctx.now;
        if pattern == "D" {
            return KVal::of_num(f64::from(now.ordinal()));
        }
        if pattern == "S" || pattern == "f" {
            return KVal::of_num(now.timestamp() as f64);
        }
        let chrono_fmt = translate_df_pattern(pattern);
        KVal::of_text(now.format(&chrono_fmt).to_string())
    }

    fn eval_gv(&mut self, args: &[KVal]) -> KVal {
        self.deps.insert(DirtyFlags::GLOBALS);
        let key = args
            .first()
            .map(|v| v.text.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let fallback = args.get(1).map(|v| v.text.as_str()).unwrap_or("");
        let val = self
            .ctx
            .globals
            .get(&key)
            .map(String::as_str)
            .unwrap_or(fallback);
        KVal::of_text(val)
    }

    fn eval_sys(&mut self, fn_name: &str, param: &str) -> KVal {
        self.deps.insert(DirtyFlags::SYSTEM_METRICS);
        let s = &self.ctx.system;
        match (fn_name, param.to_ascii_lowercase().as_str()) {
            (_, "cpu") => KVal::of_num(f64::from(s.cpu_percent)),
            (_, "ram" | "mem") => KVal::of_num(f64::from(s.ram_percent)),
            (_, "bat" | "level") => KVal::of_num(f64::from(s.battery_percent)),
            (_, "charging") => KVal::of_bool(s.battery_charging),
            ("bi", "status") => KVal::of_text(if s.battery_charging {
                "Carregando"
            } else {
                "Na Bateria"
            }),
            (_, "temp") => KVal::of_num(f64::from(s.cpu_temp_c)),
            (_, "vol" | "volm") => KVal::of_num(f64::from(s.volume_percent)),
            (_, "uptime") => KVal::of_num(s.uptime_secs as f64),
            (_, "host" | "model") => KVal::of_text(s.hostname.clone()),
            (_, "mplaying") => {
                self.deps.insert(DirtyFlags::MEDIA);
                KVal::of_bool(self.ctx.media.playing)
            }
            _ => KVal::of_text(""),
        }
    }

    fn eval_mi(&mut self, param: &str) -> KVal {
        self.deps.insert(DirtyFlags::MEDIA);
        let m = &self.ctx.media;
        match param.to_ascii_lowercase().as_str() {
            "title" => KVal::of_text(m.title.clone()),
            "artist" => KVal::of_text(m.artist.clone()),
            "album" => KVal::of_text(m.album.clone()),
            "cover" => KVal::of_text(m.cover_path.clone()),
            "state" => KVal::of_text(if m.playing { "PLAYING" } else { "PAUSED" }),
            "playing" => KVal::of_bool(m.playing),
            _ => KVal::of_text(""),
        }
    }

    fn eval_wi(&mut self, param: &str) -> KVal {
        self.deps.insert(DirtyFlags::WEATHER);
        let w = &self.ctx.weather;
        match param.to_ascii_lowercase().as_str() {
            "temp" => match w.temp_c {
                Some(t) => KVal::of_num(f64::from(t.round())),
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
            _ => KVal::of_text(""),
        }
    }

    fn eval_ai(&mut self, param: &str) -> KVal {
        self.deps.insert(DirtyFlags::WEATHER);
        match param.to_ascii_lowercase().as_str() {
            "isday" => KVal::of_bool(self.ctx.weather.is_day),
            "sunrise" => KVal::of_text("06:00"),
            "sunset" => KVal::of_text("18:00"),
            "season" => {
                let s = match self.ctx.now.month() {
                    12 | 1 | 2 => "Verão",
                    3 | 4 | 5 => "Outono",
                    6 | 7 | 8 => "Inverno",
                    _ => "Primavera",
                };
                KVal::of_text(s)
            }
            _ => KVal::of_bool(self.ctx.weather.is_day),
        }
    }

    fn eval_if(&self, args: &[KVal]) -> KVal {
        let mut i = 0;
        while i + 1 < args.len() {
            if args[i].is_truthy() {
                return args[i + 1].clone();
            }
            i += 2;
        }
        args.get(i).cloned().unwrap_or_else(|| KVal::of_text(""))
    }

    fn eval_ce(&self, args: &[KVal]) -> KVal {
        let c1 = args
            .first()
            .and_then(|v| parse_argb_u32(&v.text))
            .unwrap_or(0xFFFF_FFFF);
        let Some(op) = args.get(1).map(|v| v.text.to_ascii_lowercase()) else {
            return KVal::of_text(format_argb_hex(c1));
        };
        let a1 = (c1 >> 24) & 0xFF;
        let r1 = (c1 >> 16) & 0xFF;
        let g1 = (c1 >> 8) & 0xFF;
        let b1 = c1 & 0xFF;
        match op.as_str() {
            "alpha" => {
                let pct = args.get(2).and_then(|v| v.num).unwrap_or(100.0).clamp(0.0, 100.0);
                let a = ((pct / 100.0) * 255.0).round() as u32;
                KVal::of_text(format_argb_hex((c1 & 0x00FF_FFFF) | (a << 24)))
            }
            "mix" => {
                let c2 = args
                    .get(2)
                    .and_then(|v| parse_argb_u32(&v.text))
                    .unwrap_or(0xFFFF_FFFF);
                let t = (args.get(3).and_then(|v| v.num).unwrap_or(50.0) / 100.0).clamp(0.0, 1.0);
                let lerp = |x: u32, y: u32| -> u32 {
                    ((x as f64) + ((y as f64) - (x as f64)) * t).round().clamp(0.0, 255.0) as u32
                };
                let out = (lerp(a1, (c2 >> 24) & 0xFF) << 24)
                    | (lerp(r1, (c2 >> 16) & 0xFF) << 16)
                    | (lerp(g1, (c2 >> 8) & 0xFF) << 8)
                    | lerp(b1, c2 & 0xFF);
                KVal::of_text(format_argb_hex(out))
            }
            "contrast" => {
                let lum = (0.2126 * r1 as f64 + 0.7152 * g1 as f64 + 0.0722 * b1 as f64) / 255.0;
                KVal::of_text(if lum > 0.55 { "#FF000000" } else { "#FFFFFFFF" })
            }
            "lum" => {
                let lum = (0.2126 * r1 as f64 + 0.7152 * g1 as f64 + 0.0722 * b1 as f64) / 2.55;
                KVal::of_num(lum)
            }
            _ => KVal::of_text(format_argb_hex(c1)),
        }
    }

    fn eval_tc(&self, args: &[KVal]) -> KVal {
        let mode = args
            .first()
            .map(|v| v.text.to_ascii_lowercase())
            .unwrap_or_default();
        let txt = args.get(1).map(|v| v.text.as_str()).unwrap_or("");
        match mode.as_str() {
            "up" | "upper" => KVal::of_text(txt.to_uppercase()),
            "low" | "lower" => KVal::of_text(txt.to_lowercase()),
            "cap" | "capitalize" => {
                let cap = txt
                    .split(' ')
                    .map(|w| {
                        let mut chars = w.chars();
                        match chars.next() {
                            Some(first) => format!(
                                "{}{}",
                                first.to_uppercase(),
                                chars.as_str().to_lowercase()
                            ),
                            None => String::new(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                KVal::of_text(cap)
            }
            "len" => KVal::of_num(txt.chars().count() as f64),
            "cut" => {
                let max_len = args.get(2).and_then(|v| v.num).unwrap_or(txt.len() as f64).max(0.0) as usize;
                KVal::of_text(txt.chars().take(max_len).collect::<String>())
            }
            "ell" => {
                let max_len = args.get(2).and_then(|v| v.num).unwrap_or(txt.len() as f64).max(1.0) as usize;
                let count = txt.chars().count();
                if count > max_len {
                    let mut s: String = txt.chars().take(max_len - 1).collect();
                    s.push('…');
                    KVal::of_text(s)
                } else {
                    KVal::of_text(txt)
                }
            }
            "split" => {
                let sep = args.get(2).map(|v| v.text.as_str()).unwrap_or(" ");
                let idx = args.get(3).and_then(|v| v.num).unwrap_or(0.0).max(0.0) as usize;
                KVal::of_text(txt.split(sep).nth(idx).unwrap_or(""))
            }
            "lpad" => {
                let width = args.get(2).and_then(|v| v.num).unwrap_or(0.0).clamp(0.0, 64.0) as usize;
                let pad = args.get(3).and_then(|v| v.text.chars().next()).unwrap_or('0');
                let cur = txt.chars().count();
                if cur >= width {
                    KVal::of_text(txt)
                } else {
                    KVal::of_text(format!("{}{}", pad.to_string().repeat(width - cur), txt))
                }
            }
            "rpad" => {
                let width = args.get(2).and_then(|v| v.num).unwrap_or(0.0).clamp(0.0, 64.0) as usize;
                let pad = args.get(3).and_then(|v| v.text.chars().next()).unwrap_or(' ');
                let cur = txt.chars().count();
                if cur >= width {
                    KVal::of_text(txt)
                } else {
                    KVal::of_text(format!("{}{}", txt, pad.to_string().repeat(width - cur)))
                }
            }
            _ => KVal::of_text(txt),
        }
    }

    fn eval_mu(&self, args: &[KVal]) -> KVal {
        let op = args
            .first()
            .map(|v| v.text.to_ascii_lowercase())
            .unwrap_or_default();
        let nums: Vec<f64> = args.iter().skip(1).filter_map(|v| v.num).collect();
        if nums.is_empty() {
            return KVal::of_num(0.0);
        }
        let val = match op.as_str() {
            "min" => nums.iter().copied().fold(f64::INFINITY, f64::min),
            "max" => nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            "abs" => nums[0].abs(),
            "round" => nums[0].round(),
            "floor" => nums[0].floor(),
            "ceil" => nums[0].ceil(),
            "pow" => nums[0].powf(*nums.get(1).unwrap_or(&1.0)),
            "sqrt" => nums[0].max(0.0).sqrt(),
            "sin" => nums[0].to_radians().sin(),
            "cos" => nums[0].to_radians().cos(),
            "clamp" => {
                let lo = *nums.get(1).unwrap_or(&0.0);
                let hi = *nums.get(2).unwrap_or(&lo.max(100.0));
                nums[0].clamp(lo.min(hi), lo.max(hi))
            }
            _ => nums[0],
        };
        KVal::of_num(val)
    }

    fn eval_fl(&mut self, args: &[KVal]) -> KVal {
        let start = args.first().and_then(|v| v.num).unwrap_or(1.0).round() as i32;
        let end = args.get(1).and_then(|v| v.num).unwrap_or(1.0).round() as i32;
        let step = args
            .get(2)
            .and_then(|v| v.num)
            .unwrap_or(1.0)
            .round()
            .abs()
            .max(1.0) as i32;
        let body = args.get(3).map(|v| v.text.as_str()).unwrap_or("");
        let sep = args.get(4).map(|v| v.text.as_str()).unwrap_or("");
        let dir = if end >= start { 1 } else { -1 };
        let mut out = String::new();
        let mut i = start;
        let mut count = 0usize;
        while ((dir > 0 && i <= end) || (dir < 0 && i >= end)) && count < 64 {
            if count > 0 && !sep.is_empty() {
                out.push_str(sep);
            }
            let mut next_locals = self.locals.clone();
            next_locals.insert("i".to_string(), KVal::of_num(f64::from(i)));
            let mut sub = Parser::new(body, self.ctx, next_locals);
            out.push_str(&sub.parse_expr().text);
            self.deps.insert(sub.deps);
            i += step * dir;
            count += 1;
        }
        KVal::of_text(out)
    }
}

fn translate_df_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() * 2);
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let mut run = 1;
        while i + run < chars.len() && chars[i + run] == ch {
            run += 1;
        }
        match (ch, run) {
            ('H', 2) => out.push_str("%H"),
            ('H', 1) => out.push_str("%-H"),
            ('h', 2) => out.push_str("%I"),
            ('h', 1) => out.push_str("%-I"),
            ('m', 2) => out.push_str("%M"),
            ('m', 1) => out.push_str("%-M"),
            ('s', 2) => out.push_str("%S"),
            ('s', 1) => out.push_str("%-S"),
            ('a', _) => out.push_str("%p"),
            ('E', 4..) => out.push_str("%A"),
            ('E', 1..=3) => out.push_str("%a"),
            ('d', 2) => out.push_str("%d"),
            ('d', 1) => out.push_str("%-d"),
            ('M', 4..) => out.push_str("%B"),
            ('M', 3) => out.push_str("%b"),
            ('M', 2) => out.push_str("%m"),
            ('M', 1) => out.push_str("%-m"),
            ('y', 4..) => out.push_str("%Y"),
            ('y', 2) => out.push_str("%y"),
            ('%', _) => {
                for _ in 0..run {
                    out.push_str("%%");
                }
            }
            _ => {
                for _ in 0..run {
                    out.push(ch);
                }
            }
        }
        i += run;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn evaluates_fx_and_tracks_dirty_flags() {
        let mut ctx = EvalContext::default();
        ctx.now = Local.with_ymd_and_hms(2026, 11, 18, 18, 45, 30).unwrap();
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
    }
}
