//! SignalBus (Fase 4): barramento lock-free entre os sensores de plataforma (threads produtoras)
//! e o renderer (consumidor, thread do loop).
//!
//! Desenho:
//! - Escalares e contadores são atômicos (`f32` guardado como bits em `AtomicU32`).
//! - As 64 bandas de áudio + agregados usam um **seqlock** de escritor único (a thread WASAPI):
//!   o leitor nunca bloqueia nem aloca; numa corrida rara tenta de novo (máx. `SEQ_RETRIES`).
//! - Entradas de mouse/teclado são **acumuladores** (`fetch_add`) que o leitor zera com `swap`:
//!   nada se perde entre frames, e a ordem de chegada não importa.
//! - Texto de mídia (título/artista/álbum/capa) muda raramente: fica atrás de um `Mutex` mas o
//!   renderer só o toca quando `media_version` mudou e via `try_lock` (nunca espera no frame).
//! - Demanda: o renderer declara o que a cena usa (`set_demand`); produtores ociosos dormem
//!   (0 % de CPU). Wake: cada `publish` marca um bit `dirty`; se o bit estava limpo e está em
//!   `wake_mask`, chama o waker da plataforma (no Windows `LoopProxy::wake`). Assim uma cena
//!   estática com binding de mouse acorda no máximo 1× por frame consumido, e silêncio não acorda.
//!
//! Este módulo não conhece SO: a plataforma injeta `waker`/`demand_listener` como closures.

