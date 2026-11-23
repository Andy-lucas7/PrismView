use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::core::{Interface, PCWSTR};
use windows::Foundation::{AsyncOperationCompletedHandler, EventRegistrationToken, IAsyncOperation, TypedEventHandler};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_NOT_PRESENT, QUNS_PRESENTATION_MODE,
    QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed, KillTimer,
    PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetTimer, SetWindowLongW,
    SetWindowPos, TranslateMessage, UnregisterClassW, GWL_EXSTYLE, HWND_BOTTOM,
    MONITORINFOF_PRIMARY, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WINDOW_STYLE,
    WM_CLOSE, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_QUERYENDSESSION, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonitorInfoDto {
    pub id: String,
    pub device_name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub dpi_x: u32,
    pub dpi_y: u32,
    pub scale_factor: f32,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LiveMediaDto {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    #[serde(default)]
    pub cover_path: String,
    #[serde(default)]
    pub position_secs: f64,
    #[serde(default)]
    pub duration_secs: f64,
    #[serde(default)]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Prev,
}

pub struct GsmtcHandle {
    pub media_cache: Arc<Mutex<LiveMediaDto>>,
    signal: Arc<(Mutex<GsmtcSignal>, Condvar)>,
}

impl GsmtcHandle {
    pub fn send_command(&self, cmd: MediaCommand) {
        let (lock, cvar) = &*self.signal;
        if let Ok(mut st) = lock.lock() {
            st.commands.push_back(cmd);
            st.dirty = true;
            cvar.notify_one();
        }
    }

    pub fn wake(&self) {
        let (lock, cvar) = &*self.signal;
        if let Ok(mut st) = lock.lock() {
            st.dirty = true;
            cvar.notify_all();
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn enable_per_monitor_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

unsafe fn hwnd_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let len = GetClassNameW(hwnd, &mut buf) as usize;
    String::from_utf16_lossy(&buf[..len])
}

pub fn virtual_screen_bounds() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1280),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(720),
        )
    }
}

fn sanitize_monitor_id(device_name: &str) -> String {
    let stripped = device_name
        .trim()
        .trim_start_matches(r"\\.\")
        .trim_start_matches(r"\\?\");
    let mut out = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "DISPLAY1".to_string()
    } else {
        trimmed.to_string()
    }
}

unsafe fn monitor_device_name(hmon: HMONITOR) -> Option<(String, RECT, bool)> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = GetMonitorInfoW(
        hmon,
        &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
    )
    .as_bool();
    if !ok {
        return None;
    }
    let end = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let device_name = String::from_utf16_lossy(&info.szDevice[..end]);
    let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
    Some((device_name, info.monitorInfo.rcMonitor, is_primary))
}

unsafe extern "system" fn enum_monitor_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<MonitorInfoDto>);
    if let Some((device_name, rc, is_primary)) = monitor_device_name(hmon) {
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        if dpi_x == 0 {
            dpi_x = 96;
        }
        if dpi_y == 0 {
            dpi_y = 96;
        }
        let width = (rc.right - rc.left).max(1);
        let height = (rc.bottom - rc.top).max(1);
        let scale_factor = (dpi_x as f32 / 96.0).clamp(0.5, 4.0);
        let id = sanitize_monitor_id(&device_name);
        list.push(MonitorInfoDto {
            id,
            device_name,
            x: rc.left,
            y: rc.top,
            width,
            height,
            dpi_x,
            dpi_y,
            scale_factor,
            is_primary,
        });
    }
    BOOL(1)
}

/// Enumerates all active physical displays with real bounds (supporting negative coordinates)
/// and per-monitor effective DPI / scale factor.
pub fn enumerate_monitors() -> Vec<MonitorInfoDto> {
    let mut monitors: Vec<MonitorInfoDto> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_monitor_proc),
            LPARAM(&mut monitors as *mut Vec<MonitorInfoDto> as isize),
        );
    }

    if monitors.is_empty() {
        let (x, y, w, h) = virtual_screen_bounds();
        monitors.push(MonitorInfoDto {
            id: "DISPLAY1".to_string(),
            device_name: r"\\.\DISPLAY1".to_string(),
            x,
            y,
            width: w,
            height: h,
            dpi_x: 96,
            dpi_y: 96,
            scale_factor: 1.0,
            is_primary: true,
        });
    }

    // Deduplicate IDs if a driver reports duplicate names
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, m) in monitors.iter_mut().enumerate() {
        if !seen.insert(m.id.clone()) {
            let unique = format!("{}_{}", m.id, idx + 1);
            seen.insert(unique.clone());
            m.id = unique;
        }
    }

    monitors.sort_by(|a, b| {
        b.is_primary
            .cmp(&a.is_primary)
            .then_with(|| a.x.cmp(&b.x))
            .then_with(|| a.y.cmp(&b.y))
            .then_with(|| a.id.cmp(&b.id))
    });
    monitors
}

