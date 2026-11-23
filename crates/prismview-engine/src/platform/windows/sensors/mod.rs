//! `Win32SystemSensors` (Fase 4): produtores Windows do `core::signals::SignalBus`.
//!
//! Uma thread por fonte, todas dormindo sem demanda:
//! - `audio`: WASAPI loopback (evento) → FFT 2048 / 64 bandas / batida (`core::audio`).
//! - `input`: Raw Input (mouse/teclado global) numa janela message-only.
//! - `media`: SMTC/GSMTC por eventos WinRT, capa em disco + cor dominante.
//! - `telemetry`: CPU/RAM/GPU(PDH + DXGI)/bateria.
//!
//! Uso:
//! ```ignore
//! let bus = Arc::new(SignalBus::new());
//! let mut sensors = Win32SystemSensors::new(Arc::clone(&bus), SensorsConfig::default(), Some(host.proxy()))?;
//! sensors.set_demand(SignalKind::AUDIO | SignalKind::MEDIA); // o que a cena usa
//! bus.set_wake_mask(SignalKind::MEDIA);                       // o que acorda cena estática
//! // por frame: reader.update(&bus, Instant::now(), &mut snapshot);
//! ```
//!
//! Um `Win32SystemSensors` por `SignalBus` (o listener de demanda do bus é instalado uma vez;
//! uma segunda instância no mesmo bus cai no fallback de `park_timeout`).

mod audio;
mod input;
mod media;
mod telemetry;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::thread::{self, Thread};
use std::time::Duration;

use ::windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use self::media::MediaChannel;
use super::event_loop::LoopProxy;
use crate::engine_core::audio::BAND_COUNT;
use crate::engine_core::signals::{SignalBus, SignalKind};
use crate::platform::{
    MediaCommand, MediaSession, PlatformError, PlatformResult, SystemSensors, SystemStats,
};

/// Sem listener de demanda instalado, os workers reavaliam a demanda nesse intervalo.
const FALLBACK_IDLE_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct SensorsConfig {
    /// Pasta das capas de álbum (padrão `%APPDATA%\PrismView\cache\media`).
    pub cover_dir: PathBuf,
    /// Intervalo da telemetria enquanto `TELEMETRY` estiver na demanda.
    pub telemetry_interval: Duration,
    /// Intervalo da amostragem de energia quando só `POWER` está na demanda.
    pub power_interval: Duration,
    /// LUID `(LowPart, HighPart)` do adapter do renderer para a VRAM (None = 1º adapter HW).
    pub gpu_adapter_luid: Option<(u32, i32)>,
}

impl Default for SensorsConfig {
    fn default() -> Self {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            cover_dir: base.join("PrismView").join("cache").join("media"),
            telemetry_interval: Duration::from_millis(1500),
            power_interval: Duration::from_secs(15),
            gpu_adapter_luid: None,
        }
    }
}

/// Estado comum aos workers.
pub(super) struct Shared {
    pub(super) bus: Arc<SignalBus>,
    pub(super) stop: AtomicBool,
    listener_installed: AtomicBool,
}

impl Shared {
    /// Dorme até a demanda mudar (unpark do listener) ou, sem listener, por um intervalo curto.
    pub(super) fn idle_wait(&self) {
        if self.listener_installed.load(Ordering::Acquire) {
            thread::park();
        } else {
            thread::park_timeout(FALLBACK_IDLE_POLL);
        }
    }
}

/// `CoInitializeEx(MTA)` pareado com `CoUninitialize` (só se a inicialização contou).
pub(super) struct ComMta(bool);

