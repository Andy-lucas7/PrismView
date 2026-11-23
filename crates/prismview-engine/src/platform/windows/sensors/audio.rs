//! WASAPI loopback do dispositivo de saída padrão → `AudioAnalyzer` (core) → `SignalBus`.
//!
//! Porte do `run_wasapi_loopback_session` do Estúdio com melhorias:
//! - Modo por evento (`AUDCLNT_STREAMFLAGS_EVENTCALLBACK`): sem áudio tocando o Windows não
//!   entrega pacotes e a thread fica bloqueada em `WaitForSingleObject` (0 % de CPU). Fallback
//!   para polling de 10 ms se o driver recusar o evento.
//! - Formato lido do `WAVEFORMATEXTENSIBLE::SubFormat` (float32, PCM 16/24/32).
//! - Troca de dispositivo padrão detectada comparando o ID a cada `DEVICE_CHECK`; dispositivo
//!   invalidado → reconecta com backoff.
//! - Silêncio: decai até zero, publica uma última vez e para de publicar (não acorda o loop).
//! - Fora da demanda `AUDIO` o cliente é parado e liberado; a thread estaciona (`park`).

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::windows::core::{GUID, PCWSTR};
use ::windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use ::windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
};
use ::windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use ::windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::{ComMta, Shared};
use crate::engine_core::audio::AudioAnalyzer;
use crate::engine_core::signals::SignalKind;

/// Buffer compartilhado do loopback (100 ns): 20 ms.
const BUFFER_HNS: i64 = 200_000;
const EVENT_WAIT_MS: u32 = 100;
const IDLE_WAIT_MS: u32 = 250;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEVICE_CHECK: Duration = Duration::from_secs(2);
const RECONNECT_BACKOFF: Duration = Duration::from_millis(500);
/// Sem pacotes por esse tempo → injeta silêncio para os envelopes decaírem.
const SILENCE_AFTER: Duration = Duration::from_millis(60);

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
/// Offset do `SubFormat` dentro de `WAVEFORMATEXTENSIBLE` (estrutura empacotada).
const SUBFORMAT_OFFSET: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SampleFormat {
    F32,
    I16,
    I24,
    I32,
}

#[derive(Debug, Clone, Copy)]
struct StreamFormat {
    sample: SampleFormat,
    channels: usize,
    block_align: usize,
    sample_rate: u32,
}

enum SessionEnd {
    /// Demanda removida ou parada: não reconectar.
    Idle,
    /// Dispositivo padrão mudou: reconectar já.
    DeviceChanged,
    /// Falha/invalidação: reconectar após backoff.
    Failed,
}

struct EventHandle(HANDLE);

impl Drop for EventHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

pub(super) fn run(shared: Arc<Shared>) {
    let mut com: Option<ComMta> = None;
    let mut analyzer: Option<AudioAnalyzer> = None;
    let mut device_id: Vec<u16> = Vec::new();
    let mut id_scratch: Vec<u16> = Vec::new();

    while !shared.stop.load(Ordering::Relaxed) {
        if !shared.bus.is_demanded(SignalKind::AUDIO) {
            shared.bus.clear_audio();
            // Sem demanda de áudio: libera FFT, buffers e COM até ser necessário.
            analyzer = None;
            com = None;
            device_id = Vec::new();
            id_scratch = Vec::new();
            shared.idle_wait();
            continue;
        }
        if com.is_none() {
            com = Some(ComMta::init());
        }
        let analyzer = analyzer.get_or_insert_with(|| AudioAnalyzer::new(48_000));
        if device_id.capacity() == 0 {
            device_id.reserve(128);
            id_scratch.reserve(128);
        }
        let end = unsafe { run_session(&shared, analyzer, &mut device_id, &mut id_scratch) };
        match end {
            SessionEnd::Idle | SessionEnd::DeviceChanged => {}
            SessionEnd::Failed => {
                shared.bus.clear_audio();
                std::thread::park_timeout(RECONNECT_BACKOFF);
            }
        }
    }
    shared.bus.clear_audio();
}

