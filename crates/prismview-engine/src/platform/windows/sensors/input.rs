//! Entrada global por Raw Input (`WM_INPUT` + `RIDEV_INPUTSINK`) numa janela message-only em
//! thread própria. Sem `SetWindowsHookEx` (hooks LL atrasam a entrada do sistema inteiro).
//!
//! - Mouse: deltas relativos (mickeys), roda, botões/cliques e posição global do cursor
//!   (`GetCursorPos`, no máx. a cada `CURSOR_POLL`). Mouse absoluto (RDP/tablet) gera delta pela
//!   variação do cursor.
//! - Teclado (privacy-first, como no Estúdio): só conta key-downs. O código da tecla é usado
//!   apenas localmente para descartar auto-repeat e nunca sai desta thread nem vai ao bus.
//! - Demanda: os dispositivos só ficam registrados enquanto a cena pede MOUSE/KEYBOARD
//!   (`RIDEV_REMOVE` ao sair) → sem demanda o Windows não entrega nada e a thread dorme em
//!   `GetMessageW`.
//!
//! O buffer do `GetRawInputData` é decodificado à mão (offsets do SDK) para não depender do
//! formato dos tipos `RAWMOUSE`/`RAWKEYBOARD` entre versões do crate `windows`.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::windows::core::{w, PCWSTR};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICE_FLAGS,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage, UnregisterClassW,
    HMENU, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_DESTROY, WM_INPUT,
    WNDCLASSEXW,
};

use super::super::util::valid_hwnd;
use super::Shared;
use crate::engine_core::signals::{buttons, SignalKind};

const CLASS_NAME: PCWSTR = w!("PrismEngineRawInputSink");
/// Demanda mudou: re-sincronizar registros.
pub(super) const WM_INPUT_DEMAND: u32 = WM_APP + 0x51;

const CURSOR_POLL: Duration = Duration::from_millis(4);

const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_MOUSE: u16 = 0x02;
const HID_USAGE_KEYBOARD: u16 = 0x06;

const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;
const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
const RI_KEY_BREAK: u16 = 0x01;
const VK_FAKE: u16 = 0xFF;

// usButtonFlags
const RI_LEFT_DOWN: u16 = 0x0001;
const RI_LEFT_UP: u16 = 0x0002;
const RI_RIGHT_DOWN: u16 = 0x0004;
const RI_RIGHT_UP: u16 = 0x0008;
const RI_MIDDLE_DOWN: u16 = 0x0010;
const RI_MIDDLE_UP: u16 = 0x0020;
const RI_X1_DOWN: u16 = 0x0040;
const RI_X1_UP: u16 = 0x0080;
const RI_X2_DOWN: u16 = 0x0100;
const RI_X2_UP: u16 = 0x0200;
const RI_WHEEL: u16 = 0x0400;

/// `RAWINPUTHEADER` = dwType, dwSize, hDevice, wParam.
const HEADER_SIZE: usize = 8 + 2 * std::mem::size_of::<usize>();
/// Cabeçalho + maior payload usado (RAWMOUSE = 24 B, RAWKEYBOARD = 16 B), alinhado a 8.
const RAW_BUF_WORDS: usize = 8;

/// Estado da thread (vive na pilha de `run`; o wndproc acessa por ponteiro thread-local).
/// Só `Cell`: o wndproc pode ser reentrado, então nada de `RefCell`/`&mut`.
struct InputState {
    shared: Arc<Shared>,
    registered: Cell<SignalKind>,
    keys_down: Cell<[u64; 4]>,
    last_cursor_poll: Cell<Option<Instant>>,
    last_cursor: Cell<(i32, i32)>,
}

std::thread_local! {
    static STATE: Cell<*const InputState> = const { Cell::new(std::ptr::null()) };
}

