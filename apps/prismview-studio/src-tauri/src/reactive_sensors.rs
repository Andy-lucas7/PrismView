use crate::desktop_host::ComApartmentGuard;
use serde::Serialize;
use std::f32::consts::PI;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    PostQuitMessage, RegisterClassExW, SetTimer, TranslateMessage, UnregisterClassW, HWND_MESSAGE,
    MSG, WINDOW_STYLE, WM_CLOSE, WM_INPUT, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE,
};

/// Real-time reactive signal payload emitted on `prism://reactive` ONLY when
/// at least one visible wallpaper/widget (or Studio preview) subscribes to audio or keyboard.
#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct ReactiveSignalDto {
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub level: f32,
    pub key_pulse: f32,
    pub typing_energy: f32,
}

impl ReactiveSignalDto {
    pub fn is_quiet(&self) -> bool {
        self.bass < 0.004
            && self.mid < 0.004
            && self.treble < 0.004
            && self.level < 0.004
            && self.key_pulse < 0.004
            && self.typing_energy < 0.004
    }
}

#[derive(Default)]
struct AudioBands {
    bass: f32,
    mid: f32,
    treble: f32,
    level: f32,
}

pub struct ReactiveSensorsHandle {
    audio_enabled: Arc<AtomicBool>,
    keyboard_enabled: Arc<AtomicBool>,
    audio_bands: Arc<Mutex<AudioBands>>,
    key_press_counter: Arc<AtomicU32>,
    key_state: Mutex<(f32, f32, Instant)>,
}

impl ReactiveSensorsHandle {
    pub fn set_active(&self, audio: bool, keyboard: bool) {
        self.audio_enabled.store(audio, Ordering::Relaxed);
        self.keyboard_enabled.store(keyboard, Ordering::Relaxed);
    }

    pub fn snapshot(&self, include_audio: bool, include_keyboard: bool) -> ReactiveSignalDto {
        let mut dto = ReactiveSignalDto::default();
        if include_audio {
            if let Ok(b) = self.audio_bands.lock() {
                dto.bass = b.bass;
                dto.mid = b.mid;
                dto.treble = b.treble;
                dto.level = b.level;
            }
        }
        if include_keyboard {
            let presses = self.key_press_counter.swap(0, Ordering::Relaxed);
            if let Ok(mut ks) = self.key_state.lock() {
                let now = Instant::now();
                let dt = now.duration_since(ks.2).as_secs_f32().clamp(0.001, 0.25);
                ks.2 = now;
                // Exponential decay for crisp typing pulse + smoother cadence energy
                ks.0 *= (-8.5 * dt).exp();
                ks.1 *= (-1.8 * dt).exp();
                if presses > 0 {
                    ks.0 = (ks.0 + 0.72 * (presses as f32).min(3.0)).min(1.0);
                    ks.1 = (ks.1 + 0.22 * (presses as f32).min(4.0)).min(1.0);
                }
                if ks.0 < 0.002 {
                    ks.0 = 0.0;
                }
                if ks.1 < 0.002 {
                    ks.1 = 0.0;
                }
                dto.key_pulse = (ks.0 * 1000.0).round() / 1000.0;
                dto.typing_energy = (ks.1 * 1000.0).round() / 1000.0;
            }
        } else {
            let _ = self.key_press_counter.swap(0, Ordering::Relaxed);
        }
        dto
    }
}

const FFT_N: usize = 256;