use std::ops::{BitAnd, BitOr, BitOrAssign};
use std::sync::atomic::{fence, AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::audio::{AudioFrame, BAND_COUNT};

const SEQ_RETRIES: u32 = 8;

// ───────────────────────── SignalKind (bitmask) ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct SignalKind(pub u32);

impl SignalKind {
    pub const NONE: Self = Self(0);
    pub const AUDIO: Self = Self(1 << 0);
    pub const MOUSE: Self = Self(1 << 1);
    pub const KEYBOARD: Self = Self(1 << 2);
    pub const MEDIA: Self = Self(1 << 3);
    pub const TELEMETRY: Self = Self(1 << 4);
    /// Bateria/energia (governador). Barato: amostrado em baixa frequência.
    pub const POWER: Self = Self(1 << 5);
    /// Clique curto (toque) no fundo da área de trabalho — fora de ícones e de outras janelas.
    /// Separado de `MOUSE` para o loop acordar só em toques, não a cada movimento.
    pub const DESKTOP_TAP: Self = Self(1 << 6);
    pub const INPUT: Self = Self(Self::MOUSE.0 | Self::KEYBOARD.0);
    pub const ALL: Self = Self(0x7F);

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for SignalKind {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}
impl BitOrAssign for SignalKind {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
impl BitAnd for SignalKind {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

// ───────────────────────── Atômicos auxiliares ─────────────────────────

#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub const fn zero() -> Self {
        Self(AtomicU32::new(0))
    }
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    #[inline]
    pub fn load(&self, order: Ordering) -> f32 {
        f32::from_bits(self.0.load(order))
    }
    #[inline]
    pub fn store(&self, v: f32, order: Ordering) {
        self.0.store(v.to_bits(), order);
    }
}

/// Botões do mouse (bitmask em `MouseState::buttons`).
pub mod buttons {
    pub const LEFT: u32 = 1 << 0;
    pub const RIGHT: u32 = 1 << 1;
    pub const MIDDLE: u32 = 1 << 2;
    pub const X1: u32 = 1 << 3;
    pub const X2: u32 = 1 << 4;
}

// ───────────────────────── Dados publicados ─────────────────────────

/// Texto de mídia (cópia fora do caminho quente; ver `SignalBus::read_media_text`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaText {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// App de origem (AUMID), ex.: `Spotify.exe`.
    pub source_app: String,
    /// Capa salva em disco (vazio = sem capa).
    pub cover_path: String,
}

/// Atualização de mídia publicada pelo produtor.
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaScalars {
    pub has_session: bool,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Cor dominante da capa, RGBA8 empacotado `0xRRGGBBAA` (0 = desconhecida).
    pub dominant_rgba: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TelemetryValues {
    pub cpu_percent: f32,
    pub ram_percent: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    /// Utilização do motor 3D da GPU (0..=100). `None` se o contador não existir.
    pub gpu_percent: Option<f32>,
    /// VRAM dedicada em uso / orçamento (MiB).
    pub gpu_mem_used_mb: Option<f32>,
    pub gpu_mem_budget_mb: Option<f32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PowerValues {
    /// `None` = sem bateria (desktop).
    pub battery_percent: Option<f32>,
    pub on_battery: bool,
    pub battery_saver: bool,
}

// ───────────────────────── SignalBus ─────────────────────────

type Waker = Box<dyn Fn() + Send + Sync>;
type DemandListener = Box<dyn Fn(SignalKind) + Send + Sync>;

pub struct SignalBus {
    epoch: Instant,
    demand: AtomicU32,
    wake_mask: AtomicU32,
    dirty: AtomicU32,
    waker: OnceLock<Waker>,
    demand_listener: OnceLock<DemandListener>,

    // Áudio (seqlock, escritor único).
    audio_seq: AtomicU64,
    bands: [AtomicF32; BAND_COUNT],
    bass: AtomicF32,
    mid: AtomicF32,
    treble: AtomicF32,
    level: AtomicF32,
    bpm: AtomicF32,
    beat_count: AtomicU32,
    beat_strength: AtomicF32,
    audio_live: AtomicBool,

    // Entrada (acumuladores + estado).
    cursor_x: AtomicI32,
    cursor_y: AtomicI32,
    mouse_dx: AtomicI32,
    mouse_dy: AtomicI32,
    wheel: AtomicI32,
    buttons: AtomicU32,
    clicks: AtomicU32,
    key_presses: AtomicU32,
    last_input_ms: AtomicU64,
    /// Último toque no desktop: `(x, y)` empacotados num só atômico (sem rasgo) + sequência.
    tap_pos: AtomicU64,
    tap_seq: AtomicU32,

    // Mídia.
    media_version: AtomicU64,
    cover_version: AtomicU64,
    media_has_session: AtomicBool,
    media_playing: AtomicBool,
    media_position_ms: AtomicU64,
    media_position_stamp_ms: AtomicU64,
    media_duration_ms: AtomicU64,
    media_color: AtomicU32,
    media_text: Mutex<MediaText>,

    // Telemetria / energia.
    telemetry_version: AtomicU64,
    cpu: AtomicF32,
    ram: AtomicF32,
    ram_used_gb: AtomicF32,
    ram_total_gb: AtomicF32,
    /// NaN = indisponível.
    gpu: AtomicF32,
    gpu_mem_used: AtomicF32,
    gpu_mem_budget: AtomicF32,
    power_version: AtomicU64,
    battery: AtomicF32,
    on_battery: AtomicBool,
    battery_saver: AtomicBool,
}

impl Default for SignalBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalBus {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            demand: AtomicU32::new(0),
            wake_mask: AtomicU32::new(0),
            dirty: AtomicU32::new(0),
            waker: OnceLock::new(),
            demand_listener: OnceLock::new(),
            audio_seq: AtomicU64::new(0),
            bands: std::array::from_fn(|_| AtomicF32::zero()),
            bass: AtomicF32::zero(),
            mid: AtomicF32::zero(),
            treble: AtomicF32::zero(),
            level: AtomicF32::zero(),
            bpm: AtomicF32::zero(),
            beat_count: AtomicU32::new(0),
            beat_strength: AtomicF32::zero(),
            audio_live: AtomicBool::new(false),
            cursor_x: AtomicI32::new(0),
            cursor_y: AtomicI32::new(0),
            mouse_dx: AtomicI32::new(0),
            mouse_dy: AtomicI32::new(0),
            wheel: AtomicI32::new(0),
            buttons: AtomicU32::new(0),
            clicks: AtomicU32::new(0),
            key_presses: AtomicU32::new(0),
            last_input_ms: AtomicU64::new(0),
            tap_pos: AtomicU64::new(0),
            tap_seq: AtomicU32::new(0),
            media_version: AtomicU64::new(0),
            cover_version: AtomicU64::new(0),
            media_has_session: AtomicBool::new(false),
            media_playing: AtomicBool::new(false),
            media_position_ms: AtomicU64::new(0),
            media_position_stamp_ms: AtomicU64::new(0),
            media_duration_ms: AtomicU64::new(0),
            media_color: AtomicU32::new(0),
            media_text: Mutex::new(MediaText::default()),
            telemetry_version: AtomicU64::new(0),
            cpu: AtomicF32::zero(),
            ram: AtomicF32::zero(),
            ram_used_gb: AtomicF32::zero(),
            ram_total_gb: AtomicF32::zero(),
            gpu: AtomicF32::new(f32::NAN),
            gpu_mem_used: AtomicF32::new(f32::NAN),
            gpu_mem_budget: AtomicF32::new(f32::NAN),
            power_version: AtomicU64::new(0),
            battery: AtomicF32::new(f32::NAN),
            on_battery: AtomicBool::new(false),
            battery_saver: AtomicBool::new(false),
        }
    }

    /// Milissegundos desde a criação do bus (relógio monotônico comum a produtores/consumidor).
    #[inline]
    pub fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    // ───────── configuração (plataforma / renderer) ─────────

    /// Plataforma: como acordar o loop do renderer. Só a primeira chamada vale.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) -> bool {
        self.waker.set(Box::new(waker)).is_ok()
    }

    /// Plataforma: avisada quando a demanda muda (para acordar/dormir workers).
    pub fn set_demand_listener(&self, listener: impl Fn(SignalKind) + Send + Sync + 'static) -> bool {
        self.demand_listener.set(Box::new(listener)).is_ok()
    }

    /// Renderer: sinais que a cena atual consome. Produtores fora da demanda dormem.
    pub fn set_demand(&self, demand: SignalKind) {
        let prev = self.demand.swap(demand.0, Ordering::AcqRel);
        if prev != demand.0 {
            if let Some(listener) = self.demand_listener.get() {
                listener(demand);
            }
        }
    }

    pub fn demand(&self) -> SignalKind {
        SignalKind(self.demand.load(Ordering::Acquire))
    }

    #[inline]
    pub fn is_demanded(&self, kind: SignalKind) -> bool {
        self.demand().intersects(kind)
    }

    /// Renderer: quais sinais devem acordar o loop quando ele está em `Wait`.
    pub fn set_wake_mask(&self, mask: SignalKind) {
        self.wake_mask.store(mask.0, Ordering::Release);
    }

    /// Renderer: bits alterados desde a última chamada (zera).
    pub fn take_dirty(&self) -> SignalKind {
        SignalKind(self.dirty.swap(0, Ordering::AcqRel))
    }

    /// Produtor: marca `kind` como alterado e acorda o loop se for a 1ª marca desde o último
    /// `take_dirty` (coalescência: no máximo 1 wake por frame consumido).
    #[inline]
    pub fn mark(&self, kind: SignalKind) {
        let prev = self.dirty.fetch_or(kind.0, Ordering::AcqRel);
        let fresh = kind.0 & !prev;
        if fresh & self.wake_mask.load(Ordering::Relaxed) != 0 {
            if let Some(wake) = self.waker.get() {
                wake();
            }
        }
    }

    // ───────── produtores: áudio ─────────

    /// Publica um frame de análise (escritor único).
    pub fn publish_audio(&self, frame: &AudioFrame) {
        let s = self.audio_seq.load(Ordering::Relaxed);
        self.audio_seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (dst, &v) in self.bands.iter().zip(frame.bands.iter()) {
            dst.store(v, Ordering::Relaxed);
        }
        self.bass.store(frame.bass, Ordering::Relaxed);
        self.mid.store(frame.mid, Ordering::Relaxed);
        self.treble.store(frame.treble, Ordering::Relaxed);
        self.level.store(frame.level, Ordering::Relaxed);
        self.bpm.store(frame.bpm, Ordering::Relaxed);
        if let Some(strength) = frame.beat {
            self.beat_strength.store(strength, Ordering::Relaxed);
            self.beat_count.fetch_add(1, Ordering::Relaxed);
        }
        self.audio_seq.store(s.wrapping_add(2), Ordering::Release);
        self.audio_live.store(!frame.silent, Ordering::Relaxed);
        self.mark(SignalKind::AUDIO);
    }

    /// Zera o áudio (dispositivo perdido / demanda removida). Publica só se havia algo.
    pub fn clear_audio(&self) {
        if !self.audio_live.swap(false, Ordering::AcqRel) && self.level.load(Ordering::Relaxed) == 0.0 {
            return;
        }
        self.publish_audio(&AudioFrame::default());
    }

    // ───────── produtores: entrada ─────────

    #[inline]
    pub fn add_mouse_delta(&self, dx: i32, dy: i32) {
        if dx != 0 {
            self.mouse_dx.fetch_add(dx, Ordering::Relaxed);
        }
        if dy != 0 {
            self.mouse_dy.fetch_add(dy, Ordering::Relaxed);
        }
    }

    /// Posição global do cursor (pixels físicos, desktop virtual).
    #[inline]
    pub fn set_cursor(&self, x: i32, y: i32) {
        self.cursor_x.store(x, Ordering::Relaxed);
        self.cursor_y.store(y, Ordering::Relaxed);
    }

    #[inline]
    pub fn add_wheel(&self, delta: i32) {
        self.wheel.fetch_add(delta, Ordering::Relaxed);
    }

    /// Atualiza botões: `down`/`up` são máscaras de `buttons::*`. Cliques contam bordas de descida.
    #[inline]
    pub fn update_buttons(&self, down: u32, up: u32) {
        if down != 0 {
            let prev = self.buttons.fetch_or(down, Ordering::Relaxed);
            let edges = (down & !prev).count_ones();
            if edges > 0 {
                self.clicks.fetch_add(edges, Ordering::Relaxed);
            }
        }
        if up != 0 {
            self.buttons.fetch_and(!up, Ordering::Relaxed);
        }
    }

    /// Tecla pressionada (anônimo: nenhum código de tecla trafega pelo bus).
    #[inline]
    pub fn add_key_press(&self) {
        self.key_presses.fetch_add(1, Ordering::Relaxed);
    }

    /// Fecha um lote de entrada: carimbo + dirty/wake.
    #[inline]
    pub fn commit_input(&self, kind: SignalKind) {
        self.last_input_ms.store(self.now_ms(), Ordering::Relaxed);
        self.mark(kind);
    }

    /// Plataforma: clique curto no fundo do desktop em `(x, y)` (pixels físicos do desktop
    /// virtual). Só o último toque entre dois quadros é mantido.
    pub fn publish_desktop_tap(&self, x: i32, y: i32) {
        let packed = (u64::from(x as u32) << 32) | u64::from(y as u32);
        self.tap_pos.store(packed, Ordering::Relaxed);
        self.tap_seq.fetch_add(1, Ordering::Release);
        self.mark(SignalKind::DESKTOP_TAP);
    }

    // ───────── produtores: mídia ─────────

    /// Publica escalares de mídia. `text`: `Some` só quando metadados/capa mudaram.
    pub fn publish_media(&self, scalars: MediaScalars, text: Option<&MediaText>, cover_changed: bool) {
        if let Some(text) = text {
            match self.media_text.lock() {
                Ok(mut guard) => guard.clone_from(text),
                Err(poisoned) => poisoned.into_inner().clone_from(text),
            }
        }
        self.media_has_session.store(scalars.has_session, Ordering::Relaxed);
        self.media_playing.store(scalars.playing, Ordering::Relaxed);
        self.media_position_ms.store(scalars.position_ms, Ordering::Relaxed);
        self.media_position_stamp_ms.store(self.now_ms(), Ordering::Relaxed);
        self.media_duration_ms.store(scalars.duration_ms, Ordering::Relaxed);
        self.media_color.store(scalars.dominant_rgba, Ordering::Relaxed);
        if cover_changed {
            self.cover_version.fetch_add(1, Ordering::Relaxed);
        }
        self.media_version.fetch_add(1, Ordering::Release);
        self.mark(SignalKind::MEDIA);
    }

    pub fn media_version(&self) -> u64 {
        self.media_version.load(Ordering::Acquire)
    }

    /// Copia o texto de mídia em `out` reaproveitando a capacidade das `String`s.
    /// Retorna `false` se o lock estava ocupado (tentar no próximo frame) — nunca bloqueia.
    #[allow(dead_code)] // API reservada (ainda sem chamador)
    pub fn read_media_text(&self, out: &mut MediaText) -> bool {
        match self.media_text.try_lock() {
            Ok(guard) => {
                out.clone_from(&guard);
                true
            }
            Err(std::sync::TryLockError::Poisoned(p)) => {
                out.clone_from(&p.into_inner());
                true
            }
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
    }

    /// Cópia bloqueante do texto (fora do frame: IPC/Estúdio/diagnóstico).
    pub fn media_text(&self) -> MediaText {
        match self.media_text.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    // ───────── produtores: telemetria / energia ─────────

    pub fn publish_telemetry(&self, t: &TelemetryValues) {
        self.cpu.store(t.cpu_percent, Ordering::Relaxed);
        self.ram.store(t.ram_percent, Ordering::Relaxed);
        self.ram_used_gb.store(t.ram_used_gb, Ordering::Relaxed);
        self.ram_total_gb.store(t.ram_total_gb, Ordering::Relaxed);
        self.gpu.store(t.gpu_percent.unwrap_or(f32::NAN), Ordering::Relaxed);
        self.gpu_mem_used.store(t.gpu_mem_used_mb.unwrap_or(f32::NAN), Ordering::Relaxed);
        self.gpu_mem_budget.store(t.gpu_mem_budget_mb.unwrap_or(f32::NAN), Ordering::Relaxed);
        self.telemetry_version.fetch_add(1, Ordering::Release);
        self.mark(SignalKind::TELEMETRY);
    }

    /// Publica só se mudou (bateria muda devagar; evita wakes inúteis).
    pub fn publish_power(&self, p: &PowerValues) {
        let prev = self.power();
        if prev == *p && self.power_version.load(Ordering::Relaxed) != 0 {
            return;
        }
        self.battery.store(p.battery_percent.unwrap_or(f32::NAN), Ordering::Relaxed);
        self.on_battery.store(p.on_battery, Ordering::Relaxed);
        self.battery_saver.store(p.battery_saver, Ordering::Relaxed);
        self.power_version.fetch_add(1, Ordering::Release);
        self.mark(SignalKind::POWER);
    }

    // ───────── leitura direta (fora do frame) ─────────

    pub fn telemetry(&self) -> TelemetryValues {
        let opt = |v: f32| if v.is_nan() { None } else { Some(v) };
        TelemetryValues {
            cpu_percent: self.cpu.load(Ordering::Relaxed),
            ram_percent: self.ram.load(Ordering::Relaxed),
            ram_used_gb: self.ram_used_gb.load(Ordering::Relaxed),
            ram_total_gb: self.ram_total_gb.load(Ordering::Relaxed),
            gpu_percent: opt(self.gpu.load(Ordering::Relaxed)),
            gpu_mem_used_mb: opt(self.gpu_mem_used.load(Ordering::Relaxed)),
            gpu_mem_budget_mb: opt(self.gpu_mem_budget.load(Ordering::Relaxed)),
        }
    }

    pub fn power(&self) -> PowerValues {
        let b = self.battery.load(Ordering::Relaxed);
        PowerValues {
            battery_percent: if b.is_nan() { None } else { Some(b) },
            on_battery: self.on_battery.load(Ordering::Relaxed),
            battery_saver: self.battery_saver.load(Ordering::Relaxed),
        }
    }

    pub fn media_scalars(&self) -> MediaScalars {
        MediaScalars {
            has_session: self.media_has_session.load(Ordering::Relaxed),
            playing: self.media_playing.load(Ordering::Relaxed),
            position_ms: self.media_position_ms.load(Ordering::Relaxed),
            duration_ms: self.media_duration_ms.load(Ordering::Relaxed),
            dominant_rgba: self.media_color.load(Ordering::Relaxed),
        }
    }

    pub fn cursor(&self) -> (i32, i32) {
        (self.cursor_x.load(Ordering::Relaxed), self.cursor_y.load(Ordering::Relaxed))
    }

    /// Copia as bandas (sem alocar). Retorna quantas foram escritas.
    pub fn copy_bands(&self, out: &mut [f32]) -> usize {
        let n = out.len().min(BAND_COUNT);
        let mut tries = 0;
        loop {
            let s1 = self.audio_seq.load(Ordering::Acquire);
            if s1 & 1 == 0 {
                for (dst, src) in out[..n].iter_mut().zip(self.bands.iter()) {
                    *dst = src.load(Ordering::Relaxed);
                }
                fence(Ordering::Acquire);
                if self.audio_seq.load(Ordering::Relaxed) == s1 {
                    return n;
                }
            }
            tries += 1;
            if tries >= SEQ_RETRIES {
                return n; // aceita um frame misto em vez de girar no render
            }
            std::hint::spin_loop();
        }
    }

    /// Lê o bloco de áudio de forma consistente (seqlock).
    fn read_audio(&self, snap: &mut SignalSnapshot) -> u32 {
        let mut tries = 0;
        loop {
            let s1 = self.audio_seq.load(Ordering::Acquire);
            if s1 & 1 == 0 {
                for (dst, src) in snap.bands.iter_mut().zip(self.bands.iter()) {
                    *dst = src.load(Ordering::Relaxed);
                }
                snap.bass = self.bass.load(Ordering::Relaxed);
                snap.mid = self.mid.load(Ordering::Relaxed);
                snap.treble = self.treble.load(Ordering::Relaxed);
                snap.level = self.level.load(Ordering::Relaxed);
                snap.bpm = self.bpm.load(Ordering::Relaxed);
                snap.beat_strength = self.beat_strength.load(Ordering::Relaxed);
                let count = self.beat_count.load(Ordering::Relaxed);
                fence(Ordering::Acquire);
                if self.audio_seq.load(Ordering::Relaxed) == s1 || tries + 1 >= SEQ_RETRIES {
                    return count;
                }
            } else if tries + 1 >= SEQ_RETRIES {
                return self.beat_count.load(Ordering::Relaxed);
            }
            tries += 1;
            std::hint::spin_loop();
        }
    }
}

// ───────────────────────── Consumidor ─────────────────────────

/// Fonte escalar para bindings (resolvida 1× no load da cena; lida por frame sem `match` de string).
/// Nomes compatíveis com `Scene3DBindingSource` do Estúdio + extensões nativas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalSource {
    AudioBass,
    AudioMid,
    AudioTreble,
    AudioLevel,
    AudioBeat,
    /// Banda 0..64 (graves → agudos).
    AudioBand(u8),
    KeyPulse,
    TypingEnergy,
    MouseEnergy,
    MouseButtons,
    MediaPlaying,
    MediaProgress,
    CpuUsage,
    RamUsage,
    GpuUsage,
    Battery,
    /// Cursor sobre o monitor em que está, -1 (esquerda/topo) ..= 1 (direita/base); parallax.
    CursorX,
    CursorY,
}

impl SignalSource {
    /// Converte o identificador do contrato `.prism` (ex.: `AUDIO_BASS`, `AUDIO_BAND_12`).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "AUDIO_BASS" => Self::AudioBass,
            "AUDIO_MID" => Self::AudioMid,
            "AUDIO_TREBLE" => Self::AudioTreble,
            "AUDIO_LEVEL" => Self::AudioLevel,
            "AUDIO_BEAT" => Self::AudioBeat,
            "KEY_PULSE" => Self::KeyPulse,
            "TYPING_ENERGY" => Self::TypingEnergy,
            "MOUSE_ENERGY" => Self::MouseEnergy,
            "MOUSE_BUTTONS" => Self::MouseButtons,
            "MEDIA_PLAYING" => Self::MediaPlaying,
            "MEDIA_PROGRESS" => Self::MediaProgress,
            "CPU_USAGE" => Self::CpuUsage,
            "RAM_USAGE" => Self::RamUsage,
            "GPU_USAGE" => Self::GpuUsage,
            "BATTERY" => Self::Battery,
            "CURSOR_X" => Self::CursorX,
            "CURSOR_Y" => Self::CursorY,
            other => {
                let n: u8 = other.strip_prefix("AUDIO_BAND_")?.parse().ok()?;
                if (n as usize) < BAND_COUNT {
                    Self::AudioBand(n)
                } else {
                    return None;
                }
            }
        })
    }

    /// Que produtor precisa estar ativo para esta fonte.
    pub fn kind(self) -> SignalKind {
        match self {
            Self::AudioBass
            | Self::AudioMid
            | Self::AudioTreble
            | Self::AudioLevel
            | Self::AudioBeat
            | Self::AudioBand(_) => SignalKind::AUDIO,
            Self::KeyPulse | Self::TypingEnergy => SignalKind::KEYBOARD,
            Self::MouseEnergy | Self::MouseButtons => SignalKind::MOUSE,
            Self::MediaPlaying | Self::MediaProgress => SignalKind::MEDIA,
            Self::CpuUsage | Self::RamUsage | Self::GpuUsage => SignalKind::TELEMETRY,
            Self::Battery => SignalKind::POWER,
            Self::CursorX | Self::CursorY => SignalKind::MOUSE,
        }
    }
}