pub(super) fn run(shared: Arc<Shared>, hwnd_out: Arc<AtomicIsize>) {
    unsafe {
        let Ok(module) = GetModuleHandleW(PCWSTR::null()) else {
            return;
        };
        let hinstance: HINSTANCE = module.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // Falha = classe já registrada (outra instância na mesma execução): segue.
        let _ = RegisterClassExW(&wc);

        let state = InputState {
            shared,
            registered: Cell::new(SignalKind::NONE),
            keys_down: Cell::new([0; 4]),
            last_cursor_poll: Cell::new(None),
            last_cursor: Cell::new((0, 0)),
        };
        STATE.with(|s| s.set(&state as *const InputState));

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            CLASS_NAME,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            HMENU::default(),
            hinstance,
            None,
        );
        if let Some(hwnd) = valid_hwnd(hwnd) {
            // Publica o HWND antes de ler a demanda: o listener nunca perde uma mudança.
            hwnd_out.store(hwnd.0 as isize, Ordering::Release);
            if state.shared.stop.load(Ordering::Acquire) {
                let _ = DestroyWindow(hwnd);
            } else {
                sync_registration(hwnd, &state);
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, HWND::default(), 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            hwnd_out.store(0, Ordering::Release);
        }

        STATE.with(|s| s.set(std::ptr::null()));
        let _ = UnregisterClassW(CLASS_NAME, hinstance);
    }
}

/// Pede à thread de entrada para re-sincronizar os registros (chamado pelo listener de demanda).
pub(super) fn notify_demand(hwnd: isize) {
    let hwnd = HWND(hwnd as *mut c_void);
    if !hwnd.is_invalid() {
        let _ = unsafe { PostMessageW(hwnd, WM_INPUT_DEMAND, WPARAM(0), LPARAM(0)) };
    }
}