/// Pure-Rust zero-allocation 256-point Radix-2 Cooley-Tukey FFT.
/// Returns normalized `(bass, mid, treble, rms_level)` in `0.0..=1.0`.
pub fn compute_fft_bands(samples: &[f32; FFT_N], sample_rate: u32) -> (f32, f32, f32, f32) {
    let mut re = [0.0f32; FFT_N];
    let mut im = [0.0f32; FFT_N];
    let mut sum_sq = 0.0f32;

    for i in 0..FFT_N {
        let s = samples[i];
        sum_sq += s * s;
        // Hann window eliminates spectral leakage
        let w = 0.5 * (1.0 - (2.0 * PI * i as f32 / (FFT_N as f32 - 1.0)).cos());
        re[i] = s * w;
    }

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..FFT_N {
        let mut bit = FFT_N >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
        }
    }

    // Butterfly stages
    let mut len = 2usize;
    while len <= FFT_N {
        let half = len >> 1;
        let ang = -2.0 * PI / (len as f32);
        let wlen_re = ang.cos();
        let wlen_im = ang.sin();
        let mut i = 0;
        while i < FFT_N {
            let mut w_re = 1.0f32;
            let mut w_im = 0.0f32;
            for k in 0..half {
                let u_re = re[i + k];
                let u_im = im[i + k];
                let v_re = re[i + k + half] * w_re - im[i + k + half] * w_im;
                let v_im = re[i + k + half] * w_im + im[i + k + half] * w_re;
                re[i + k] = u_re + v_re;
                im[i + k] = u_im + v_im;
                re[i + k + half] = u_re - v_re;
                im[i + k + half] = u_im - v_im;
                let next_re = w_re * wlen_re - w_im * wlen_im;
                w_im = w_re * wlen_im + w_im * wlen_re;
                w_re = next_re;
            }
            i += len;
        }
        len <<= 1;
    }

    let sr = sample_rate.max(8000) as f32;
    let bin_hz = sr / FFT_N as f32;
    let inv_n = 2.0 / FFT_N as f32;

    let mut bass_sum = 0.0f32;
    let mut bass_peak = 0.0f32;
    let mut bass_cnt = 0.0f32;
    let mut mid_sum = 0.0f32;
    let mut mid_peak = 0.0f32;
    let mut mid_cnt = 0.0f32;
    let mut treb_sum = 0.0f32;
    let mut treb_peak = 0.0f32;
    let mut treb_cnt = 0.0f32;

    for bin in 1..(FFT_N / 2) {
        let freq = bin as f32 * bin_hz;
        let mag = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() * inv_n;
        if freq < 260.0 {
            bass_sum += mag;
            if mag > bass_peak {
                bass_peak = mag;
            }
            bass_cnt += 1.0;
        } else if freq < 2200.0 {
            mid_sum += mag;
            if mag > mid_peak {
                mid_peak = mag;
            }
            mid_cnt += 1.0;
        } else if freq <= 12000.0 {
            treb_sum += mag;
            if mag > treb_peak {
                treb_peak = mag;
            }
            treb_cnt += 1.0;
        }
    }

    let norm = |raw: f32, gain: f32| -> f32 {
        // Gentle perceptual compression so both quiet lo-fi and loud EDM look crisp
        let v = (raw * gain).max(0.0);
        (v / (1.0 + v * 0.65)).clamp(0.0, 1.0)
    };

    let band_energy = |sum: f32, peak: f32, cnt: f32| -> f32 {
        if cnt <= 0.0 {
            return 0.0;
        }
        (sum / cnt) * 0.35 + peak * 0.65
    };

    let bass = norm(band_energy(bass_sum, bass_peak, bass_cnt), 5.5);
    let mid = norm(band_energy(mid_sum, mid_peak, mid_cnt), 7.5);
    let treble = norm(band_energy(treb_sum, treb_peak, treb_cnt), 9.5);
    let rms = (sum_sq / FFT_N as f32).sqrt();
    let level = norm(rms, 4.2);

    (bass, mid, treble, level)
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;