/// Returns whether a cached WorkerW HWND is still alive (detects explorer.exe restarts).
pub fn is_window_alive(hwnd_raw: isize) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe { IsWindow(HWND(hwnd_raw as *mut core::ffi::c_void)).as_bool() }
}

pub fn configure_desktop_widget_window(hwnd_raw: isize, locked: bool) {
    let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        let cur_ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut next_ex = cur_ex | WS_EX_TOOLWINDOW.0;
        if locked {
            next_ex |= WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0;
        } else {
            next_ex &= !(WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0);
        }
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, next_ex as i32);
        let _ = SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmGetWindowAttribute(
        hwnd: HWND,
        dwattribute: u32,
        pvattribute: *mut core::ffi::c_void,
        cbattribute: u32,
    ) -> i32;
}

const DWMWA_CLOAKED: u32 = 14;

unsafe fn is_window_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut core::ffi::c_void,
        std::mem::size_of::<u32>() as u32,
    );
    hr == 0 && cloaked != 0
}

fn is_shell_class(cls: &str) -> bool {
    matches!(
        cls,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "Shell_InputSwitchTopLevelWindow"
    )
}

struct OcclusionScan<'a> {
    own_pid: u32,
    studio_hwnd: Option<isize>,
    bounds_to_id: &'a HashMap<(i32, i32, i32, i32), String>,
    device_to_id: &'a HashMap<String, String>,
    occluded: HashSet<String>,
}

unsafe extern "system" fn enum_occlusion_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = &mut *(lparam.0 as *mut OcclusionScan<'_>);
    if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return BOOL(1);
    }
    if let Some(sh) = scan.studio_hwnd {
        if hwnd.0 as isize == sh {
            return BOOL(1);
        }
    }
    let mut pid: u32 = 0;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == scan.own_pid {
        return BOOL(1);
    }
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    // Skip tool windows and click-through transparent HUD overlays (e.g. RTSS/Discord/GameBar overlays)
    if (ex_style & (WS_EX_TOOLWINDOW.0 | WS_EX_TRANSPARENT.0)) != 0 {
        return BOOL(1);
    }
    if is_window_cloaked(hwnd) {
        return BOOL(1);
    }
    let cls = hwnd_class_name(hwnd);
    if is_shell_class(&cls) {
        return BOOL(1);
    }

    let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let Some((dev_name, mr, _)) = monitor_device_name(hmon) else {
        return BOOL(1);
    };
    let bounds_key = (mr.left, mr.top, (mr.right - mr.left).max(1), (mr.bottom - mr.top).max(1));
    let Some(mon_id) = scan
        .bounds_to_id
        .get(&bounds_key)
        .or_else(|| scan.device_to_id.get(&dev_name))
    else {
        return BOOL(1);
    };
    if scan.occluded.contains(mon_id) {
        return BOOL(1);
    }

    let mut is_covering = IsZoomed(hwnd).as_bool();
    if !is_covering {
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_ok() {
            let covers_full = wr.left <= mr.left + 2
                && wr.top <= mr.top + 2
                && wr.right >= mr.right - 2
                && wr.bottom >= mr.bottom - 2;
            if covers_full {
                is_covering = true;
            }
        }
    }

    if is_covering {
        scan.occluded.insert(mon_id.clone());
    }
    BOOL(1)
}