pub(super) fn request_close(hwnd: isize) {
    let hwnd = HWND(hwnd as *mut c_void);
    if !hwnd.is_invalid() {
        let _ = unsafe { PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

fn device(usage: u16, flags: RAWINPUTDEVICE_FLAGS, hwnd: HWND) -> RAWINPUTDEVICE {
    RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: usage,
        dwFlags: flags,
        hwndTarget: hwnd,
    }
}

/// Registra/remove mouse e teclado conforme a demanda atual.
unsafe fn sync_registration(hwnd: HWND, state: &InputState) {
    let wanted = state.shared.bus.demand() & SignalKind::INPUT;
    let current = state.registered.get();
    let mut now = current;
    let size = std::mem::size_of::<RAWINPUTDEVICE>() as u32;

    for (kind, usage) in [
        (SignalKind::MOUSE, HID_USAGE_MOUSE),
        (SignalKind::KEYBOARD, HID_USAGE_KEYBOARD),
    ] {
        let want = wanted.contains(kind);
        let have = current.contains(kind);
        if want && !have {
            if RegisterRawInputDevices(&[device(usage, RIDEV_INPUTSINK, hwnd)], size).is_ok() {
                now |= kind;
            }
        } else if !want && have {
            // RIDEV_REMOVE exige hwndTarget nulo.
            let _ = RegisterRawInputDevices(&[device(usage, RIDEV_REMOVE, HWND::default())], size);
            now = SignalKind(now.0 & !kind.0);
            if kind == SignalKind::KEYBOARD {
                state.keys_down.set([0; 4]);
            }
        }
    }
    if now.contains(SignalKind::MOUSE) && !current.contains(SignalKind::MOUSE) {
        poll_cursor(state, Instant::now(), true);
    }
    state.registered.set(now);
}

unsafe fn unregister_all(state: &InputState) {
    let size = std::mem::size_of::<RAWINPUTDEVICE>() as u32;
    let current = state.registered.get();
    if current.contains(SignalKind::MOUSE) {
        let _ = RegisterRawInputDevices(
            &[device(HID_USAGE_MOUSE, RIDEV_REMOVE, HWND::default())],
            size,
        );
    }
    if current.contains(SignalKind::KEYBOARD) {
        let _ = RegisterRawInputDevices(
            &[device(HID_USAGE_KEYBOARD, RIDEV_REMOVE, HWND::default())],
            size,
        );
    }
    state.registered.set(SignalKind::NONE);
}

/// Lê o cursor no máximo a cada `CURSOR_POLL`. Retorna a variação desde a última leitura.
fn poll_cursor(state: &InputState, now: Instant, force: bool) -> Option<(i32, i32)> {
    if !force {
        if let Some(last) = state.last_cursor_poll.get() {
            if now.duration_since(last) < CURSOR_POLL {
                return None;
            }
        }
    }
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    state.last_cursor_poll.set(Some(now));
    let (px, py) = state.last_cursor.get();
    state.last_cursor.set((pt.x, pt.y));
    state.shared.bus.set_cursor(pt.x, pt.y);
    Some((pt.x - px, pt.y - py))
}

#[inline]
fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_ne_bytes([b[o], b[o + 1]])
}
#[inline]
fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_ne_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn rd_i32(b: &[u8], o: usize) -> i32 {
    rd_u32(b, o) as i32
}

fn handle_mouse(state: &InputState, raw: &[u8]) {
    let bus = &state.shared.bus;
    let m = HEADER_SIZE;
    let flags = rd_u16(raw, m);
    let button_flags = rd_u16(raw, m + 4);
    let button_data = rd_u16(raw, m + 6) as i16;
    let dx = rd_i32(raw, m + 12);
    let dy = rd_i32(raw, m + 16);

    let now = Instant::now();
    let cursor_delta = poll_cursor(state, now, false);
    if flags & MOUSE_MOVE_ABSOLUTE != 0 {
        if let Some((cx, cy)) = cursor_delta {
            bus.add_mouse_delta(cx, cy);
        }
    } else {
        bus.add_mouse_delta(dx, dy);
    }

    if button_flags != 0 {
        let pairs = [
            (RI_LEFT_DOWN, RI_LEFT_UP, buttons::LEFT),
            (RI_RIGHT_DOWN, RI_RIGHT_UP, buttons::RIGHT),
            (RI_MIDDLE_DOWN, RI_MIDDLE_UP, buttons::MIDDLE),
            (RI_X1_DOWN, RI_X1_UP, buttons::X1),
            (RI_X2_DOWN, RI_X2_UP, buttons::X2),
        ];
        let (mut down, mut up) = (0u32, 0u32);
        for (d, u, bit) in pairs {
            if button_flags & d != 0 {
                down |= bit;
            }
            if button_flags & u != 0 {
                up |= bit;
            }
        }
        if down | up != 0 {
            bus.update_buttons(down, up);
            // Clique: força cursor exato no instante do clique.
            poll_cursor(state, now, true);
        }
        if button_flags & RI_WHEEL != 0 {
            bus.add_wheel(button_data as i32);
        }
    }
    bus.commit_input(SignalKind::MOUSE);
}

fn handle_keyboard(state: &InputState, raw: &[u8]) {
    let k = HEADER_SIZE;
    let flags = rd_u16(raw, k + 2);
    let vkey = rd_u16(raw, k + 6);
    if vkey == VK_FAKE || vkey >= 256 {
        return;
    }
    let mut keys = state.keys_down.get();
    let (word, bit) = ((vkey / 64) as usize, 1u64 << (vkey % 64));
    if flags & RI_KEY_BREAK != 0 {
        keys[word] &= !bit;
        state.keys_down.set(keys);
        return;
    }
    if keys[word] & bit != 0 {
        return; // auto-repeat
    }
    keys[word] |= bit;
    state.keys_down.set(keys);
    state.shared.bus.add_key_press();
    state.shared.bus.commit_input(SignalKind::KEYBOARD);
}

unsafe fn handle_raw_input(state: &InputState, lparam: LPARAM) {
    let mut buf = [0u64; RAW_BUF_WORDS];
    let mut size = std::mem::size_of_val(&buf) as u32;
    let read = GetRawInputData(
        HRAWINPUT(lparam.0 as *mut c_void),
        RID_INPUT,
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut size,
        HEADER_SIZE as u32,
    );
    if read == u32::MAX || (read as usize) < HEADER_SIZE {
        return;
    }
    let len = (read as usize).min(std::mem::size_of_val(&buf));
    let raw = std::slice::from_raw_parts(buf.as_ptr() as *const u8, len);
    let kind = rd_u32(raw, 0);
    let registered = state.registered.get();
    if kind == RIM_TYPEMOUSE && registered.contains(SignalKind::MOUSE) && len >= HEADER_SIZE + 20 {
        handle_mouse(state, raw);
    } else if kind == RIM_TYPEKEYBOARD
        && registered.contains(SignalKind::KEYBOARD)
        && len >= HEADER_SIZE + 8
    {
        handle_keyboard(state, raw);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_ptr = STATE.with(|s| s.get());
    // SAFETY: o ponteiro aponta para o `InputState` na pilha de `run`, válido enquanto o loop de
    // mensagens desta thread roda; é zerado antes do estado sair de escopo.
    let state = state_ptr.as_ref();
    match msg {
        WM_INPUT => {
            if let Some(state) = state {
                handle_raw_input(state, lparam);
            }
            // WM_INPUT com RIM_INPUTSINK ainda exige DefWindowProc para liberar o buffer.
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_INPUT_DEMAND => {
            if let Some(state) = state {
                sync_registration(hwnd, state);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(state) = state {
                unregister_all(state);
            }
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