impl ComMta {
    pub(super) fn init() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComMta {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub struct Win32SystemSensors {
    bus: Arc<SignalBus>,
    shared: Arc<Shared>,
    media: Arc<MediaChannel>,
    input_hwnd: Arc<AtomicIsize>,
    parked: Vec<Thread>,
}

impl Win32SystemSensors {
    /// Sobe os workers (todos ociosos até `set_demand`). `waker`: acorda o loop do renderer
    /// quando um sinal em `bus.set_wake_mask` muda com a cena em `ControlFlow::Wait`.
    pub fn new(
        bus: Arc<SignalBus>,
        config: SensorsConfig,
        waker: Option<LoopProxy>,
    ) -> PlatformResult<Self> {
        if let Some(proxy) = waker {
            bus.set_waker(move || {
                proxy.wake();
            });
        }

        let shared = Arc::new(Shared {
            bus: Arc::clone(&bus),
            stop: AtomicBool::new(false),
            listener_installed: AtomicBool::new(false),
        });
        let media = Arc::new(MediaChannel::default());
        let input_hwnd = Arc::new(AtomicIsize::new(0));

        let spawn_err = |what: &str, e: std::io::Error| PlatformError::Os(format!("thread {what}: {e}"));

        let s = Arc::clone(&shared);
        let audio = thread::Builder::new()
            .name("prism-sensor-audio".into())
            .stack_size(256 * 1024)
            .spawn(move || audio::run(s))
            .map_err(|e| spawn_err("audio", e))?;

        let s = Arc::clone(&shared);
        let cfg = config.clone();
        let telemetry = thread::Builder::new()
            .name("prism-sensor-telemetry".into())
            .stack_size(256 * 1024)
            .spawn(move || telemetry::run(s, cfg))
            .map_err(|e| spawn_err("telemetry", e))?;

        let s = Arc::clone(&shared);
        let ch = Arc::clone(&media);
        let cover_dir = config.cover_dir.clone();
        thread::Builder::new()
            .name("prism-sensor-media".into())
            .stack_size(512 * 1024)
            .spawn(move || media::run(s, ch, cover_dir))
            .map_err(|e| spawn_err("media", e))?;

        let s = Arc::clone(&shared);
        let hw = Arc::clone(&input_hwnd);
        thread::Builder::new()
            .name("prism-sensor-input".into())
            .stack_size(256 * 1024)
            .spawn(move || input::run(s, hw))
            .map_err(|e| spawn_err("input", e))?;

        // Workers são destacados (sem join): o Drop sinaliza parada e não bloqueia o renderer.
        let parked = vec![audio.thread().clone(), telemetry.thread().clone()];

        let listener_threads = parked.clone();
        let listener_media = Arc::clone(&media);
        let listener_hwnd = Arc::clone(&input_hwnd);
        let installed = bus.set_demand_listener(move |_demand| {
            for t in &listener_threads {
                t.unpark();
            }
            listener_media.notify_demand();
            input::notify_demand(listener_hwnd.load(Ordering::Acquire));
        });
        shared.listener_installed.store(installed, Ordering::Release);
        // Acorda quem já tenha estacionado antes do listener existir.
        for t in &parked {
            t.unpark();
        }

        Ok(Self {
            bus,
            shared,
            media,
            input_hwnd,
            parked,
        })
    }

    pub fn bus(&self) -> &Arc<SignalBus> {
        &self.bus
    }
}

impl Drop for Win32SystemSensors {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        for t in &self.parked {
            t.unpark();
        }
        self.media.stop();
        input::request_close(self.input_hwnd.load(Ordering::Acquire));
    }
}

impl SystemSensors for Win32SystemSensors {
    fn signal_bus(&self) -> Option<&Arc<SignalBus>> {
        Some(&self.bus)
    }

    fn set_demand(&mut self, demand: SignalKind) {
        self.bus.set_demand(demand);
        if !self.shared.listener_installed.load(Ordering::Acquire) {
            // Sem listener (bus reaproveitado): acorda manualmente.
            for t in &self.parked {
                t.unpark();
            }
            self.media.notify_demand();
            input::notify_demand(self.input_hwnd.load(Ordering::Acquire));
        }
    }

    fn system_stats(&mut self) -> PlatformResult<SystemStats> {
        // RAM/energia lidas na hora (baratas); CPU/GPU dependem de delta → valor do worker.
        let t = self.bus.telemetry();
        let (ram_percent, _, _) = telemetry::sample_memory();
        let power = telemetry::sample_power();
        Ok(SystemStats {
            cpu_percent: t.cpu_percent,
            ram_percent,
            gpu_percent: t.gpu_percent,
            battery_percent: power.battery_percent,
            on_battery: power.on_battery,
            battery_saver: power.battery_saver,
        })
    }

    fn current_media(&mut self) -> PlatformResult<Option<MediaSession>> {
        let m = self.bus.media_scalars();
        if !m.has_session {
            return Ok(None);
        }
        let text = self.bus.media_text();
        let rgba = m.dominant_rgba;
        Ok(Some(MediaSession {
            title: text.title,
            artist: text.artist,
            album: text.album,
            playing: m.playing,
            position_ms: m.position_ms,
            duration_ms: m.duration_ms,
            cover_path: (!text.cover_path.is_empty()).then(|| PathBuf::from(text.cover_path)),
            dominant_color: (rgba != 0).then(|| rgba.to_be_bytes()),
        }))
    }

    fn media_control(&mut self, command: MediaCommand) -> PlatformResult<()> {
        self.media.push_command(command);
        Ok(())
    }

    fn audio_bands(&mut self, bands: &mut [f32]) -> PlatformResult<usize> {
        if !self.bus.is_demanded(SignalKind::AUDIO) {
            bands.iter_mut().take(BAND_COUNT).for_each(|b| *b = 0.0);
            return Ok(bands.len().min(BAND_COUNT));
        }
        Ok(self.bus.copy_bands(bands))
    }
}
