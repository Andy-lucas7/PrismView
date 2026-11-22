//! Camada de abstração de plataforma do PrismView Engine.
//!
//! Contratos (traits) e tipos neutros. Implementação Win32 em `platform/windows/`;
//! macOS/Linux são placeholders (`Unsupported`).
//! Este arquivo não depende de `windows`/Win32.

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::engine_core::signals::{SignalBus, SignalKind};

// ───────────────────────── Erros ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    /// Recurso não implementado nesta plataforma (macOS/Linux por enquanto).
    Unsupported(&'static str),
    /// Falha reportada pelo SO/driver.
    Os(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "não suportado nesta plataforma: {what}"),
            Self::Os(msg) => write!(f, "erro do sistema: {msg}"),
        }
    }
}

impl std::error::Error for PlatformError {}

pub type PlatformResult<T> = Result<T, PlatformError>;

// ───────────────────────── Tipos neutros ─────────────────────────

/// Handle nativo opaco da janela (HWND no Windows, NSWindow*/X11 Window depois).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindowId(pub isize);

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: String,
    /// Retângulo em pixels físicos, coordenadas do desktop virtual.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopVisibility {
    Visible,
    /// Desktop coberto por janela maximizada/fullscreen → render pode pausar.
    Covered,
    /// Sessão bloqueada / tela desligada.
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEvent {
    VisibilityChanged(DesktopVisibility),
    MonitorsChanged,
    /// Shell reiniciou (ex.: explorer.exe); é preciso reanexar.
    ShellRestarted,
}

// ───────────────────────── DesktopHost ─────────────────────────

/// Anexa janelas de wallpaper atrás dos ícones do desktop e observa o ambiente.
pub trait DesktopHost {
    fn list_monitors(&self) -> PlatformResult<Vec<MonitorInfo>>;

    /// Anexa `window` como wallpaper do monitor (WorkerW no Windows).
    fn attach_wallpaper(&mut self, window: NativeWindowId, monitor_id: &str) -> PlatformResult<()>;

    fn detach_wallpaper(&mut self, window: NativeWindowId) -> PlatformResult<()>;

    /// Estado atual de visibilidade do desktop.
    fn visibility(&self) -> PlatformResult<DesktopVisibility>;

    /// Posição global do cursor em pixels físicos.
    fn cursor_position(&self) -> PlatformResult<(i32, i32)>;

    /// Drena eventos pendentes (não bloqueante).
    fn poll_events(&mut self) -> Vec<DesktopEvent>;
}

// ───────────────────────── HardwareVideoDecoder ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodedFrameFormat {
    Rgba8,
    Bgra8,
    Nv12,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub duration_ms: Option<u64>,
    pub codec: Option<VideoCodec>,
}

/// Frame decodificado em memória de CPU (caminho legado/fallback). Backends zero-copy
/// entregam `GpuVideoFrame` e podem nunca produzir este tipo.
#[derive(Debug)]
pub struct DecodedFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub format: DecodedFrameFormat,
    pub data: &'a [u8],
    pub timestamp_ms: u64,
}

/// Frame de vídeo já residente na GPU (caminho zero-copy): a textura pertence ao decoder e é
/// reaproveitada (anel de slots), então nada é alocado por frame.
#[derive(Debug, Clone, Copy)]
pub struct GpuVideoFrame<'a> {
    pub texture: &'a wgpu::Texture,
    pub view: &'a wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    /// `Bgra8Unorm`, valores na codificação da tela. O canal alpha NÃO é garantido (vídeo é
    /// opaco): amostrar forçando alpha = 1.
    pub format: wgpu::TextureFormat,
    pub timestamp_ms: u64,
    /// Índice estável do slot no anel: permite cachear um bind group por slot.
    pub slot: usize,
    /// Muda a cada `open`/`close` (texturas recriadas → descartar bind groups cacheados).
    pub generation: u64,
}

/// Decodificador de vídeo acelerado por hardware (Media Foundation, VideoToolbox, VAAPI).
///
/// Caminho zero-copy (preferido): `bind_gpu` → `open` → a cada frame renderizado `advance` +
/// `current_gpu_frame`; `time_until_next_frame` diz quando acordar o loop.
pub trait HardwareVideoDecoder {
    /// O backend consegue decodificar `codec` em hardware nesta máquina?
    fn supports(&self, codec: VideoCodec) -> bool;