/// Computes per-monitor desktop visibility (`monitor_id -> visible`).
/// Only monitors actually covered by a maximized/fullscreen app are paused;
/// uncovered monitors remain fluid.
pub fn compute_monitors_visibility(
    monitors: &[MonitorInfoDto],
    studio_hwnd_raw: Option<isize>,
) -> HashMap<String, bool> {
    let mut result: HashMap<String, bool> =
        monitors.iter().map(|m| (m.id.clone(), true)).collect();
    if monitors.is_empty() {
        return result;
    }

    let bounds_to_id: HashMap<(i32, i32, i32, i32), String> = monitors
        .iter()
        .map(|m| ((m.x, m.y, m.width, m.height), m.id.clone()))
        .collect();
    let device_to_id: HashMap<String, String> = monitors
        .iter()
        .map(|m| (m.device_name.clone(), m.id.clone()))
        .collect();
    let own_pid = std::process::id();

    unsafe {
        if let Ok(quns) = SHQueryUserNotificationState() {
            if quns == QUNS_NOT_PRESENT {
                for v in result.values_mut() {
                    *v = false;
                }
                return result;
            }
            if quns == QUNS_RUNNING_D3D_FULL_SCREEN
                || quns == QUNS_BUSY
                || quns == QUNS_PRESENTATION_MODE
            {
                let fg = GetForegroundWindow();
                let mut matched_one = false;
                if !fg.is_invalid() && IsWindowVisible(fg).as_bool() {
                    let mut fg_pid: u32 = 0;
                    let _ = GetWindowThreadProcessId(fg, Some(&mut fg_pid));
                    let is_own = fg_pid == own_pid
                        || studio_hwnd_raw.map(|s| fg.0 as isize == s).unwrap_or(false);
                    let cls = hwnd_class_name(fg);
                    if !is_own && !is_shell_class(&cls) {
                        let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
                        if let Some((dev, mr, _)) = monitor_device_name(hmon) {
                            let bkey = (
                                mr.left,
                                mr.top,
                                (mr.right - mr.left).max(1),
                                (mr.bottom - mr.top).max(1),
                            );
                            if let Some(mid) =
                                bounds_to_id.get(&bkey).or_else(|| device_to_id.get(&dev))
                            {
                                result.insert(mid.clone(), false);
                                matched_one = true;
                            }
                        }
                    }
                }
                if !matched_one && quns == QUNS_RUNNING_D3D_FULL_SCREEN {
                    for v in result.values_mut() {
                        *v = false;
                    }
                    return result;
                }
            }
        }

        let mut scan = OcclusionScan {
            own_pid,
            studio_hwnd: studio_hwnd_raw,
            bounds_to_id: &bounds_to_id,
            device_to_id: &device_to_id,
            occluded: HashSet::new(),
        };
        let _ = EnumWindows(
            Some(enum_occlusion_proc),
            LPARAM(&mut scan as *mut OcclusionScan<'_> as isize),
        );
        for mid in scan.occluded {
            result.insert(mid, false);
        }
    }

    result
}

pub fn monitor_id_for_point(monitors: &[MonitorInfoDto], x: i32, y: i32) -> Option<String> {
    for m in monitors {
        if x >= m.x && x < m.x + m.width && y >= m.y && y < m.y + m.height {
            return Some(m.id.clone());
        }
    }
    monitors.first().map(|m| m.id.clone())
}

pub fn poll_cursor_raw() -> Option<(i32, i32)> {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_ok() {
            return Some((pt.x, pt.y));
        }
    }
    None
}

pub fn normalize_cursor_for_monitor(px: i32, py: i32, mon: &MonitorInfoDto) -> [f32; 2] {
    let nx = ((px - mon.x) as f32 / mon.width.max(1) as f32).clamp(0.0, 1.0);
    let ny = ((py - mon.y) as f32 / mon.height.max(1) as f32).clamp(0.0, 1.0);
    [nx, ny]
}

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
    fn WindowFromPoint(point: POINT) -> HWND;
    fn GetAncestor(hwnd: HWND, ga_flags: u32) -> HWND;
}

const VK_LBUTTON: i32 = 0x01;
const GA_ROOT: u32 = 2;

/// Non-blocking check for left mouse button state (zero global hooks, zero input lag).
pub fn is_left_button_down() -> bool {
    unsafe { (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0 }
}

/// Returns true only when the screen point directly hits the Windows desktop shell
/// (Progman / WorkerW / SHELLDLL_DefView / SysListView32), preventing clicks inside
/// foreground apps or games from firing wallpaper 3D raycast events.
pub fn is_point_on_desktop(px: i32, py: i32) -> bool {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x: px, y: py });
        if hwnd.is_invalid() {
            return false;
        }
        let cls = hwnd_class_name(hwnd);
        if matches!(
            cls.as_str(),
            "Progman" | "WorkerW" | "SysListView32" | "SHELLDLL_DefView" | "FolderView"
        ) {
            return true;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        if !root.is_invalid() {
            let root_cls = hwnd_class_name(root);
            return matches!(root_cls.as_str(), "Progman" | "WorkerW");
        }
        false
    }
}