/// Estado de sinais de um frame. Pré-alocado pelo renderer (`Copy`, ~400 B), preenchido por
/// `SignalReader::update`.
#[derive(Debug, Clone, Copy)]
pub struct SignalSnapshot {
    /// Bits alterados desde o frame anterior.
    pub dirty: SignalKind,
    pub time_s: f64,

    pub bands: [f32; BAND_COUNT],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub level: f32,
    /// Pulso de batida 0..=1 (salta na batida, decai ~exp(-6t)).
    pub beat: f32,
    pub beat_strength: f32,
    /// Batidas desde o início (dispara efeitos discretos).
    pub beat_count: u32,
    /// Houve batida neste frame.
    pub beat_now: bool,
    pub bpm: f32,

    /// Cursor global (pixels físicos do desktop virtual).
    pub cursor: (i32, i32),
    /// Cursor normalizado (-1..=1) sobre o monitor que o contém (preenchido pelo loop nativo,
    /// que conhece as saídas). Centro = 0.
    pub cursor_norm: [f32; 2],
    /// Movimento bruto acumulado neste frame (mickeys).
    pub mouse_delta: (i32, i32),
    /// Velocidade suavizada 0..=1.
    pub mouse_energy: f32,
    pub wheel: i32,
    pub buttons: u32,
    /// Cliques (bordas de descida) neste frame.
    pub clicks: u32,
    pub key_pulse: f32,
    pub typing_energy: f32,
    /// Segundos desde a última entrada global.
    pub idle_s: f32,
    /// Toque no fundo do desktop neste quadro (pixels físicos do desktop virtual).
    pub desktop_tap: Option<(i32, i32)>,