    /// Liga o decoder ao device wgpu do renderer. Necessário antes de `open` nos backends
    /// zero-copy (o decoder precisa decodificar no mesmo adapter físico).
    fn bind_gpu(&mut self, device: &wgpu::Device) -> PlatformResult<()>;

    fn open(&mut self, path: &Path) -> PlatformResult<VideoInfo>;

    fn set_looping(&mut self, looping: bool);
    fn set_paused(&mut self, paused: bool);
    fn seek_ms(&mut self, position_ms: u64) -> PlatformResult<()>;

    /// Caminho de CPU: se houver frame novo desde a última chamada, entrega-o; senão `None`.
    /// O slice vale até a próxima chamada (buffer reutilizado, sem alocar por frame).
    /// Backends zero-copy retornam sempre `None`.
    fn next_frame(&mut self) -> Option<DecodedFrame<'_>>;

    /// Avança o relógio de reprodução e troca o frame exibido se outro já deveria estar na
    /// tela. Chamar exatamente 1× por frame renderizado (antes de gravar o passe). `true` se o
    /// frame exibido mudou.
    fn advance(&mut self) -> bool;

    /// Frame atualmente exibido (zero-copy). `None` antes do primeiro frame decodificado.
    fn current_gpu_frame(&self) -> Option<GpuVideoFrame<'_>>;

    /// Quanto falta para o próximo frame (para `ControlFlow::WaitUntil`). `None` = nada a
    /// esperar (pausado, fim sem loop, fechado).
    fn time_until_next_frame(&self) -> Option<Duration>;

    /// Erro assíncrono do decodificador (stream corrompido, device removido...).
    fn last_error(&self) -> Option<String>;

    fn close(&mut self);
}