unsafe fn default_device(enumerator: &IMMDeviceEnumerator) -> Option<IMMDevice> {
    enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()
}

/// Copia o ID do dispositivo em `out` (reaproveita a capacidade).
unsafe fn read_device_id(device: &IMMDevice, out: &mut Vec<u16>) -> bool {
    out.clear();
    let Ok(id) = device.GetId() else {
        return false;
    };
    if id.is_null() {
        return false;
    }
    out.extend_from_slice(id.as_wide());
    CoTaskMemFree(Some(id.0 as *const c_void));
    true
}

unsafe fn parse_format(mix: *const WAVEFORMATEX) -> Option<StreamFormat> {
    let fmt = std::ptr::read_unaligned(mix);
    let tag = fmt.wFormatTag;
    let bits = fmt.wBitsPerSample;
    let channels = fmt.nChannels as usize;
    let block_align = fmt.nBlockAlign as usize;
    if channels == 0 || block_align == 0 {
        return None;
    }
    let float = match tag {
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_PCM => false,
        WAVE_FORMAT_EXTENSIBLE if fmt.cbSize >= 22 => {
            let sub: GUID =
                std::ptr::read_unaligned((mix as *const u8).add(SUBFORMAT_OFFSET) as *const GUID);
            if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                true
            } else if sub == KSDATAFORMAT_SUBTYPE_PCM {
                false
            } else {
                return None;
            }
        }
        _ => return None,
    };
    let sample = match (float, bits) {
        (true, 32) => SampleFormat::F32,
        (false, 16) => SampleFormat::I16,
        (false, 24) => SampleFormat::I24,
        (false, 32) => SampleFormat::I32,
        _ => return None,
    };
    let bytes_per_sample = (bits / 8) as usize;
    if block_align < channels * bytes_per_sample {
        return None;
    }
    Some(StreamFormat {
        sample,
        channels,
        block_align,
        sample_rate: fmt.nSamplesPerSec,
    })
}

#[inline]
fn read_sample(frame: &[u8], offset: usize, format: SampleFormat) -> f32 {
    match format {
        SampleFormat::F32 => f32::from_le_bytes([
            frame[offset],
            frame[offset + 1],
            frame[offset + 2],
            frame[offset + 3],
        ]),
        SampleFormat::I16 => i16::from_le_bytes([frame[offset], frame[offset + 1]]) as f32 / 32_768.0,
        SampleFormat::I24 => {
            let v = i32::from_le_bytes([0, frame[offset], frame[offset + 1], frame[offset + 2]]) >> 8;
            v as f32 / 8_388_608.0
        }
        SampleFormat::I32 => {
            i32::from_le_bytes([frame[offset], frame[offset + 1], frame[offset + 2], frame[offset + 3]])
                as f32
                / 2_147_483_648.0
        }
    }
}

/// Inicializa o cliente em modo evento; se o driver recusar, reativa em modo polling.
unsafe fn open_client(
    device: &IMMDevice,
) -> Option<(IAudioClient, StreamFormat, Option<EventHandle>)> {
    for event_mode in [true, false] {
        let Ok(client) = device.Activate::<IAudioClient>(CLSCTX_ALL, None) else {
            return None;
        };
        let Ok(mix) = client.GetMixFormat() else {
            return None;
        };
        if mix.is_null() {
            return None;
        }
        let format = parse_format(mix);
        let flags = if event_mode {
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        } else {
            AUDCLNT_STREAMFLAGS_LOOPBACK
        };
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, mix, None);
        CoTaskMemFree(Some(mix as *const c_void));
        let Some(format) = format else {
            return None;
        };
        if init.is_err() {
            continue;
        }
        if !event_mode {
            return Some((client, format, None));
        }
        let Ok(event) = CreateEventW(None, BOOL(0), BOOL(0), PCWSTR::null()) else {
            continue;
        };
        let event = EventHandle(event);
        if client.SetEventHandle(event.0).is_ok() {
            return Some((client, format, Some(event)));
        }
    }
    None
}