    pub media_has_session: bool,
    pub media_playing: bool,
    /// Posição extrapolada (s) se tocando.
    pub media_position_s: f32,
    pub media_duration_s: f32,
    pub media_progress: f32,
    /// Cor dominante da capa (sRGB, 0..=1).
    pub media_color: [f32; 4],
    pub media_version: u64,
    pub cover_version: u64,

    pub cpu_percent: f32,
    pub ram_percent: f32,
    /// NaN = indisponível.
    pub gpu_percent: f32,
    /// NaN = sem bateria.
    pub battery_percent: f32,
    pub on_battery: bool,
    pub battery_saver: bool,
}

impl Default for SignalSnapshot {
    fn default() -> Self {
        Self {
            dirty: SignalKind::NONE,
            time_s: 0.0,
            bands: [0.0; BAND_COUNT],
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            level: 0.0,
            beat: 0.0,
            beat_strength: 0.0,
            beat_count: 0,
            beat_now: false,
            bpm: 0.0,
            cursor: (0, 0),
            cursor_norm: [0.0, 0.0],
            mouse_delta: (0, 0),
            mouse_energy: 0.0,
            wheel: 0,
            buttons: 0,
            clicks: 0,
            key_pulse: 0.0,
            typing_energy: 0.0,
            idle_s: 0.0,
            desktop_tap: None,
            media_has_session: false,
            media_playing: false,
            media_position_s: 0.0,
            media_duration_s: 0.0,
            media_progress: 0.0,
            media_color: [0.0; 4],
            media_version: 0,
            cover_version: 0,
            cpu_percent: 0.0,
            ram_percent: 0.0,
            gpu_percent: f32::NAN,
            battery_percent: f32::NAN,
            on_battery: false,
            battery_saver: false,
        }
    }
}