// ───────────────────────── SystemSensors ─────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SystemStats {
    pub cpu_percent: f32,
    pub ram_percent: f32,
    /// Utilização do motor 3D da GPU (0..=100); `None` se o SO não expõe.
    pub gpu_percent: Option<f32>,
    pub battery_percent: Option<f32>,
    pub on_battery: bool,
    pub battery_saver: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaSession {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Capa salva em disco (o frontend não recebe `blob:` entre WebViews).
    pub cover_path: Option<std::path::PathBuf>,
    /// Cor dominante da capa (RGBA8).
    pub dominant_color: Option<[u8; 4]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

/// Telemetria do sistema, mídia atual, entrada global e áudio reativo.
///
/// Os backends alimentam um `core::signals::SignalBus` em threads próprias; o renderer lê por
/// frame com `SignalReader` (lock-free). Os métodos abaixo são a visão "pull" do mesmo estado
/// (Estúdio/IPC/diagnóstico) e não bloqueiam.
pub trait SystemSensors {
    /// Barramento alimentado por este backend (`None` se não houver produtores).
    fn signal_bus(&self) -> Option<&Arc<SignalBus>>;

    /// Sinais que a cena consome. Produtores fora da demanda dormem (0 % de CPU) e o
    /// dispositivo é liberado (loopback de áudio, raw input).
    fn set_demand(&mut self, demand: SignalKind);

    fn system_stats(&mut self) -> PlatformResult<SystemStats>;

    fn current_media(&mut self) -> PlatformResult<Option<MediaSession>>;
    fn media_control(&mut self, command: MediaCommand) -> PlatformResult<()>;

    /// Preenche `bands` com a energia do áudio do sistema (0.0..=1.0) via FFT.
    /// Retorna quantas bandas foram escritas. Não aloca.
    fn audio_bands(&mut self, bands: &mut [f32]) -> PlatformResult<usize>;
}

// ───────────────────────── Placeholders não-Windows ─────────────────────────

/// Backend vazio para macOS/Linux: toda operação retorna `PlatformError::Unsupported`.
#[cfg(not(windows))]
#[derive(Debug, Default)]
pub struct UnsupportedPlatform;

#[cfg(not(windows))]
impl DesktopHost for UnsupportedPlatform {
    fn list_monitors(&self) -> PlatformResult<Vec<MonitorInfo>> {
        Err(PlatformError::Unsupported("DesktopHost::list_monitors"))
    }
    fn attach_wallpaper(&mut self, _: NativeWindowId, _: &str) -> PlatformResult<()> {
        // TODO(macOS): NSWindow level kCGDesktopWindowLevel. TODO(Linux): X11 _NET_WM_WINDOW_TYPE_DESKTOP / layer-shell.
        Err(PlatformError::Unsupported("DesktopHost::attach_wallpaper"))
    }
    fn detach_wallpaper(&mut self, _: NativeWindowId) -> PlatformResult<()> {
        Err(PlatformError::Unsupported("DesktopHost::detach_wallpaper"))
    }
    fn visibility(&self) -> PlatformResult<DesktopVisibility> {
        Err(PlatformError::Unsupported("DesktopHost::visibility"))
    }
    fn cursor_position(&self) -> PlatformResult<(i32, i32)> {
        Err(PlatformError::Unsupported("DesktopHost::cursor_position"))
    }
    fn poll_events(&mut self) -> Vec<DesktopEvent> {
        Vec::new()
    }
}

#[cfg(not(windows))]
impl HardwareVideoDecoder for UnsupportedPlatform {
    fn supports(&self, _: VideoCodec) -> bool {
        false // TODO(macOS): VideoToolbox. TODO(Linux): VAAPI/VDPAU.
    }
    fn bind_gpu(&mut self, _: &wgpu::Device) -> PlatformResult<()> {
        // TODO(macOS): IOSurface → MTLTexture. TODO(Linux): dmabuf → Vulkan external memory.
        Err(PlatformError::Unsupported("HardwareVideoDecoder::bind_gpu"))
    }
    fn open(&mut self, _: &Path) -> PlatformResult<VideoInfo> {
        Err(PlatformError::Unsupported("HardwareVideoDecoder::open"))
    }
    fn set_looping(&mut self, _: bool) {}
    fn set_paused(&mut self, _: bool) {}
    fn seek_ms(&mut self, _: u64) -> PlatformResult<()> {
        Err(PlatformError::Unsupported("HardwareVideoDecoder::seek_ms"))
    }
    fn next_frame(&mut self) -> Option<DecodedFrame<'_>> {
        None
    }
    fn advance(&mut self) -> bool {
        false
    }
    fn current_gpu_frame(&self) -> Option<GpuVideoFrame<'_>> {
        None
    }
    fn time_until_next_frame(&self) -> Option<Duration> {
        None
    }
    fn last_error(&self) -> Option<String> {
        None
    }
    fn close(&mut self) {}
}

#[cfg(not(windows))]
impl SystemSensors for UnsupportedPlatform {
    fn signal_bus(&self) -> Option<&Arc<SignalBus>> {
        None // TODO(macOS): CoreAudio tap + IOHID. TODO(Linux): PipeWire monitor + libinput.
    }
    fn set_demand(&mut self, _: SignalKind) {}
    fn system_stats(&mut self) -> PlatformResult<SystemStats> {
        Err(PlatformError::Unsupported("SystemSensors::system_stats"))
    }
    fn current_media(&mut self) -> PlatformResult<Option<MediaSession>> {
        // TODO(macOS): MediaRemote. TODO(Linux): MPRIS (D-Bus).
        Err(PlatformError::Unsupported("SystemSensors::current_media"))
    }
    fn media_control(&mut self, _: MediaCommand) -> PlatformResult<()> {
        Err(PlatformError::Unsupported("SystemSensors::media_control"))
    }
    fn audio_bands(&mut self, _: &mut [f32]) -> PlatformResult<usize> {
        Err(PlatformError::Unsupported("SystemSensors::audio_bands"))
    }
}

// ───────────────────────── Backend Windows ─────────────────────────

/// Win32: WorkerW, multi-monitor PMv2, SetWinEventHook, loop `ControlFlow` e
/// `MfVideoDecoder` (Media Foundation + D3D11 → D3D12/wgpu zero-copy) e `Win32SystemSensors`
/// (WASAPI loopback, Raw Input, SMTC, telemetria) alimentando o `SignalBus`.
#[cfg(windows)]
pub mod windows;