unsafe fn run_session(
    shared: &Shared,
    analyzer: &mut AudioAnalyzer,
    device_id: &mut Vec<u16>,
    id_scratch: &mut Vec<u16>,
) -> SessionEnd {
    let bus = &shared.bus;
    let enumerator: IMMDeviceEnumerator =
        match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(_) => return SessionEnd::Failed,
        };
    let Some(device) = default_device(&enumerator) else {
        return SessionEnd::Failed;
    };
    read_device_id(&device, device_id);
    let Some((client, format, event)) = open_client(&device) else {
        return SessionEnd::Failed;
    };
    let Ok(capture) = client.GetService::<IAudioCaptureClient>() else {
        return SessionEnd::Failed;
    };
    if analyzer.sample_rate() != format.sample_rate {
        analyzer.set_sample_rate(format.sample_rate);
    } else {
        analyzer.reset();
    }
    if client.Start().is_err() {
        return SessionEnd::Failed;
    }

    let inv_channels = 1.0 / format.channels as f32;
    let mut last_analysis = Instant::now();
    let mut last_packet = Instant::now();
    let mut last_device_check = Instant::now();
    let mut published_silent = false;

    let end = loop {
        if shared.stop.load(Ordering::Relaxed) || !bus.is_demanded(SignalKind::AUDIO) {
            break SessionEnd::Idle;
        }

        match &event {
            Some(ev) => {
                // Em silêncio o Windows não sinaliza: o timeout só serve para notar parada,
                // demanda removida e troca de dispositivo.
                let wait = if published_silent { IDLE_WAIT_MS } else { EVENT_WAIT_MS };
                let _ = WaitForSingleObject(ev.0, wait);
            }
            None => std::thread::sleep(POLL_INTERVAL),
        }

        // Drena todos os pacotes disponíveis.
        let mut got = false;
        let mut failed = false;
        loop {
            let packet = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => {
                    failed = true; // AUDCLNT_E_DEVICE_INVALIDATED etc.
                    break;
                }
            };
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            if capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                failed = true;
                break;
            }
            let n = frames as usize;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 || data.is_null() {
                analyzer.push_silence(n);
            } else {
                let bytes = std::slice::from_raw_parts(data as *const u8, n * format.block_align);
                let width = match format.sample {
                    SampleFormat::I16 => 2,
                    SampleFormat::I24 => 3,
                    SampleFormat::F32 | SampleFormat::I32 => 4,
                };
                for frame in bytes.chunks_exact(format.block_align) {
                    let mut mono = 0.0f32;
                    for c in 0..format.channels {
                        mono += read_sample(frame, c * width, format.sample);
                    }
                    analyzer.push(mono * inv_channels);
                }
            }
            let _ = capture.ReleaseBuffer(frames);
            got = true;
        }
        if failed {
            break SessionEnd::Failed;
        }

        let now = Instant::now();
        if got {
            last_packet = now;
        } else if now.duration_since(last_packet) >= SILENCE_AFTER && !published_silent {
            // Nada tocando: alimenta silêncio para os envelopes decaírem até publicar o zero.
            let hop = (format.sample_rate / 60).max(128) as usize;
            analyzer.push_silence(hop);
        }

        if analyzer.ready() {
            let dt = now.duration_since(last_analysis).as_secs_f32();
            last_analysis = now;
            let frame = analyzer.analyze(dt);
            if !(frame.silent && published_silent) {
                bus.publish_audio(frame);
                published_silent = frame.silent;
            }
        }

        if now.duration_since(last_device_check) >= DEVICE_CHECK {
            last_device_check = now;
            let changed = match default_device(&enumerator) {
                Some(d) => read_device_id(&d, id_scratch) && id_scratch != device_id,
                None => true,
            };
            if changed {
                break SessionEnd::DeviceChanged;
            }
        }
    };

    let _ = client.Stop();
    drop(event);
    end
}