struct RecoveryHookState {
    wm_taskbar_created: u32,
    stop: Arc<AtomicBool>,
    callback: Box<dyn Fn(&'static str)>,
}

thread_local! {
    static RECOVERY_HOOK: RefCell<Option<RecoveryHookState>> = const { RefCell::new(None) };
}

const RECOVERY_TIMER_ID: usize = 0x5052;
/// Checagem do `stop` na thread de recuperação (a saída do app encerra o processo de qualquer jeito).
const RECOVERY_STOP_POLL_MS: u32 = 30_000;

unsafe extern "system" fn recovery_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == RECOVERY_TIMER_ID => {
            let should_quit = RECOVERY_HOOK.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|s| s.stop.load(Ordering::Relaxed))
                    .unwrap_or(true)
            });
            if should_quit {
                let _ = KillTimer(hwnd, RECOVERY_TIMER_ID);
                let _ = DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE => {
            RECOVERY_HOOK.with(|cell| {
                if let Some(ref s) = *cell.borrow() {
                    (s.callback)("display_change");
                }
            });
            return LRESULT(0);
        }
        // Logoff/desligamento: avisa antes das janelas morrerem (widgets continuam fixados).
        // Se outro app vetar o logoff, WM_ENDSESSION chega com wparam = FALSE e desfazemos.
        WM_QUERYENDSESSION => {
            RECOVERY_HOOK.with(|cell| {
                if let Some(ref s) = *cell.borrow() {
                    (s.callback)("end_session");
                }
            });
            return LRESULT(1);
        }
        WM_ENDSESSION => {
            if wparam.0 == 0 {
                RECOVERY_HOOK.with(|cell| {
                    if let Some(ref s) = *cell.borrow() {
                        (s.callback)("end_session_cancelled");
                    }
                });
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            let _ = KillTimer(hwnd, RECOVERY_TIMER_ID);
            let _ = DestroyWindow(hwnd);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {
            let matched_taskbar = RECOVERY_HOOK.with(|cell| {
                if let Some(ref s) = *cell.borrow() {
                    if s.wm_taskbar_created != 0 && msg == s.wm_taskbar_created {
                        (s.callback)("taskbar_created");
                        return true;
                    }
                }
                false
            });
            if matched_taskbar {
                return LRESULT(0);
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Spawns a dedicated hidden Win32 event-sink window (non-visible top-level tool window so
/// Windows delivers `HWND_BROADCAST` messages `WM_DISPLAYCHANGE` and `TaskbarCreated` —
/// `HWND_MESSAGE` windows do not receive broadcast messages per Win32 spec).
pub fn spawn_desktop_recovery_listener<F>(stop: Arc<AtomicBool>, on_recover: F)
where
    F: Fn(&'static str) + Send + 'static,
{
    thread::Builder::new()
        .name("prism-win32-recovery".to_string())
        .spawn(move || unsafe {
            let class_name = to_wide("PrismViewDesktopRecoverySink");
            let taskbar_msg_name = to_wide("TaskbarCreated");
            let wm_taskbar_created = RegisterWindowMessageW(PCWSTR(taskbar_msg_name.as_ptr()));
            let hinst_raw = GetModuleHandleW(std::ptr::null());
            let hinst = HINSTANCE(hinst_raw);

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(recovery_wnd_proc),
                hInstance: hinst,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);

            RECOVERY_HOOK.with(|cell| {
                *cell.borrow_mut() = Some(RecoveryHookState {
                    wm_taskbar_created,
                    stop: Arc::clone(&stop),
                    callback: Box::new(on_recover),
                });
            });

            if let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND::default(),
                None,
                hinst,
                None,
            ) {
                if !hwnd.is_invalid() {
                    // Hooks de visibilidade vivem nesta thread (tem laço de mensagens).
                    let _vis_hooks = crate::vis_events::VisibilityHooks::install();
                    // Só confere o `stop` (saída); eventos chegam por mensagem, não por este timer.
                    let _ = SetTimer(hwnd, RECOVERY_TIMER_ID, RECOVERY_STOP_POLL_MS, None);
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

            RECOVERY_HOOK.with(|cell| {
                *cell.borrow_mut() = None;
            });
            let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), hinst);
        })
        .ok();
}

const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_MULTITHREADED: u32 = 0x0;
const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const RPC_E_CHANGED_MODE: i32 = -2147417850; // 0x80010106

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvreserved: *mut core::ffi::c_void, dwcoinit: u32) -> i32;
    fn CoUninitialize();
}

pub struct ComApartmentGuard {
    should_uninit: bool,
}

impl ComApartmentGuard {
    pub fn init_mta() -> Result<Self, i32> {
        let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED) };
        match hr {
            S_OK | S_FALSE => Ok(Self {
                should_uninit: true,
            }),
            RPC_E_CHANGED_MODE => Ok(Self {
                should_uninit: false,
            }),
            err => Err(err),
        }
    }

    pub fn init_sta() -> Result<Self, i32> {
        let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
        match hr {
            S_OK | S_FALSE => Ok(Self {
                should_uninit: true,
            }),
            RPC_E_CHANGED_MODE => Ok(Self {
                should_uninit: false,
            }),
            err => Err(err),
        }
    }
}

impl Drop for ComApartmentGuard {
    fn drop(&mut self) {
        if self.should_uninit {
            unsafe { CoUninitialize() };
        }
    }
}

#[derive(Default)]
struct GsmtcSignal {
    dirty: bool,
    session_changed: bool,
    commands: VecDeque<MediaCommand>,
}

fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn await_async_timeout<T>(op: IAsyncOperation<T>, timeout: Duration) -> Option<T>
where
    T: windows::core::RuntimeType + 'static,
{
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let done_cb = Arc::clone(&done);
    let handler = AsyncOperationCompletedHandler::new(move |_op, _status| {
        let (lock, cvar) = &*done_cb;
        if let Ok(mut flag) = lock.lock() {
            *flag = true;
            cvar.notify_one();
        }
        Ok(())
    });
    if op.SetCompleted(&handler).is_err() {
        return None;
    }
    let (lock, cvar) = &*done;
    let Ok(mut guard) = lock.lock() else {
        return None;
    };
    let deadline = Instant::now() + timeout;
    while !*guard {
        let rem = deadline.checked_duration_since(Instant::now())?;
        let res = cvar.wait_timeout(guard, rem);
        guard = match res {
            Ok((g, wait_res)) => {
                if wait_res.timed_out() && !*g {
                    return None;
                }
                g
            }
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
    op.GetResults().ok()
}

fn cleanup_stale_media_covers(cover_dir: &PathBuf) {
    if let Ok(entries) = fs::read_dir(cover_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("media_cover_") {
                    let _ = fs::remove_file(&p);
                }
            }
        }
    }
}

fn read_thumbnail_to_disk(
    thumb_ref: &IRandomAccessStreamReference,
    cover_dir: &PathBuf,
    counter: &mut u64,
    prev_cover_file: &mut Option<PathBuf>,
) -> String {
    let Ok(op) = thumb_ref.OpenReadAsync() else {
        return String::new();
    };
    let Some(stream) = await_async_timeout(op, Duration::from_millis(1500)) else {
        return String::new();
    };
    let Ok(size_u64) = stream.Size() else {
        return String::new();
    };
    if size_u64 == 0 || size_u64 > 6 * 1024 * 1024 {
        return String::new();
    }
    let total_size = size_u64 as usize;
    let Ok(input) = stream.GetInputStreamAt(0) else {
        return String::new();
    };
    let Ok(reader) = DataReader::CreateDataReader(&input) else {
        return String::new();
    };
    // Loop LoadAsync until the full thumbnail is read (prevents truncated covers)
    let mut bytes = Vec::with_capacity(total_size);
    while bytes.len() < total_size {
        let rem = (total_size - bytes.len()).min(u32::MAX as usize) as u32;
        let Ok(load_op) = reader.LoadAsync(rem) else {
            return String::new();
        };
        let Ok(async_op) = load_op.cast::<IAsyncOperation<u32>>() else {
            return String::new();
        };
        let Some(loaded) = await_async_timeout(async_op, Duration::from_millis(1500)) else {
            return String::new();
        };
        if loaded == 0 {
            break;
        }
        let mut chunk = vec![0u8; loaded as usize];
        if reader.ReadBytes(&mut chunk).is_err() {
            return String::new();
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return String::new();
    }
    let ext = if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "png"
    } else if bytes.starts_with(b"RIFF") {
        "webp"
    } else {
        "jpg"
    };
    let _ = fs::create_dir_all(cover_dir);
    *counter = counter.wrapping_add(1);
    let dest = cover_dir.join(format!("media_cover_{}.{ext}", *counter));
    if fs::write(&dest, &bytes).is_ok() {
        if let Some(old) = prev_cover_file.replace(dest.clone()) {
            let _ = fs::remove_file(old);
        }
        return dest.to_string_lossy().to_string();
    }
    String::new()
}

fn read_session_media(
    session: &GlobalSystemMediaTransportControlsSession,
    prev_dto: &LiveMediaDto,
    cover_dir: &PathBuf,
    last_track_key: &mut String,
    last_cover_path: &mut String,
    cover_counter: &mut u64,
    prev_cover_file: &mut Option<PathBuf>,
) -> LiveMediaDto {
    let playing = session
        .GetPlaybackInfo()
        .and_then(|pi| pi.PlaybackStatus())
        .map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(prev_dto.playing);

    let (position_secs, duration_secs) = session
        .GetTimelineProperties()
        .map(|tl| {
            let pos = tl
                .Position()
                .map(|t| (t.Duration as f64 / 10_000_000.0).max(0.0))
                .unwrap_or(0.0);
            let dur = tl
                .EndTime()
                .map(|t| (t.Duration as f64 / 10_000_000.0).max(0.0))
                .unwrap_or(0.0);
            (pos, dur)
        })
        .unwrap_or((prev_dto.position_secs, prev_dto.duration_secs));

    // Preserve previous track metadata if TryGetMediaPropertiesAsync transiently fails or times out
    let mut dto = LiveMediaDto {
        title: prev_dto.title.clone(),
        artist: prev_dto.artist.clone(),
        album: prev_dto.album.clone(),
        cover_path: prev_dto.cover_path.clone(),
        playing,
        position_secs,
        duration_secs,
        updated_at_ms: unix_now_ms(),
    };

    if let Ok(prop_op) = session.TryGetMediaPropertiesAsync() {
        if let Some(props) = await_async_timeout(prop_op, Duration::from_millis(1500)) {
            dto.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
            dto.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
            dto.album = props
                .AlbumTitle()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let track_key = format!("{}|{}|{}", dto.title, dto.artist, dto.album);
            // Retry cover extraction if track changed OR previous cover was missing (late-arriving thumbnails)
            if track_key != *last_track_key || last_cover_path.is_empty() {
                let track_changed = track_key != *last_track_key;
                *last_track_key = track_key;
                let fetched = props
                    .Thumbnail()
                    .ok()
                    .map(|t| read_thumbnail_to_disk(&t, cover_dir, cover_counter, prev_cover_file))
                    .unwrap_or_default();
                if !fetched.is_empty() {
                    *last_cover_path = fetched;
                } else if track_changed {
                    last_cover_path.clear();
                    if let Some(old) = prev_cover_file.take() {
                        let _ = fs::remove_file(old);
                    }
                }
            }
            dto.cover_path = last_cover_path.clone();
        }
    }
    dto
}

fn detach_session_events(
    session: &mut Option<GlobalSystemMediaTransportControlsSession>,
    playback_token: &mut Option<EventRegistrationToken>,
    props_token: &mut Option<EventRegistrationToken>,
    timeline_token: &mut Option<EventRegistrationToken>,
) {
    if let Some(s) = session.take() {
        if let Some(tok) = playback_token.take() {
            let _ = s.RemovePlaybackInfoChanged(tok);
        }
        if let Some(tok) = props_token.take() {
            let _ = s.RemoveMediaPropertiesChanged(tok);
        }
        if let Some(tok) = timeline_token.take() {
            let _ = s.RemoveTimelinePropertiesChanged(tok);
        }
    } else {
        *playback_token = None;
        *props_token = None;
        *timeline_token = None;
    }
}

/// Dedicated COM-MTA thread driving Windows GSMTC via native WinRT events
/// (CurrentSessionChanged, PlaybackInfoChanged, MediaPropertiesChanged, TimelinePropertiesChanged)
/// plus async transport commands (PlayPause, Next, Prev) and cached album art extraction.
pub fn spawn_gsmtc_worker(stop: Arc<AtomicBool>, cover_dir: PathBuf) -> Arc<GsmtcHandle> {
    let shared_media = Arc::new(Mutex::new(LiveMediaDto::default()));
    let signal = Arc::new((
        Mutex::new(GsmtcSignal {
            dirty: true,
            session_changed: true,
            commands: VecDeque::new(),
        }),
        Condvar::new(),
    ));

    let handle = Arc::new(GsmtcHandle {
        media_cache: Arc::clone(&shared_media),
        signal: Arc::clone(&signal),
    });

    thread::Builder::new()
        .name("prism-gsmtc-mta".to_string())
        .spawn(move || {
            let _com_guard = match ComApartmentGuard::init_mta() {
                Ok(g) => g,
                Err(_) => return,
            };

            cleanup_stale_media_covers(&cover_dir);

            let mgr = loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
                    .ok()
                    .and_then(|op| await_async_timeout(op, Duration::from_millis(2000)))
                {
                    Some(m) => break m,
                    None => {
                        for _ in 0..10 {
                            if stop.load(Ordering::Relaxed) {
                                return;
                            }
                            thread::sleep(Duration::from_millis(500));
                        }
                    }
                }
            };

            let sig_curr = Arc::clone(&signal);
            let current_token = mgr
                .CurrentSessionChanged(&TypedEventHandler::new(move |_sender, _args| {
                    let (lock, cvar) = &*sig_curr;
                    if let Ok(mut st) = lock.lock() {
                        st.session_changed = true;
                        st.dirty = true;
                        cvar.notify_one();
                    }
                    Ok(())
                }))
                .ok();

            let mut attached_session: Option<GlobalSystemMediaTransportControlsSession> = None;
            let mut attached_session_ptr: usize = 0;
            let mut playback_token: Option<EventRegistrationToken> = None;
            let mut props_token: Option<EventRegistrationToken> = None;
            let mut timeline_token: Option<EventRegistrationToken> = None;
            let mut last_track_key = String::new();
            let mut last_cover_path = String::new();
            let mut cover_counter: u64 = 0;
            let mut prev_cover_file: Option<PathBuf> = None;

            while !stop.load(Ordering::Relaxed) {
                let (need_session_rebind, need_refresh, pending_cmds) = {
                    let (lock, cvar) = &*signal;
                    let Ok(mut guard) = lock.lock() else {
                        break;
                    };
                    if !guard.dirty && !guard.session_changed && guard.commands.is_empty() {
                        let res = cvar.wait_timeout(guard, Duration::from_millis(500));
                        guard = match res {
                            Ok((g, _)) => g,
                            Err(poisoned) => poisoned.into_inner().0,
                        };
                    }
                    let sc = guard.session_changed;
                    let dr = guard.dirty;
                    let cmds: Vec<MediaCommand> = guard.commands.drain(..).collect();
                    guard.session_changed = false;
                    guard.dirty = false;
                    (sc, dr, cmds)
                };

                if stop.load(Ordering::Relaxed) {
                    break;
                }

                if need_session_rebind
                    || ((need_refresh || !pending_cmds.is_empty()) && attached_session.is_none())
                {
                    match mgr.GetCurrentSession() {
                        Ok(next_session) => {
                            // Compare actual COM object identity so switching tabs/players in the same browser rebinds cleanly
                            let next_ptr = next_session.as_raw() as usize;
                            let same =
                                attached_session.is_some() && attached_session_ptr == next_ptr;
                            if !same {
                                detach_session_events(
                                    &mut attached_session,
                                    &mut playback_token,
                                    &mut props_token,
                                    &mut timeline_token,
                                );
                                let sig_pb = Arc::clone(&signal);
                                playback_token = next_session
                                    .PlaybackInfoChanged(&TypedEventHandler::new(
                                        move |_sender, _args| {
                                            let (lock, cvar) = &*sig_pb;
                                            if let Ok(mut st) = lock.lock() {
                                                st.dirty = true;
                                                cvar.notify_one();
                                            }
                                            Ok(())
                                        },
                                    ))
                                    .ok();

                                let sig_mp = Arc::clone(&signal);
                                props_token = next_session
                                    .MediaPropertiesChanged(&TypedEventHandler::new(
                                        move |_sender, _args| {
                                            let (lock, cvar) = &*sig_mp;
                                            if let Ok(mut st) = lock.lock() {
                                                st.dirty = true;
                                                cvar.notify_one();
                                            }
                                            Ok(())
                                        },
                                    ))
                                    .ok();

                                let sig_tl = Arc::clone(&signal);
                                timeline_token = next_session
                                    .TimelinePropertiesChanged(&TypedEventHandler::new(
                                        move |_sender, _args| {
                                            let (lock, cvar) = &*sig_tl;
                                            if let Ok(mut st) = lock.lock() {
                                                st.dirty = true;
                                                cvar.notify_one();
                                            }
                                            Ok(())
                                        },
                                    ))
                                    .ok();

                                attached_session_ptr = next_ptr;
                                attached_session = Some(next_session);
                            }
                        }
                        Err(_) => {
                            detach_session_events(
                                &mut attached_session,
                                &mut playback_token,
                                &mut props_token,
                                &mut timeline_token,
                            );
                            attached_session_ptr = 0;
                        }
                    }
                }

                if !pending_cmds.is_empty() {
                    if let Some(ref session) = attached_session {
                        for cmd in pending_cmds {
                            match cmd {
                                MediaCommand::PlayPause => {
                                    if let Ok(op) = session.TryTogglePlayPauseAsync() {
                                        let _ = await_async_timeout(op, Duration::from_millis(1200));
                                    }
                                }
                                MediaCommand::Next => {
                                    if let Ok(op) = session.TrySkipNextAsync() {
                                        let _ = await_async_timeout(op, Duration::from_millis(1200));
                                    }
                                }
                                MediaCommand::Prev => {
                                    if let Ok(op) = session.TrySkipPreviousAsync() {
                                        let _ = await_async_timeout(op, Duration::from_millis(1200));
                                    }
                                }
                            }
                        }
                    }
                }

                if need_refresh || need_session_rebind {
                    let prev_snapshot = shared_media
                        .lock()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    let next_dto = match &attached_session {
                        Some(s) => read_session_media(
                            s,
                            &prev_snapshot,
                            &cover_dir,
                            &mut last_track_key,
                            &mut last_cover_path,
                            &mut cover_counter,
                            &mut prev_cover_file,
                        ),
                        None => {
                            last_track_key.clear();
                            last_cover_path.clear();
                            if let Some(old) = prev_cover_file.take() {
                                let _ = fs::remove_file(old);
                            }
                            LiveMediaDto::default()
                        }
                    };
                    if let Ok(mut guard) = shared_media.lock() {
                        *guard = next_dto;
                    }
                }
            }

            if let Some(old) = prev_cover_file.take() {
                let _ = fs::remove_file(old);
            }
            detach_session_events(
                &mut attached_session,
                &mut playback_token,
                &mut props_token,
                &mut timeline_token,
            );
            if let Some(tok) = current_token {
                let _ = mgr.RemoveCurrentSessionChanged(tok);
            }
            drop(mgr);
        })
        .ok();

    handle
}

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
#[derive(Default)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    dw_length: u32,
    dw_memory_load: u32,
    ull_total_phys: u64,
    ull_avail_phys: u64,
    ull_total_page_file: u64,
    ull_avail_page_file: u64,
    ull_total_virtual: u64,
    ull_avail_virtual: u64,
    ull_avail_extended_virtual: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lpmodulename: *const u16) -> *mut core::ffi::c_void;
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
}