unsafe fn run_wasapi_loopback_session(
    stop: &AtomicBool,
    enabled: &AtomicBool,
    bands_out: &Mutex<AudioBands>,
) {
    let enumerator: IMMDeviceEnumerator =
        match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(_) => return,
        };
    let device = match enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
        Ok(d) => d,
        Err(_) => return,
    };
    let audio_client: IAudioClient = match device.Activate(CLSCTX_ALL, None) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mix_ptr: *mut WAVEFORMATEX = match audio_client.GetMixFormat() {
        Ok(p) if !p.is_null() => p,
        _ => return,
    };

    let fmt = *mix_ptr;
    let channels = (fmt.nChannels as usize).max(1);
    let sample_rate = fmt.nSamplesPerSec.max(8000);
    let bits = fmt.wBitsPerSample;
    let format_tag = fmt.wFormatTag;
    let is_float = format_tag == WAVE_FORMAT_IEEE_FLOAT
        || (format_tag == WAVE_FORMAT_EXTENSIBLE && bits == 32);
    let is_pcm16 = format_tag == WAVE_FORMAT_PCM && bits == 16;

    // 40ms shared-mode loopback buffer (400_000 in 100ns units)
    let init_res = audio_client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
        400_000,
        0,
        mix_ptr,
        None,
    );
    CoTaskMemFree(Some(mix_ptr as *const core::ffi::c_void));
    if init_res.is_err() {
        return;
    }

    let capture: IAudioCaptureClient = match audio_client.GetService() {
        Ok(c) => c,
        Err(_) => return,
    };
    if audio_client.Start().is_err() {
        return;
    }

    let mut ring = [0.0f32; FFT_N];
    let mut ring_pos = 0usize;
    let mut smooth_b = 0.0f32;
    let mut smooth_m = 0.0f32;
    let mut smooth_t = 0.0f32;
    let mut smooth_l = 0.0f32;

    while !stop.load(Ordering::Relaxed) && enabled.load(Ordering::Relaxed) {
        let mut got_audio = false;
        loop {
            let pkt_len = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => {
                    // Audio endpoint changed / invalidated -> break to reconnect cleanly
                    let _ = audio_client.Stop();
                    return;
                }
            };
            if pkt_len == 0 {
                break;
            }
            let mut data_ptr: *mut u8 = std::ptr::null_mut();
            let mut num_frames: u32 = 0;
            let mut flags: u32 = 0;
            if capture
                .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                .is_err()
            {
                let _ = audio_client.Stop();
                return;
            }
            let frames = num_frames as usize;
            if (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || data_ptr.is_null() {
                for _ in 0..frames.min(FFT_N) {
                    ring[ring_pos] = 0.0;
                    ring_pos = (ring_pos + 1) % FFT_N;
                }
            } else if is_float {
                let floats = std::slice::from_raw_parts(data_ptr as *const f32, frames * channels);
                for f in 0..frames {
                    let mut mono = 0.0f32;
                    let base = f * channels;
                    for c in 0..channels {
                        mono += floats[base + c];
                    }
                    ring[ring_pos] = mono / channels as f32;
                    ring_pos = (ring_pos + 1) % FFT_N;
                }
                got_audio = true;
            } else if is_pcm16 {
                let ints = std::slice::from_raw_parts(data_ptr as *const i16, frames * channels);
                for f in 0..frames {
                    let mut mono = 0.0f32;
                    let base = f * channels;
                    for c in 0..channels {
                        mono += ints[base + c] as f32 / 32768.0;
                    }
                    ring[ring_pos] = mono / channels as f32;
                    ring_pos = (ring_pos + 1) % FFT_N;
                }
                got_audio = true;
            }
            let _ = capture.ReleaseBuffer(num_frames);
        }

        let (tb, tm, tt, tl) = if got_audio {
            // Unroll ring into chronological order for accurate FFT
            let mut ordered = [0.0f32; FFT_N];
            for i in 0..FFT_N {
                ordered[i] = ring[(ring_pos + i) % FFT_N];
            }
            compute_fft_bands(&ordered, sample_rate)
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        // Fast attack (0.58) / smooth release (0.20) for organic visual reactivity
        let env = |cur: f32, target: f32| -> f32 {
            let k = if target > cur { 0.58 } else { 0.20 };
            let next = cur + (target - cur) * k;
            if next < 0.002 {
                0.0
            } else {
                next
            }
        };
        smooth_b = env(smooth_b, tb);
        smooth_m = env(smooth_m, tm);
        smooth_t = env(smooth_t, tt);
        smooth_l = env(smooth_l, tl);

        if let Ok(mut guard) = bands_out.lock() {
            guard.bass = (smooth_b * 1000.0).round() / 1000.0;
            guard.mid = (smooth_m * 1000.0).round() / 1000.0;
            guard.treble = (smooth_t * 1000.0).round() / 1000.0;
            guard.level = (smooth_l * 1000.0).round() / 1000.0;
        }

        thread::sleep(Duration::from_millis(20));
    }

    let _ = audio_client.Stop();
    if let Ok(mut guard) = bands_out.lock() {
        *guard = AudioBands::default();
    }
}

struct KeySinkState {
    stop: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    counter: Arc<AtomicU32>,
}

std::thread_local! {
    static KEY_SINK_STATE: std::cell::RefCell<Option<KeySinkState>> = const { std::cell::RefCell::new(None) };
}

const KEY_SINK_TIMER_ID: usize = 0x5053;
const RI_KEY_BREAK: u16 = 0x0001;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lpmodulename: *const u16) -> *mut core::ffi::c_void;
}