impl SignalSnapshot {
    /// Valor escalar 0..=1 de uma fonte (porcentagens normalizadas). Sem alocação.
    pub fn get(&self, source: SignalSource) -> f32 {
        let pct = |v: f32| if v.is_nan() { 0.0 } else { (v / 100.0).clamp(0.0, 1.0) };
        match source {
            SignalSource::AudioBass => self.bass,
            SignalSource::AudioMid => self.mid,
            SignalSource::AudioTreble => self.treble,
            SignalSource::AudioLevel => self.level,
            SignalSource::AudioBeat => self.beat,
            SignalSource::AudioBand(i) => self.bands.get(i as usize).copied().unwrap_or(0.0),
            SignalSource::KeyPulse => self.key_pulse,
            SignalSource::TypingEnergy => self.typing_energy,
            SignalSource::MouseEnergy => self.mouse_energy,
            SignalSource::MouseButtons => {
                if self.buttons != 0 {
                    1.0
                } else {
                    0.0
                }
            }
            SignalSource::MediaPlaying => {
                if self.media_playing {
                    1.0
                } else {
                    0.0
                }
            }
            SignalSource::MediaProgress => self.media_progress,
            SignalSource::CpuUsage => pct(self.cpu_percent),
            SignalSource::RamUsage => pct(self.ram_percent),
            SignalSource::GpuUsage => pct(self.gpu_percent),
            SignalSource::Battery => pct(self.battery_percent),
            SignalSource::CursorX => self.cursor_norm[0].clamp(-1.0, 1.0),
            SignalSource::CursorY => self.cursor_norm[1].clamp(-1.0, 1.0),
        }
    }