static PREV_CPU: Mutex<Option<(u64, u64, f32, Instant)>> = Mutex::new(None);

/// Returns `(ram_percent, ram_used_gb, ram_total_gb)` via direct Win32 `GlobalMemoryStatusEx` (zero crate overhead).
pub fn sample_memory_usage() -> (f32, f32, f32) {
    unsafe {
        let mut ms = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            ..MemoryStatusEx::default()
        };
        if GlobalMemoryStatusEx(&mut ms) != 0 && ms.ull_total_phys > 0 {
            let total_gb = ms.ull_total_phys as f64 / (1024.0 * 1024.0 * 1024.0);
            let used_bytes = ms.ull_total_phys.saturating_sub(ms.ull_avail_phys);
            let used_gb = used_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            let pct = ((used_bytes as f64 / ms.ull_total_phys as f64) * 100.0).clamp(0.0, 100.0);
            return (
                (pct as f32 * 10.0).round() / 10.0,
                (used_gb as f32 * 10.0).round() / 10.0,
                (total_gb as f32 * 10.0).round() / 10.0,
            );
        }
    }
    (0.0, 0.0, 0.0)
}

pub fn sample_cpu_and_battery() -> (f32, u8, bool) {
    let mut cpu = 0.0f32;
    let mut bat = 100u8;
    let mut charging = true;
    unsafe {
        let mut idle = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        if GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0 {
            let idle_v = idle.as_u64();
            let total_v = kernel.as_u64().saturating_add(user.as_u64());
            let now = Instant::now();
            if let Ok(mut guard) = PREV_CPU.lock() {
                if let Some((pi, pt, last, last_at)) = *guard {
                    // Enforce minimum 250ms sample window so concurrent callers never corrupt CPU delta
                    if now.duration_since(last_at) < Duration::from_millis(250) {
                        cpu = last;
                    } else {
                        let di = idle_v.saturating_sub(pi);
                        let dt = total_v.saturating_sub(pt);
                        if dt > 0 {
                            let pct = ((dt.saturating_sub(di) as f64 / dt as f64) * 100.0)
                                .clamp(0.0, 100.0) as f32;
                            cpu = (pct * 10.0).round() / 10.0;
                            *guard = Some((idle_v, total_v, cpu, now));
                        } else {
                            cpu = last;
                        }
                    }
                } else {
                    *guard = Some((idle_v, total_v, 0.0, now));
                }
            }
        }
        let mut ps = SystemPowerStatus::default();
        if GetSystemPowerStatus(&mut ps) != 0 {
            if ps.battery_life_percent <= 100 {
                bat = ps.battery_life_percent;
            }
            charging = ps.ac_line_status == 1;
        }
    }
    (cpu, bat, charging)
}