unsafe extern "system" fn keyboard_raw_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == KEY_SINK_TIMER_ID => {
            let should_quit = KEY_SINK_STATE.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|s| s.stop.load(Ordering::Relaxed))
                    .unwrap_or(true)
            });
            if should_quit {
                let _ = KillTimer(hwnd, KEY_SINK_TIMER_ID);
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        WM_INPUT => {
            // Privacy-first: ONLY check whether a key-down occurred when keyboard reactivity is enabled.
            // NEVER inspect, store, or transmit VKey / MakeCode / characters.
            let is_active = KEY_SINK_STATE.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|s| s.enabled.load(Ordering::Relaxed))
                    .unwrap_or(false)
            });
            if is_active {
                let mut raw = RAWINPUT::default();
                let mut size = std::mem::size_of::<RAWINPUT>() as u32;
                let res = GetRawInputData(
                    HRAWINPUT(lparam.0 as *mut core::ffi::c_void),
                    RID_INPUT,
                    Some(&mut raw as *mut RAWINPUT as *mut core::ffi::c_void),
                    &mut size,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                );
                if res != u32::MAX && raw.header.dwType == RIM_TYPEKEYBOARD.0 {
                    let flags = raw.data.keyboard.Flags;
                    if (flags & RI_KEY_BREAK) == 0 {
                        KEY_SINK_STATE.with(|cell| {
                            if let Some(s) = cell.borrow().as_ref() {
                                s.counter.fetch_add(1, Ordering::Relaxed);
                            }
                        });
                    }
                }
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Spawns lightweight on-demand background workers for WASAPI Loopback FFT and
/// Privacy-First Anonymous Keyboard Rhythm. Both idle at 0% CPU when unused.
pub fn spawn_reactive_sensors(stop: Arc<AtomicBool>) -> Arc<ReactiveSensorsHandle> {
    let audio_enabled = Arc::new(AtomicBool::new(false));
    let keyboard_enabled = Arc::new(AtomicBool::new(false));
    let audio_bands = Arc::new(Mutex::new(AudioBands::default()));
    let key_press_counter = Arc::new(AtomicU32::new(0));

    let handle = Arc::new(ReactiveSensorsHandle {
        audio_enabled: Arc::clone(&audio_enabled),
        keyboard_enabled: Arc::clone(&keyboard_enabled),
        audio_bands: Arc::clone(&audio_bands),
        key_press_counter: Arc::clone(&key_press_counter),
        key_state: Mutex::new((0.0, 0.0, Instant::now())),
    });

    // Worker 1: On-demand WASAPI Loopback FFT
    let stop_audio = Arc::clone(&stop);
    let en_audio = Arc::clone(&audio_enabled);
    let bands_ref = Arc::clone(&audio_bands);
    thread::Builder::new()
        .name("prism-wasapi-fft".to_string())
        .spawn(move || {
            let _com = match ComApartmentGuard::init_mta() {
                Ok(g) => g,
                Err(_) => return,
            };
            while !stop_audio.load(Ordering::Relaxed) {
                if !en_audio.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(250));
                    continue;
                }
                unsafe {
                    run_wasapi_loopback_session(&stop_audio, &en_audio, &bands_ref);
                }
                if !stop_audio.load(Ordering::Relaxed) && en_audio.load(Ordering::Relaxed) {
                    // Brief backoff before reconnecting if audio device changed
                    thread::sleep(Duration::from_millis(400));
                }
            }
        })
        .ok();

    // Worker 2: Privacy-First Anonymous Raw Input Keyboard Rhythm (zero keylogger)
    let stop_kbd = Arc::clone(&stop);
    let en_kbd = Arc::clone(&keyboard_enabled);
    let cnt_kbd = Arc::clone(&key_press_counter);
    thread::Builder::new()
        .name("prism-kbd-rhythm".to_string())
        .spawn(move || unsafe {
            let class_name = to_wide("PrismViewAnonymousKbdSink");
            let hinst = HINSTANCE(GetModuleHandleW(std::ptr::null()));
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(keyboard_raw_wnd_proc),
                hInstance: hinst,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);

            KEY_SINK_STATE.with(|cell| {
                *cell.borrow_mut() = Some(KeySinkState {
                    stop: Arc::clone(&stop_kbd),
                    enabled: Arc::clone(&en_kbd),
                    counter: Arc::clone(&cnt_kbd),
                });
            });

            if let Ok(hwnd) = CreateWindowExW(
                WS_EX_NOACTIVATE,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                hinst,
                None,
            ) {
                if !hwnd.is_invalid() {
                    let rid = RAWINPUTDEVICE {
                        usUsagePage: 0x01, // HID Generic Desktop Controls
                        usUsage: 0x06,     // Keyboard
                        dwFlags: RIDEV_INPUTSINK,
                        hwndTarget: hwnd,
                    };
                    let _ = RegisterRawInputDevices(
                        &[rid],
                        std::mem::size_of::<RAWINPUTDEVICE>() as u32,
                    );
                    let _ = SetTimer(hwnd, KEY_SINK_TIMER_ID, 500, None);
                    let mut msg = MSG::default();
                    loop {
                        let ret = GetMessageW(&mut msg, HWND::default(), 0, 0).0;
                        if ret <= 0 {
                            break;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }

            KEY_SINK_STATE.with(|cell| {
                *cell.borrow_mut() = None;
            });
            let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinst);
        })
        .ok();

    handle
}