    /// Algum sinal contínuo ainda está animando (decaimentos em curso) → o loop deve continuar
    /// renderizando mesmo sem `dirty` novo.
    pub fn animating(&self) -> bool {
        self.beat > 0.0
            || self.key_pulse > 0.0
            || self.typing_energy > 0.0
            || self.mouse_energy > 0.0
            || self.level > 0.0
            || (self.media_playing && self.media_duration_s > 0.0)
    }
}

/// Estado de suavização do consumidor (decaimentos dependentes de dt). 1 por renderer.
#[derive(Debug, Clone)]
pub struct SignalReader {
    last_update: Option<Instant>,
    last_beat_count: Option<u32>,
    last_tap_seq: Option<u32>,
    key_pulse: f32,
    typing_energy: f32,
    beat: f32,
    mouse_energy: f32,
}

impl Default for SignalReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Velocidade (mickeys/s) mapeada para energia 1.0.
const MOUSE_FULL_SPEED: f32 = 2_500.0;

impl SignalReader {
    pub fn new() -> Self {
        Self {
            last_update: None,
            last_beat_count: None,
            last_tap_seq: None,
            key_pulse: 0.0,
            typing_energy: 0.0,
            beat: 0.0,
            mouse_energy: 0.0,
        }
    }

    /// Lê o bus para `snap` (1× por frame, antes de avaliar bindings). Sem alocação nem bloqueio.
    pub fn update(&mut self, bus: &SignalBus, now: Instant, snap: &mut SignalSnapshot) {
        let dt = match self.last_update {
            Some(prev) => now.saturating_duration_since(prev).as_secs_f32().clamp(0.0005, 0.25),
            None => 1.0 / 60.0,
        };
        self.last_update = Some(now);
        let now_ms = bus.now_ms();
        snap.time_s = now_ms as f64 / 1000.0;
        snap.dirty = bus.take_dirty();
        let demand = bus.demand();

        // Áudio.
        let count = bus.read_audio(snap);
        snap.beat_now = matches!(self.last_beat_count, Some(prev) if prev != count);
        self.last_beat_count = Some(count);
        self.beat *= (-6.0 * dt).exp();
        if snap.beat_now {
            self.beat = self.beat.max(snap.beat_strength.max(0.6));
        }
        if self.beat < 0.002 {
            self.beat = 0.0;
        }
        snap.beat = self.beat;
        snap.beat_count = count;

        // Entrada (acumuladores zerados aqui).
        snap.cursor = bus.cursor();
        let dx = bus.mouse_dx.swap(0, Ordering::Relaxed);
        let dy = bus.mouse_dy.swap(0, Ordering::Relaxed);
        snap.mouse_delta = (dx, dy);
        snap.wheel = bus.wheel.swap(0, Ordering::Relaxed);
        snap.buttons = bus.buttons.load(Ordering::Relaxed);
        snap.clicks = bus.clicks.swap(0, Ordering::Relaxed);
        let presses = bus.key_presses.swap(0, Ordering::Relaxed);

        let speed = ((dx as f32).hypot(dy as f32) / dt / MOUSE_FULL_SPEED).min(1.0);
        let k = if speed > self.mouse_energy { 1.0 - (-dt / 0.03).exp() } else { 1.0 - (-dt / 0.25).exp() };
        self.mouse_energy += (speed - self.mouse_energy) * k;
        if self.mouse_energy < 0.002 {
            self.mouse_energy = 0.0;
        }
        snap.mouse_energy = self.mouse_energy;

        // Ritmo de digitação (port 1:1 do Estúdio: pulso nítido + energia de cadência).
        self.key_pulse *= (-8.5 * dt).exp();
        self.typing_energy *= (-1.8 * dt).exp();
        if presses > 0 && demand.contains(SignalKind::KEYBOARD) {
            let p = presses as f32;
            self.key_pulse = (self.key_pulse + 0.72 * p.min(3.0)).min(1.0);
            self.typing_energy = (self.typing_energy + 0.22 * p.min(4.0)).min(1.0);
        }
        if self.key_pulse < 0.002 {
            self.key_pulse = 0.0;
        }
        if self.typing_energy < 0.002 {
            self.typing_energy = 0.0;
        }
        snap.key_pulse = self.key_pulse;
        snap.typing_energy = self.typing_energy;
        let last_input = bus.last_input_ms.load(Ordering::Relaxed);
        snap.idle_s = now_ms.saturating_sub(last_input) as f32 / 1000.0;

        // Toque no desktop (1º update só sincroniza a sequência: toque antigo não dispara).
        let tap_seq = bus.tap_seq.load(Ordering::Acquire);
        snap.desktop_tap = match self.last_tap_seq {
            Some(prev) if prev != tap_seq => {
                let packed = bus.tap_pos.load(Ordering::Relaxed);
                Some(((packed >> 32) as u32 as i32, packed as u32 as i32))
            }
            _ => None,
        };
        self.last_tap_seq = Some(tap_seq);

        // Mídia (posição extrapolada entre eventos do SMTC).
        let m = bus.media_scalars();
        snap.media_has_session = m.has_session;
        snap.media_playing = m.playing;
        let mut pos_ms = m.position_ms;
        if m.playing {
            let stamp = bus.media_position_stamp_ms.load(Ordering::Relaxed);
            pos_ms = pos_ms.saturating_add(now_ms.saturating_sub(stamp));
        }
        if m.duration_ms > 0 {
            pos_ms = pos_ms.min(m.duration_ms);
        }
        snap.media_position_s = pos_ms as f32 / 1000.0;
        snap.media_duration_s = m.duration_ms as f32 / 1000.0;
        snap.media_progress = if m.duration_ms > 0 {
            (pos_ms as f32 / m.duration_ms as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        snap.media_color = unpack_rgba(m.dominant_rgba);
        snap.media_version = bus.media_version();
        snap.cover_version = bus.cover_version.load(Ordering::Relaxed);

        // Telemetria / energia.
        snap.cpu_percent = bus.cpu.load(Ordering::Relaxed);
        snap.ram_percent = bus.ram.load(Ordering::Relaxed);
        snap.gpu_percent = bus.gpu.load(Ordering::Relaxed);
        let p = bus.power();
        snap.battery_percent = p.battery_percent.unwrap_or(f32::NAN);
        snap.on_battery = p.on_battery;
        snap.battery_saver = p.battery_saver;
    }
}

#[inline]
pub fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | a as u32
}

#[inline]
pub fn unpack_rgba(v: u32) -> [f32; 4] {
    [
        ((v >> 24) & 0xFF) as f32 / 255.0,
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
    ]
}

/// Cor dominante de uma imagem RGBA8 (já reduzida, ex. 32×32). Histograma de matiz ponderado por
/// saturação e luminância média (ignora pretos/brancos/cinzas); cai na média se a capa for
/// acromática. Independente de SO; roda na thread do produtor de mídia.
pub fn dominant_color(rgba: &[u8]) -> u32 {
    const BINS: usize = 24;
    let mut weight = [0.0f32; BINS];
    let mut acc = [[0.0f32; 3]; BINS];
    let mut avg = [0.0f32; 3];
    let mut n = 0.0f32;
    for px in rgba.chunks_exact(4) {
        if px[3] < 128 {
            continue;
        }
        let (r, g, b) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        avg[0] += r;
        avg[1] += g;
        avg[2] += b;
        n += 1.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let chroma = max - min;
        let light = (max + min) * 0.5;
        if chroma < 0.08 || !(0.12..=0.92).contains(&light) {
            continue;
        }
        let sector = if max == r {
            ((g - b) / chroma).rem_euclid(6.0)
        } else if max == g {
            (b - r) / chroma + 2.0
        } else {
            (r - g) / chroma + 4.0
        };
        let hue = sector / 6.0;
        let bin = ((hue * BINS as f32) as usize).min(BINS - 1);
        // Prefere cores vivas e de luminância média.
        let w = chroma * (1.0 - (light - 0.5).abs() * 1.4).max(0.1);
        weight[bin] += w;
        acc[bin][0] += r * w;
        acc[bin][1] += g * w;
        acc[bin][2] += b * w;
    }
    if n == 0.0 {
        return 0;
    }
    // Pico com vizinhos (suaviza bordas de bin).
    let mut best = 0usize;
    let mut best_w = 0.0f32;
    for i in 0..BINS {
        let w = weight[i] + 0.5 * (weight[(i + BINS - 1) % BINS] + weight[(i + 1) % BINS]);
        if w > best_w {
            best_w = w;
            best = i;
        }
    }
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    if best_w > n * 0.02 && weight[best] > 0.0 {
        let w = weight[best];
        pack_rgba(to_u8(acc[best][0] / w), to_u8(acc[best][1] / w), to_u8(acc[best][2] / w), 255)
    } else {
        pack_rgba(to_u8(avg[0] / n), to_u8(avg[1] / n), to_u8(avg[2] / n), 255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_and_accumulators() {
        let bus = SignalBus::new();
        bus.set_demand(SignalKind::ALL);
        bus.add_mouse_delta(10, -4);
        bus.update_buttons(buttons::LEFT, 0);
        bus.add_key_press();
        bus.commit_input(SignalKind::INPUT);
        let mut reader = SignalReader::new();
        let mut snap = SignalSnapshot::default();
        reader.update(&bus, Instant::now(), &mut snap);
        assert!(snap.dirty.contains(SignalKind::MOUSE));
        assert_eq!(snap.mouse_delta, (10, -4));
        assert_eq!(snap.clicks, 1);
        assert!(snap.key_pulse > 0.5);
        reader.update(&bus, Instant::now(), &mut snap);
        assert_eq!(snap.mouse_delta, (0, 0));
        assert!(snap.dirty.is_empty());
    }

    #[test]
    fn desktop_tap_roundtrip() {
        let bus = SignalBus::new();
        let mut reader = SignalReader::new();
        let mut snap = SignalSnapshot::default();
        reader.update(&bus, Instant::now(), &mut snap);
        assert_eq!(snap.desktop_tap, None);
        bus.publish_desktop_tap(-1920, 1079);
        reader.update(&bus, Instant::now(), &mut snap);
        assert_eq!(snap.desktop_tap, Some((-1920, 1079)));
        assert!(snap.dirty.contains(SignalKind::DESKTOP_TAP));
        reader.update(&bus, Instant::now(), &mut snap);
        assert_eq!(snap.desktop_tap, None);
    }

    #[test]
    fn parse_sources() {
        assert_eq!(SignalSource::parse("AUDIO_BAND_12"), Some(SignalSource::AudioBand(12)));
        assert_eq!(SignalSource::parse("AUDIO_BAND_64"), None);
        assert_eq!(SignalSource::parse("CPU_USAGE"), Some(SignalSource::CpuUsage));
    }

    #[test]
    fn dominant_red() {
        let mut img = vec![0u8; 16 * 4];
        for px in img.chunks_exact_mut(4) {
            px.copy_from_slice(&[220, 30, 40, 255]);
        }
        let c = unpack_rgba(dominant_color(&img));
        assert!(c[0] > 0.8 && c[1] < 0.2);
    }
}
