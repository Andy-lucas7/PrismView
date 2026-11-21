#![windows_subsystem = "windows"]

// Camada de plataforma nova (Fase 0); o loop legado abaixo ainda não a usa.
// Só o modo `--native` (native.rs) usa `platform/` + `core/`.
// `core/` vive em disco com esse nome (PLANO_NATIVO), mas o módulo se chama `engine_core`:
// um `mod core` na raiz sombrearia a crate `core` em todos os `use super::*` dos submódulos.
#[path = "core/mod.rs"]
mod engine_core;
mod native;
#[allow(dead_code)]
mod platform;
mod video;

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont};
use bytemuck::{Pod, Zeroable};
use chrono::{Local, Timelike};
use prism_core::fx::{EvalContext, FxEngine, SystemTelemetry, WeatherTelemetry};
use prism_core::package::{import_package, SteamWorkshopMeta};
use prism_core::schema::{
    Anchor, BackgroundSource, DepthPostProcess, DirtyFlags, EngineStatus, FillMode, IpcRequest,
    IpcResponse, LayerKind, LayerNode, OpticalCrystalParams, PackageKind, ParallaxParams,
    PrismScene, ShapeKind, StaticOrFx, StemStretchParams, TextAlign, TextContent, WidgetPlacement,
    IPC_PIPE_NAME, MANIFEST_VERSION,
};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::num::NonZeroIsize;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use windows::core::PCWSTR;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_PIPE_CONNECTED, HANDLE, HINSTANCE, HWND,
    INVALID_HANDLE_VALUE, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, MonitorFromWindow, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NOTIFYICONDATAW, QUNS_ACCEPTS_NOTIFICATIONS, QUNS_BUSY, QUNS_NOT_PRESENT,
    QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindow, GetWindowLongW, GetWindowRect,
    IsWindowVisible, IsZoomed, KillTimer, LoadIconW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SendMessageTimeoutW, SetForegroundWindow, SetParent, SetTimer,
    SetWindowLongW, SetWindowPos, ShowWindow, TrackPopupMenu, TranslateMessage, GWL_STYLE,
    GW_HWNDNEXT, HWND_BOTTOM, HWND_TOP, IDI_APPLICATION, MF_STRING, MSG, SMTO_NORMAL,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOW, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, WM_APP, WM_COMMAND, WM_DESTROY, WM_DISPLAYCHANGE, WM_LBUTTONDBLCLK, WM_PAINT,
    WM_RBUTTONUP, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE, WTS_SESSION_LOCK,
    WTS_SESSION_UNLOCK,
};

const WM_PRISM_WAKE: u32 = WM_APP + 1;
const WM_PRISM_TRAY: u32 = WM_APP + 2;
const WM_PRISM_VIDEO_FRAME: u32 = WM_APP + 3;
const TRAY_ICON_ID: u32 = 1;
const CMD_OPEN_STUDIO: usize = 1001;
const CMD_TOGGLE_PAUSE: usize = 1002;
const CMD_EXIT: usize = 1003;
const TIMER_TICK_ID: usize = 1;
const TIMER_VISIBILITY_ID: usize = 2;

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

mod win32_host {
    use super::*;

    struct WorkerSearch {
        defview_parent: HWND,
        worker_w: HWND,
    }

    unsafe fn hwnd_class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..len])
    }

    unsafe extern "system" fn enum_worker_proc(top: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut WorkerSearch);
        let def_view_cls = to_wide("SHELLDLL_DefView");
        if let Ok(sv) = FindWindowExW(top, HWND::default(), PCWSTR(def_view_cls.as_ptr()), PCWSTR::null()) {
            if !sv.is_invalid() {
                state.defview_parent = top;
                // Windows 11 & 10: target WorkerW is the sibling immediately below defview_parent
                if let Ok(next) = GetWindow(top, GW_HWNDNEXT) {
                    if !next.is_invalid() && hwnd_class_name(next).eq_ignore_ascii_case("WorkerW") {
                        state.worker_w = next;
                        return BOOL(0);
                    }
                }
                let worker_cls = to_wide("WorkerW");
                if let Ok(worker) = FindWindowExW(HWND::default(), top, PCWSTR(worker_cls.as_ptr()), PCWSTR::null()) {
                    if !worker.is_invalid() {
                        state.worker_w = worker;
                        return BOOL(0);
                    }
                }
            }
        }
        BOOL(1)
    }

    pub fn resolve_worker_w() -> HWND {
        unsafe {
            let progman_cls = to_wide("Progman");
            let progman = FindWindowW(PCWSTR(progman_cls.as_ptr()), PCWSTR::null())
                .unwrap_or_default();
            if !progman.is_invalid() {
                let mut result = 0usize;
                let _ = SendMessageTimeoutW(
                    progman,
                    0x052C,
                    WPARAM(0x0D),
                    LPARAM(0x01),
                    SMTO_NORMAL,
                    1000,
                    Some(&mut result),
                );
                let _ = SendMessageTimeoutW(
                    progman,
                    0x052C,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_NORMAL,
                    500,
                    Some(&mut result),
                );

                // Windows 11 24H2: SHELLDLL_DefView stays a child of Progman, with WorkerW child below it
                let def_view_cls = to_wide("SHELLDLL_DefView");
                let worker_cls = to_wide("WorkerW");
                if let Ok(sv) = FindWindowExW(progman, HWND::default(), PCWSTR(def_view_cls.as_ptr()), PCWSTR::null()) {
                    if !sv.is_invalid() {
                        if let Ok(child_worker) = FindWindowExW(progman, sv, PCWSTR(worker_cls.as_ptr()), PCWSTR::null()) {
                            if !child_worker.is_invalid() {
                                return child_worker;
                            }
                        }
                        if let Ok(next_child) = GetWindow(sv, GW_HWNDNEXT) {
                            if !next_child.is_invalid() && hwnd_class_name(next_child).eq_ignore_ascii_case("WorkerW") {
                                return next_child;
                            }
                        }
                    }
                }
            }

            let mut search = WorkerSearch {
                defview_parent: HWND::default(),
                worker_w: HWND::default(),
            };
            let _ = EnumWindows(Some(enum_worker_proc), LPARAM(&mut search as *mut WorkerSearch as isize));
            if !search.worker_w.is_invalid() {
                search.worker_w
            } else if !progman.is_invalid() {
                progman
            } else {
                HWND::default()
            }
        }
    }

    pub fn register_window_classes(
        hinstance: HINSTANCE,
        wnd_proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    ) {
        unsafe {
            for class_name in ["PrismViewWallpaperHost", "PrismViewWidgetWindow"] {
                let wide = to_wide(class_name);
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: hinstance,
                    lpszClassName: PCWSTR(wide.as_ptr()),
                    ..Default::default()
                };
                let _ = RegisterClassExW(&wc);
            }
        }
    }

    pub fn create_wallpaper_host(hinstance: HINSTANCE) -> Option<HWND> {
        unsafe {
            let parent = resolve_worker_w();
            let cls = to_wide("PrismViewWallpaperHost");
            let title = to_wide("PrismView Engine");
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1280);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(720);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(cls.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                vw,
                vh,
                parent,
                None,
                hinstance,
                None,
            )
            .ok()?;

            if !parent.is_invalid() {
                let _ = SetParent(hwnd, parent);
                let cur_style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                let child_style = (cur_style & !WS_POPUP.0) | WS_CHILD.0 | WS_VISIBLE.0 | WS_CLIPSIBLINGS.0;
                let _ = SetWindowLongW(hwnd, GWL_STYLE, child_style as i32);
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    vw,
                    vh,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_BOTTOM,
                    GetSystemMetrics(SM_XVIRTUALSCREEN),
                    GetSystemMetrics(SM_YVIRTUALSCREEN),
                    vw,
                    vh,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            Some(hwnd)
        }
    }

    pub fn create_widget_window(
        hinstance: HINSTANCE,
        title: &str,
        placement: &WidgetPlacement,
    ) -> Option<HWND> {
        unsafe {
            let cls = to_wide("PrismViewWidgetWindow");
            let title_w = to_wide(title);
            let mut ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            if placement.click_through {
                ex |= WS_EX_TRANSPARENT;
            }
            let hwnd = CreateWindowExW(
                ex,
                PCWSTR(cls.as_ptr()),
                PCWSTR(title_w.as_ptr()),
                WS_POPUP | WS_VISIBLE,
                placement.pos_x,
                placement.pos_y,
                placement.width_px.max(120) as i32,
                placement.height_px.max(80) as i32,
                HWND::default(),
                None,
                hinstance,
                None,
            )
            .ok()?;
            if placement.pinned_to_desktop {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_BOTTOM,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            Some(hwnd)
        }
    }
}

mod visibility {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PauseCheck {
        pub paused: bool,
        pub reason: Option<&'static str>,
    }

    pub fn evaluate_visibility(session_locked: bool, manual_pause: bool, studio_preview: bool) -> PauseCheck {
        if manual_pause {
            return PauseCheck {
                paused: true,
                reason: Some("manual_pause"),
            };
        }
        if session_locked {
            return PauseCheck {
                paused: true,
                reason: Some("session_locked"),
            };
        }
        if studio_preview {
            return PauseCheck {
                paused: false,
                reason: None,
            };
        }
        unsafe {
            if let Ok(quns) = SHQueryUserNotificationState() {
                if quns != QUNS_ACCEPTS_NOTIFICATIONS {
                    let reason = if quns == QUNS_RUNNING_D3D_FULL_SCREEN {
                        "d3d_fullscreen"
                    } else if quns == QUNS_BUSY {
                        "fullscreen_app"
                    } else if quns == QUNS_PRESENTATION_MODE {
                        "presentation_mode"
                    } else if quns == QUNS_NOT_PRESENT {
                        "screen_off_or_locked"
                    } else {
                        "user_busy"
                    };
                    return PauseCheck {
                        paused: true,
                        reason: Some(reason),
                    };
                }
            }

            let fg = GetForegroundWindow();
            if !fg.is_invalid() && IsWindowVisible(fg).as_bool() && !is_desktop_shell_window(fg) {
                if IsZoomed(fg).as_bool() {
                    return PauseCheck {
                        paused: true,
                        reason: Some("maximized_window"),
                    };
                }
                let mut win_rect = RECT::default();
                if GetWindowRect(fg, &mut win_rect).is_ok() {
                    let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
                    let mut mi = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                        let m = mi.rcMonitor;
                        let covers_monitor = win_rect.left <= m.left
                            && win_rect.top <= m.top
                            && win_rect.right >= m.right
                            && win_rect.bottom >= m.bottom;
                        if covers_monitor {
                            return PauseCheck {
                                paused: true,
                                reason: Some("borderless_fullscreen"),
                            };
                        }
                    }
                }
            }
        }
        PauseCheck {
            paused: false,
            reason: None,
        }
    }

    unsafe fn is_desktop_shell_window(hwnd: HWND) -> bool {
        let mut buf = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut buf) as usize;
        if len == 0 {
            return false;
        }
        let cls = String::from_utf16_lossy(&buf[..len]);
        matches!(
            cls.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        )
    }
}

struct IpcEnvelope {
    req: IpcRequest,
    reply: Sender<IpcResponse>,
}

mod ipc_server {
    use super::*;

    /// `wake` acorda o loop do host depois de enfileirar um pedido (cada host usa o seu).
    pub fn spawn_named_pipe_server(wake: impl Fn() + Send + 'static, tx: Sender<IpcEnvelope>) {
        thread::spawn(move || {
            let pipe_w = to_wide(IPC_PIPE_NAME);
            loop {
                unsafe {
                    let pipe: HANDLE = CreateNamedPipeW(
                        PCWSTR(pipe_w.as_ptr()),
                        PIPE_ACCESS_DUPLEX,
                        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                        PIPE_UNLIMITED_INSTANCES,
                        64 * 1024,
                        64 * 1024,
                        0,
                        None,
                    );
                    if pipe == INVALID_HANDLE_VALUE {
                        thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    }

                    let connected = ConnectNamedPipe(pipe, None).is_ok()
                        || GetLastError() == ERROR_PIPE_CONNECTED;
                    if connected {
                        handle_pipe_client(pipe, &wake, &tx);
                    }
                    let _ = DisconnectNamedPipe(pipe);
                    let _ = CloseHandle(pipe);
                }
            }
        });
    }

    unsafe fn handle_pipe_client(pipe: HANDLE, wake: &dyn Fn(), tx: &Sender<IpcEnvelope>) {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let mut bytes_read = 0u32;
            if ReadFile(pipe, Some(&mut buf), Some(&mut bytes_read), None).is_err() || bytes_read == 0
            {
                break;
            }
            let payload = &buf[..bytes_read as usize];
            let resp = match serde_json::from_slice::<IpcRequest>(payload) {
                Ok(req) => {
                    let (reply_tx, reply_rx) = mpsc::channel();
                    if tx.send(IpcEnvelope { req, reply: reply_tx }).is_ok() {
                        wake();
                        reply_rx
                            .recv_timeout(std::time::Duration::from_secs(3))
                            .unwrap_or(IpcResponse::Error {
                                message: "engine_timeout".to_string(),
                            })
                    } else {
                        IpcResponse::Error {
                            message: "engine_closed".to_string(),
                        }
                    }
                }
                Err(e) => IpcResponse::Error {
                    message: format!("invalid_json: {e}"),
                },
            };

            if let Ok(mut out) = serde_json::to_vec(&resp) {
                out.push(b'\n');
                let mut written = 0u32;
                if WriteFile(pipe, Some(&out), Some(&mut written), None).is_err() {
                    break;
                }
                let _ = FlushFileBuffers(pipe);
            }
        }
    }
}

mod gpu_renderer {
    use super::*;

    const LIQUID_GLASS_WGSL: &str = include_str!("shaders/liquid_glass.wgsl");

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable)]
    struct KawaseUniforms {
        texel_size: [f32; 2],
        offset: f32,
        dim_alpha: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable)]
    struct CrystalUniforms {
        resolution: [f32; 2],
        box_origin: [f32; 2],
        box_size: [f32; 2],
        parallax_offset: [f32; 2],
        refraction_px: f32,
        bevel_radius: f32,
        stroke_radius: f32,
        dispersion_rgb: f32,
        ior_rgb: [f32; 3],
        frost_amount: f32,
        fresnel_intensity: f32,
        prismatic_rim_intensity: f32,
        light_angle_rad: f32,
        saturation: f32,
        tint_rgba: [f32; 4],
        solid_rgba: [f32; 4],
        opacity: f32,
        fill_mode: u32,
        grain: f32,
        corner_radius: f32,
    }

    struct CachedLens {
        signature: String,
        width: u32,
        height: u32,
        _tex: wgpu::Texture,
        view: wgpu::TextureView,
    }

    pub struct GpuRenderer {
        pub adapter_name: String,
        host_hwnd_raw: isize,
        surface: wgpu::Surface<'static>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        config: wgpu::SurfaceConfiguration,
        linear_sampler: wgpu::Sampler,
        bg_bgl: wgpu::BindGroupLayout,
        crystal_bgl: wgpu::BindGroupLayout,
        blit_pipeline: wgpu::RenderPipeline,
        kawase_down_pipeline: wgpu::RenderPipeline,
        kawase_up_pipeline: wgpu::RenderPipeline,
        crystal_pipeline: wgpu::RenderPipeline,
        kawase_buf: wgpu::Buffer,
        crystal_buf: wgpu::Buffer,
        bg_sharp_tex: wgpu::Texture,
        bg_sharp_view: wgpu::TextureView,
        kawase_half_tex: wgpu::Texture,
        kawase_half_view: wgpu::TextureView,
        bg_frosted_tex: wgpu::Texture,
        bg_frosted_view: wgpu::TextureView,
        default_lens_view: wgpu::TextureView,
        lens_cache: HashMap<String, CachedLens>,
        font_cache: HashMap<String, FontArc>,
        loaded_bg_key: String,
        video_player: Option<crate::video::WinRtVideoPlayer>,
    }

    impl GpuRenderer {
        pub fn new(hwnd: HWND, width: u32, height: u32) -> Option<Self> {
            let w = width.max(64);
            let h = height.max(64);
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::DX12,
                ..Default::default()
            });

            let nz = NonZeroIsize::new(hwnd.0 as isize)?;
            let raw_window_handle = RawWindowHandle::Win32(Win32WindowHandle::new(nz));
            let raw_display_handle = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
            let surface = unsafe {
                instance
                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle,
                        raw_window_handle,
                    })
                    .ok()?
            };

            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            }))?;
            let adapter_name = adapter.get_info().name;

            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("prismview_dx12_device"),
                    ..Default::default()
                },
                None,
            ))
            .ok()?;

            let caps = surface.get_capabilities(&adapter);
            let format = caps
                .formats
                .iter()
                .copied()
                .find(|f| !f.is_srgb())
                .unwrap_or(caps.formats[0]);

            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: w,
                height: h,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: caps.alpha_modes[0],
                view_formats: Vec::new(),
            };
            surface.configure(&device, &config);

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("liquid_glass_wgsl"),
                source: wgpu::ShaderSource::Wgsl(LIQUID_GLASS_WGSL.into()),
            });

            let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("clamp_linear"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            let bg_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bg_kawase_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

            let crystal_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("crystal_bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

            let bg_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("bg_kawase_pl"),
                bind_group_layouts: &[&bg_bgl],
                push_constant_ranges: &[],
            });
            let crystal_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("crystal_pl"),
                bind_group_layouts: &[&crystal_bgl],
                push_constant_ranges: &[],
            });

            let blit_pipeline = make_fs_pipeline(&device, &bg_pl, &shader, "fs_background_blit", format, false);
            let kawase_down_pipeline = make_fs_pipeline(
                &device,
                &bg_pl,
                &shader,
                "fs_kawase_down",
                wgpu::TextureFormat::Rgba8Unorm,
                false,
            );
            let kawase_up_pipeline = make_fs_pipeline(
                &device,
                &bg_pl,
                &shader,
                "fs_kawase_up",
                wgpu::TextureFormat::Rgba8Unorm,
                false,
            );
            let crystal_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("crystal_pipeline"),
                layout: Some(&crystal_pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_layer"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_liquid_glass"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

            let kawase_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("kawase_uniforms"),
                size: std::mem::size_of::<KawaseUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let crystal_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("crystal_uniforms"),
                size: std::mem::size_of::<CrystalUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let (bg_sharp_tex, bg_sharp_view) = create_rgba_tex(&device, w, h, "bg_sharp");
            let (kawase_half_tex, kawase_half_view) =
                create_rgba_tex(&device, (w / 2).max(1), (h / 2).max(1), "kawase_half");
            let (bg_frosted_tex, bg_frosted_view) = create_rgba_tex(&device, w, h, "bg_frosted");

            // 1x1 neutral opaque lens map (SDF rounded-rect fallback in shader)
            let (def_lens_tex, default_lens_view) = create_rgba_tex(&device, 1, 1, "default_lens");
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &def_lens_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[0u8, 0u8, 0u8, 255u8],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );

            let mut renderer = Self {
                adapter_name,
                host_hwnd_raw: hwnd.0 as isize,
                surface,
                device,
                queue,
                config,
                linear_sampler,
                bg_bgl,
                crystal_bgl,
                blit_pipeline,
                kawase_down_pipeline,
                kawase_up_pipeline,
                crystal_pipeline,
                kawase_buf,
                crystal_buf,
                bg_sharp_tex,
                bg_sharp_view,
                kawase_half_tex,
                kawase_half_view,
                bg_frosted_tex,
                bg_frosted_view,
                default_lens_view,
                lens_cache: HashMap::new(),
                font_cache: HashMap::new(),
                loaded_bg_key: String::new(),
                video_player: None,
            };
            renderer.upload_default_oled_gradient();
            Some(renderer)
        }

        pub fn resize(&mut self, width: u32, height: u32) {
            let w = width.max(64);
            let h = height.max(64);
            if self.config.width == w && self.config.height == h {
                return;
            }
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
            let (st, sv) = create_rgba_tex(&self.device, w, h, "bg_sharp");
            let (ht, hv) = create_rgba_tex(&self.device, (w / 2).max(1), (h / 2).max(1), "kawase_half");
            let (ft, fv) = create_rgba_tex(&self.device, w, h, "bg_frosted");
            self.bg_sharp_tex = st;
            self.bg_sharp_view = sv;
            self.kawase_half_tex = ht;
            self.kawase_half_view = hv;
            self.bg_frosted_tex = ft;
            self.bg_frosted_view = fv;
            self.loaded_bg_key.clear();
        }

        fn upload_default_oled_gradient(&mut self) {
            let w = self.config.width;
            let h = self.config.height;
            let wf = w.max(1) as f32;
            let hf = h.max(1) as f32;
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                let ny = y as f32 / hf;
                for x in 0..w {
                    let nx = x as f32 / wf;
                    let dr = ((nx - 0.34).powi(2) + (ny - 0.42).powi(2)).sqrt();
                    let dg = ((nx - 0.50).powi(2) + (ny - 0.56).powi(2)).sqrt();
                    let db = ((nx - 0.66).powi(2) + (ny - 0.42).powi(2)).sqrt();
                    let beam_r = (-dr * dr * 7.5).exp() * 0.42;
                    let beam_g = (-dg * dg * 8.0).exp() * 0.28;
                    let beam_b = (-db * db * 7.5).exp() * 0.46;
                    let diag = (1.0 - ((nx * 0.85 - ny + 0.08).abs() * 6.5).clamp(0.0, 1.0)).powi(2) * 0.16;

                    let r = ((0.02 + beam_r + diag * 1.1) * 255.0).clamp(0.0, 255.0) as u8;
                    let g = ((0.02 + beam_g + diag * 0.95) * 255.0).clamp(0.0, 255.0) as u8;
                    let b = ((0.03 + beam_b + diag * 1.15) * 255.0).clamp(0.0, 255.0) as u8;
                    let i = ((y * w + x) * 4) as usize;
                    pixels[i] = r;
                    pixels[i + 1] = g;
                    pixels[i + 2] = b;
                    pixels[i + 3] = 255;
                }
            }
            self.upload_background_rgba(&pixels, w, h);
        }

        pub fn has_active_video(&self) -> bool {
            self.video_player.is_some()
        }

        pub fn set_video_paused(&mut self, paused: bool) {
            if let Some(vp) = self.video_player.as_mut() {
                vp.set_paused(paused);
            }
        }

        pub fn poll_video_frame(&mut self, needs_frost: bool) -> bool {
            let maybe = self
                .video_player
                .as_mut()
                .and_then(|vp| vp.poll_rgba_frame().map(|(s, w, h)| (s.to_vec(), w, h)));
            if let Some((rgba, vw, vh)) = maybe {
                if vw == self.config.width && vh == self.config.height {
                    self.upload_background_rgba_opt(&rgba, vw, vh, needs_frost);
                } else if let Some(img) = image::RgbaImage::from_raw(vw, vh, rgba) {
                    let resized = image::imageops::resize(
                        &img,
                        self.config.width,
                        self.config.height,
                        image::imageops::FilterType::Triangle,
                    );
                    self.upload_background_rgba_opt(
                        resized.as_raw(),
                        self.config.width,
                        self.config.height,
                        needs_frost,
                    );
                }
                return true;
            }
            false
        }

        pub fn ensure_background(&mut self, bg: &BackgroundSource) {
            let mut video_path: Option<(PathBuf, bool)> = None;
            let mut image_path: Option<PathBuf> = None;

            match bg {
                BackgroundSource::Image { path } => {
                    image_path = Some(PathBuf::from(path));
                }
                BackgroundSource::Video { path, muted, .. } => {
                    let p = PathBuf::from(path);
                    if p.is_file() {
                        video_path = Some((p, *muted));
                    }
                }
                BackgroundSource::SteamWorkshop { project_json_path, .. } => {
                    let p = PathBuf::from(project_json_path);
                    if let Ok(meta) = SteamWorkshopMeta::from_project_json(&p) {
                        if let Some(dir) = p.parent() {
                            if !meta.file.is_empty() {
                                let media = dir.join(&meta.file);
                                let ext = media
                                    .extension()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                                    .to_ascii_lowercase();
                                if media.is_file()
                                    && matches!(ext.as_str(), "mp4" | "webm" | "mkv" | "mov")
                                {
                                    video_path = Some((media, true));
                                } else if media.is_file()
                                    && matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp")
                                {
                                    image_path = Some(media);
                                }
                            }
                        }
                    }
                }
                BackgroundSource::None => {
                    self.video_player = None;
                    if !self.loaded_bg_key.is_empty() {
                        self.loaded_bg_key.clear();
                        self.upload_default_oled_gradient();
                    }
                    return;
                }
                _ => {
                    self.video_player = None;
                }
            }

            if let Some((vp, muted)) = video_path {
                let vkey = vp.to_string_lossy().to_string();
                let already = self
                    .video_player
                    .as_ref()
                    .map(|p| p.source_key == vkey)
                    .unwrap_or(false);
                if !already {
                    self.video_player = crate::video::WinRtVideoPlayer::open(
                        &vp,
                        self.config.width,
                        self.config.height,
                        muted,
                        self.host_hwnd_raw,
                        WM_PRISM_VIDEO_FRAME,
                    );
                    self.loaded_bg_key = vkey;
                }
                return;
            } else {
                self.video_player = None;
            }

            let Some(path) = image_path else { return };
            let key = path.to_string_lossy().to_string();
            if key == self.loaded_bg_key {
                return;
            }
            if let Ok(img) = image::open(&path) {
                let resized = img
                    .resize_to_fill(
                        self.config.width,
                        self.config.height,
                        image::imageops::FilterType::Triangle,
                    )
                    .to_rgba8();
                self.upload_background_rgba(resized.as_raw(), self.config.width, self.config.height);
                self.loaded_bg_key = key;
            }
        }

        pub fn upload_background_rgba(&mut self, rgba: &[u8], w: u32, h: u32) {
            self.upload_background_rgba_opt(rgba, w, h, true);
        }

        pub fn upload_background_rgba_opt(
            &mut self,
            rgba: &[u8],
            w: u32,
            h: u32,
            rebuild_frost: bool,
        ) {
            if w != self.config.width || h != self.config.height {
                return;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.bg_sharp_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            if rebuild_frost {
                self.rebuild_frosted_dual_kawase(4.0);
            }
        }

        fn rebuild_frosted_dual_kawase(&self, offset: f32) {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("kawase_encoder"),
                });
            let half_w = (self.config.width / 2).max(1) as f32;
            let half_h = (self.config.height / 2).max(1) as f32;

            // Pass 1: Downsample sharp -> half
            let u_down = KawaseUniforms {
                texel_size: [1.0 / self.config.width as f32, 1.0 / self.config.height as f32],
                offset,
                dim_alpha: 0.0,
            };
            self.queue.write_buffer(&self.kawase_buf, 0, bytemuck::bytes_of(&u_down));
            let bg_down = self.make_bg_bind_group(&self.bg_sharp_view);
            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("kawase_down"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.kawase_half_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rp.set_pipeline(&self.kawase_down_pipeline);
                rp.set_bind_group(0, &bg_down, &[]);
                rp.draw(0..3, 0..1);
            }

            // Pass 2: Upsample half -> frosted
            let u_up = KawaseUniforms {
                texel_size: [1.0 / half_w, 1.0 / half_h],
                offset: offset * 1.35,
                dim_alpha: 0.0,
            };
            self.queue.write_buffer(&self.kawase_buf, 0, bytemuck::bytes_of(&u_up));
            let bg_up = self.make_bg_bind_group(&self.kawase_half_view);
            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("kawase_up"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.bg_frosted_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rp.set_pipeline(&self.kawase_up_pipeline);
                rp.set_bind_group(0, &bg_up, &[]);
                rp.draw(0..3, 0..1);
            }

            self.queue.submit(std::iter::once(encoder.finish()));
        }

        fn load_font(&mut self, font_id: &str, weight: u16) -> Option<FontArc> {
            let key = format!("{}|{}", font_id.to_ascii_lowercase(), weight >= 600);
            if let Some(f) = self.font_cache.get(&key) {
                return Some(f.clone());
            }
            let needle = font_id.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
            let mut search_dirs = vec![PathBuf::from(r"C:\Windows\Fonts")];
            if let Some(appdata) = std::env::var_os("APPDATA") {
                search_dirs.insert(0, PathBuf::from(appdata).join("PrismView").join("fonts"));
            }
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                search_dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
            }

            let mut candidate: Option<PathBuf> = None;
            if !needle.is_empty() {
                for dir in &search_dirs {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
                            if ext != "ttf" && ext != "otf" {
                                continue;
                            }
                            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase().replace([' ', '-', '_'], "");
                            if stem == needle || stem.contains(&needle) {
                                candidate = Some(p);
                                break;
                            }
                        }
                    }
                    if candidate.is_some() {
                        break;
                    }
                }
            }

            let fallbacks = if weight >= 600 {
                ["segoeuib.ttf", "bahnschrift.ttf", "arialbd.ttf", "segoeui.ttf", "arial.ttf"]
            } else {
                ["segoeui.ttf", "bahnschrift.ttf", "segoeuil.ttf", "arial.ttf", "calibri.ttf"]
            };
            let path = candidate.or_else(|| {
                fallbacks
                    .iter()
                    .map(|f| PathBuf::from(r"C:\Windows\Fonts").join(f))
                    .find(|p| p.is_file())
            })?;
            let bytes = std::fs::read(path).ok()?;
            let font = FontArc::try_from_vec(bytes).ok()?;
            self.font_cache.insert(key, font.clone());
            Some(font)
        }

        fn ensure_text_lens(
            &mut self,
            node_id: &str,
            text: &str,
            font_id: &str,
            size_px: f32,
            weight: u16,
            stretch: f32,
            letter_spacing_em: f32,
            align: TextAlign,
        ) -> Option<(u32, u32)> {
            let clean_text = if text.trim().is_empty() { " " } else { text };
            let clamped_size = size_px.clamp(12.0, 360.0);
            let clamped_stretch = stretch.clamp(0.75, 2.8);
            let sig = format!(
                "txt|{}|{}|{:.1}|{}|{:.2}|{:.3}|{:?}",
                clean_text, font_id, clamped_size, weight, clamped_stretch, letter_spacing_em, align
            );
            if let Some(c) = self.lens_cache.get(node_id) {
                if c.signature == sig {
                    return Some((c.width, c.height));
                }
            }

            let font = self.load_font(font_id, weight)?;
            let scale = PxScale {
                x: clamped_size,
                y: clamped_size * clamped_stretch,
            };
            let scaled = font.as_scaled(scale);
            let ascent = scaled.ascent();
            let descent = scaled.descent();
            let line_h = (ascent - descent + scaled.line_gap()).max(clamped_size * clamped_stretch);
            let extra_track = letter_spacing_em * clamped_size;

            let lines: Vec<&str> = clean_text.split('\n').collect();
            let mut line_widths = Vec::with_capacity(lines.len());
            let mut max_w = 0.0f32;
            for line in &lines {
                let mut pen_x = 0.0f32;
                let mut prev: Option<GlyphId> = None;
                for ch in line.chars() {
                    let gid = font.glyph_id(ch);
                    if let Some(p) = prev {
                        pen_x += scaled.kern(p, gid);
                    }
                    pen_x += scaled.h_advance(gid) + extra_track;
                    prev = Some(gid);
                }
                let w = pen_x.max(4.0);
                max_w = max_w.max(w);
                line_widths.push(w);
            }

            let pad = (clamped_size * 0.16).clamp(10.0, 28.0);
            let tex_w = ((max_w + pad * 2.0).ceil() as u32).clamp(16, 2048);
            let tex_h = (((line_h * lines.len() as f32) + pad * 2.0).ceil() as u32).clamp(16, 1024);

            let mut alpha_buf = vec![0.0f32; (tex_w * tex_h) as usize];
            for (li, line) in lines.iter().enumerate() {
                let lw = line_widths[li];
                let start_x = match align {
                    TextAlign::Start => pad,
                    TextAlign::Center => ((tex_w as f32 - lw) * 0.5).max(pad * 0.5),
                    TextAlign::End => (tex_w as f32 - pad - lw).max(0.0),
                };
                let baseline_y = pad + ascent + (li as f32 * line_h);
                let mut pen_x = start_x;
                let mut prev: Option<GlyphId> = None;
                for ch in line.chars() {
                    let gid = font.glyph_id(ch);
                    if let Some(p) = prev {
                        pen_x += scaled.kern(p, gid);
                    }
                    let glyph = gid.with_scale_and_position(scale, ab_glyph::point(pen_x, baseline_y));
                    if let Some(outlined) = font.outline_glyph(glyph) {
                        let bounds = outlined.px_bounds();
                        let min_x = bounds.min.x.floor() as i32;
                        let min_y = bounds.min.y.floor() as i32;
                        outlined.draw(|gx, gy, cov| {
                            let px = min_x + gx as i32;
                            let py = min_y + gy as i32;
                            if px >= 0 && py >= 0 && (px as u32) < tex_w && (py as u32) < tex_h {
                                let idx = (py as u32 * tex_w + px as u32) as usize;
                                alpha_buf[idx] = alpha_buf[idx].max(cov.clamp(0.0, 1.0));
                            }
                        });
                    }
                    pen_x += scaled.h_advance(gid) + extra_track;
                    prev = Some(gid);
                }
            }

            // Smooth 2-pass separable box blur over alpha_buf -> 3D convex bevel height map for Optical Crystal
            let blur_r = (clamped_size * 0.045).clamp(2.0, 7.0) as i32;
            let mut tmp = vec![0.0f32; alpha_buf.len()];
            let mut height_buf = vec![0.0f32; alpha_buf.len()];
            let inv_k = 1.0 / (2 * blur_r + 1) as f32;
            for y in 0..tex_h as i32 {
                for x in 0..tex_w as i32 {
                    let mut sum = 0.0f32;
                    for k in -blur_r..=blur_r {
                        let sx = (x + k).clamp(0, tex_w as i32 - 1);
                        sum += alpha_buf[(y * tex_w as i32 + sx) as usize];
                    }
                    tmp[(y * tex_w as i32 + x) as usize] = sum * inv_k;
                }
            }
            for y in 0..tex_h as i32 {
                for x in 0..tex_w as i32 {
                    let mut sum = 0.0f32;
                    for k in -blur_r..=blur_r {
                        let sy = (y + k).clamp(0, tex_h as i32 - 1);
                        sum += tmp[(sy * tex_w as i32 + x) as usize];
                    }
                    height_buf[(y * tex_w as i32 + x) as usize] = sum * inv_k;
                }
            }

            let mut rgba = vec![0u8; (tex_w * tex_h * 4) as usize];
            for i in 0..(tex_w * tex_h) as usize {
                let a = (alpha_buf[i] * 255.0).round().clamp(0.0, 255.0) as u8;
                let h_val = (height_buf[i] * 255.0).round().clamp(0.0, 255.0) as u8;
                rgba[i * 4] = h_val.max(if a > 0 { 4 } else { 0 });
                rgba[i * 4 + 1] = h_val;
                rgba[i * 4 + 2] = h_val;
                rgba[i * 4 + 3] = a;
            }

            let (tex, view) = create_rgba_tex(&self.device, tex_w, tex_h, "text_lens");
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(tex_w * 4),
                    rows_per_image: Some(tex_h),
                },
                wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: 1,
                },
            );
            self.lens_cache.insert(
                node_id.to_string(),
                CachedLens {
                    signature: sig,
                    width: tex_w,
                    height: tex_h,
                    _tex: tex,
                    view,
                },
            );
            Some((tex_w, tex_h))
        }

        fn ensure_image_lens(&mut self, node_id: &str, asset_path: &str, w: u32, h: u32) {
            if asset_path.is_empty() || !Path::new(asset_path).is_file() {
                return;
            }
            let sig = format!("{asset_path}|{w}x{h}");
            if self
                .lens_cache
                .get(node_id)
                .map(|c| c.signature == sig)
                .unwrap_or(false)
            {
                return;
            }
            if let Ok(img) = image::open(asset_path) {
                let lw = w.clamp(16, 512);
                let lh = h.clamp(16, 512);
                let rgba = img
                    .resize_to_fill(lw, lh, image::imageops::FilterType::Triangle)
                    .to_rgba8();
                let mut height_rgba = vec![0u8; (lw * lh * 4) as usize];
                for i in 0..(lw * lh) as usize {
                    let a = rgba.as_raw()[i * 4 + 3];
                    height_rgba[i * 4] = a;
                    height_rgba[i * 4 + 1] = a;
                    height_rgba[i * 4 + 2] = a;
                    height_rgba[i * 4 + 3] = a;
                }
                let (tex, view) = create_rgba_tex(&self.device, lw, lh, "img_lens");
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &height_rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(lw * 4),
                        rows_per_image: Some(lh),
                    },
                    wgpu::Extent3d {
                        width: lw,
                        height: lh,
                        depth_or_array_layers: 1,
                    },
                );
                self.lens_cache.insert(
                    node_id.to_string(),
                    CachedLens {
                        signature: sig,
                        width: lw,
                        height: lh,
                        _tex: tex,
                        view,
                    },
                );
            }
        }

        fn make_bg_bind_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bg_kawase_bg"),
                layout: &self.bg_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.kawase_buf.as_entire_binding(),
                    },
                ],
            })
        }

        pub fn render_scene(&mut self, scene: &PrismScene, ctx: &EvalContext) -> bool {
            self.ensure_background(&scene.background);

            // Pre-bake cached image alpha lens maps outside render pass
            let mut layer_uniforms = Vec::new();
            let sw = self.config.width as f32;
            let sh = self.config.height as f32;
            for (layer, _) in scene.flatten_layers() {
                if layer.hidden || !FxEngine::eval_bool(&layer.visible, ctx).value {
                    continue;
                }
                if layer.kind == LayerKind::Stack || layer.kind == LayerKind::DepthSubject {
                    continue;
                }
                let mut bw = FxEngine::eval_f32(&layer.width_px, ctx).value.max(8.0);
                let mut bh = FxEngine::eval_f32(&layer.height_px, ctx).value.max(8.0);
                let mut corner_radius = layer.corner_radius_px;

                if layer.kind == LayerKind::Text {
                    let resolved_text = FxEngine::eval_text(&layer.text.text, ctx).value;
                    let font_size = FxEngine::eval_f32(&layer.text.size_px, ctx).value;
                    let stretch = FxEngine::eval_f32(&layer.text.stem_stretch.stretch, ctx).value;
                    if let Some((tw, th)) = self.ensure_text_lens(
                        &layer.id,
                        &resolved_text,
                        &layer.text.font_id,
                        font_size,
                        layer.text.weight,
                        stretch,
                        layer.text.letter_spacing_em,
                        layer.text.align,
                    ) {
                        bw = tw as f32;
                        bh = th as f32;
                    }
                    corner_radius = 0.0;
                } else {
                    let asset_key = FxEngine::eval_string(&layer.asset, ctx).value;
                    let resolved_asset = scene
                        .custom_assets
                        .get(&asset_key)
                        .cloned()
                        .unwrap_or(asset_key);
                    if layer.kind == LayerKind::Image {
                        self.ensure_image_lens(&layer.id, &resolved_asset, bw as u32, bh as u32);
                        corner_radius = 0.0;
                    }
                }

                let nx = FxEngine::eval_f32(&layer.x, ctx).value;
                let ny = FxEngine::eval_f32(&layer.y, ctx).value;
                let ox = FxEngine::eval_f32(&layer.offset_x_px, ctx).value;
                let oy = FxEngine::eval_f32(&layer.offset_y_px, ctx).value;
                let (ax, ay) = anchor_offset(layer.anchor, bw, bh);
                let depth_z = FxEngine::eval_f32(&layer.parallax.depth_z, ctx).value;
                let p_strength = layer.parallax.mouse_strength + scene.depth.parallax_strength * depth_z;
                let ptr_dx = ctx.pointer_norm[0] - 0.5;
                let ptr_dy = ctx.pointer_norm[1] - 0.5;
                let px_off = ptr_dx * 56.0 * p_strength;
                let py_off = ptr_dy * 38.0 * p_strength;

                let tint_hex = FxEngine::eval_string(&layer.crystal.tint_rgba, ctx).value;
                let solid_hex = FxEngine::eval_string(&layer.color, ctx).value;
                let fill_mode = match layer.fill {
                    FillMode::ClassicBlur => 1u32,
                    FillMode::OpticalCrystal => 2u32,
                    _ => 0u32,
                };
                let refr_pct = FxEngine::eval_f32(&layer.crystal.refraction, ctx).value.clamp(0.0, 100.0);
                let base_disp = FxEngine::eval_f32(&layer.crystal.dispersion_rgb, ctx).value.clamp(0.0, 0.05);
                let cursor_boost = (ptr_dx.hypot(ptr_dy) * 0.004).min(0.006);
                let base_angle = FxEngine::eval_f32(&layer.crystal.light_angle_deg, ctx).value.to_radians();

                let uniforms = CrystalUniforms {
                    resolution: [sw, sh],
                    box_origin: [nx * sw + ax + ox, ny * sh + ay + oy],
                    box_size: [bw, bh],
                    parallax_offset: [px_off, py_off],
                    refraction_px: (refr_pct / 100.0) * (sw * 0.14),
                    bevel_radius: FxEngine::eval_f32(&layer.crystal.bevel_width, ctx).value.clamp(1.0, 48.0),
                    stroke_radius: (bw.min(bh) * 0.28).max(6.0),
                    dispersion_rgb: (base_disp + cursor_boost).clamp(0.0, 0.05),
                    ior_rgb: [
                        layer.crystal.ior_red,
                        layer.crystal.ior_green,
                        layer.crystal.ior_blue,
                    ],
                    frost_amount: FxEngine::eval_f32(&layer.crystal.frost_amount, ctx).value.clamp(0.0, 1.0),
                    fresnel_intensity: FxEngine::eval_f32(&layer.crystal.fresnel_intensity, ctx).value.clamp(0.0, 1.0),
                    prismatic_rim_intensity: FxEngine::eval_f32(&layer.crystal.prismatic_rim_intensity, ctx).value.clamp(0.0, 1.5),
                    light_angle_rad: base_angle + ptr_dx * 1.35 + ptr_dy * 0.65,
                    saturation: FxEngine::eval_f32(&layer.crystal.saturation, ctx).value.clamp(0.5, 2.0),
                    tint_rgba: FxEngine::parse_rgba_hex(&tint_hex).unwrap_or([1.0, 1.0, 1.0, 0.15]),
                    solid_rgba: FxEngine::parse_rgba_hex(&solid_hex).unwrap_or([1.0, 1.0, 1.0, 1.0]),
                    opacity: FxEngine::eval_f32(&layer.opacity, ctx).value.clamp(0.0, 1.0),
                    fill_mode,
                    grain: layer.crystal.grain,
                    corner_radius,
                };
                layer_uniforms.push((layer.id.clone(), uniforms));
            }

            let Ok(frame) = self.surface.get_current_texture() else {
                return false;
            };
            let target_view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());

            let blit_u = KawaseUniforms {
                texel_size: [1.0 / sw, 1.0 / sh],
                offset: 1.0,
                dim_alpha: scene.dim.clamp(0.0, 0.85),
            };
            self.queue
                .write_buffer(&self.kawase_buf, 0, bytemuck::bytes_of(&blit_u));
            let bg_bind = self.make_bg_bind_group(&self.bg_sharp_view);

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("scene_encoder"),
                });

            {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("background_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rp.set_pipeline(&self.blit_pipeline);
                rp.set_bind_group(0, &bg_bind, &[]);
                rp.draw(0..3, 0..1);
            }
            self.queue.submit(std::iter::once(encoder.finish()));

            for (layer_id, uniforms) in layer_uniforms {
                self.queue
                    .write_buffer(&self.crystal_buf, 0, bytemuck::bytes_of(&uniforms));
                let lens_view = self
                    .lens_cache
                    .get(&layer_id)
                    .map(|c| &c.view)
                    .unwrap_or(&self.default_lens_view);
                let crystal_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("crystal_bg"),
                    layout: &self.crystal_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&self.bg_sharp_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&self.bg_frosted_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(lens_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: self.crystal_buf.as_entire_binding(),
                        },
                    ],
                });
                let mut l_enc = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("layer_encoder"),
                    });
                {
                    let mut rp = l_enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("crystal_layer_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    rp.set_pipeline(&self.crystal_pipeline);
                    rp.set_bind_group(0, &crystal_bg, &[]);
                    rp.draw(0..6, 0..1);
                }
                self.queue.submit(std::iter::once(l_enc.finish()));
            }

            frame.present();
            true
        }
    }

    fn anchor_offset(anchor: Anchor, w: f32, h: f32) -> (f32, f32) {
        let ax = match anchor {
            Anchor::TopLeft | Anchor::CenterLeft | Anchor::BottomLeft => 0.0,
            Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => -w * 0.5,
            Anchor::TopRight | Anchor::CenterRight | Anchor::BottomRight => -w,
        };
        let ay = match anchor {
            Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => 0.0,
            Anchor::CenterLeft | Anchor::Center | Anchor::CenterRight => -h * 0.5,
            Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => -h,
        };
        (ax, ay)
    }

    fn create_rgba_tex(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    }

    fn make_fs_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        entry: &str,
        format: wgpu::TextureFormat,
        alpha_blend: bool,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_fullscreen"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: if alpha_blend {
                        Some(wgpu::BlendState::ALPHA_BLENDING)
                    } else {
                        None
                    },
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    pub fn poll_gsmtc_media(ctx: &mut EvalContext) {
        let Ok(mgr_async) = GlobalSystemMediaTransportControlsSessionManager::RequestAsync() else {
            return;
        };
        let Ok(mgr) = mgr_async.get() else { return };
        let Ok(session) = mgr.GetCurrentSession() else {
            ctx.media.playing = false;
            return;
        };
        if let Ok(info) = session.GetPlaybackInfo() {
            if let Ok(status) = info.PlaybackStatus() {
                ctx.media.playing =
                    status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
            }
        }
        if let Ok(props_async) = session.TryGetMediaPropertiesAsync() {
            if let Ok(props) = props_async.get() {
                ctx.media.title = props.Title().map(|s| s.to_string()).unwrap_or_default();
                ctx.media.artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
                ctx.media.album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
            }
        }
    }

    pub fn poll_pointer_norm(ctx: &mut EvalContext) {
        unsafe {
            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_ok() {
                let sw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as f32;
                let sh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as f32;
                let vx = GetSystemMetrics(SM_XVIRTUALSCREEN) as f32;
                let vy = GetSystemMetrics(SM_YVIRTUALSCREEN) as f32;
                ctx.pointer_norm = [
                    ((pt.x as f32 - vx) / sw).clamp(0.0, 1.0),
                    ((pt.y as f32 - vy) / sh).clamp(0.0, 1.0),
                ];
            }
        }
    }
}

mod tray {
    use super::*;
    use std::process::Command;

    pub fn add_tray_icon(hwnd: HWND) {
        unsafe {
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ICON_ID,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: WM_PRISM_TRAY,
                hIcon: LoadIconW(HINSTANCE::default(), IDI_APPLICATION).unwrap_or_default(),
                ..Default::default()
            };
            let tip = to_wide("PrismView Engine (DX12)");
            let len = tip.len().min(nid.szTip.len());
            nid.szTip[..len].copy_from_slice(&tip[..len]);
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    pub fn remove_tray_icon(hwnd: HWND) {
        unsafe {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: TRAY_ICON_ID,
                ..Default::default()
            };
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    pub fn show_context_menu(hwnd: HWND, paused: bool) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let open_txt = to_wide("Abrir PrismView Studio");
            let pause_txt = to_wide(if paused {
                "Retomar Engine"
            } else {
                "Pausar Engine"
            });
            let exit_txt = to_wide("Sair do PrismView Engine");

            let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN_STUDIO, PCWSTR(open_txt.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, CMD_TOGGLE_PAUSE, PCWSTR(pause_txt.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, PCWSTR(exit_txt.as_ptr()));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
            let _ = DestroyMenu(menu);
        }
    }

    pub fn launch_studio() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let studio = dir.join("prismview-studio.exe");
                if studio.is_file() {
                    let _ = Command::new(studio).spawn();
                }
            }
        }
    }
}

fn build_default_prismatic_scene() -> PrismScene {
    PrismScene {
        id: "builtin-prismatic-crystal".to_string(),
        title: "Monolith Prism".to_string(),
        package_type: PackageKind::Wallpaper,
        dim: 0.04,
        depth: DepthPostProcess {
            enabled: true,
            parallax_strength: 0.72,
            ..DepthPostProcess::default()
        },
        layers: vec![
            LayerNode {
                id: "default-lens-primary".to_string(),
                name: "Lente Prismática".to_string(),
                kind: LayerKind::Shape,
                x: StaticOrFx::Static(0.5),
                y: StaticOrFx::Static(0.46),
                width_px: StaticOrFx::Static(660.0),
                height_px: StaticOrFx::Static(270.0),
                corner_radius_px: 44.0,
                shape_kind: ShapeKind::Rect,
                fill: FillMode::OpticalCrystal,
                parallax: ParallaxParams {
                    depth_z: StaticOrFx::Static(0.35),
                    mouse_strength: 0.25,
                    tilt_max_deg: 0.0,
                },
                crystal: OpticalCrystalParams {
                    refraction: StaticOrFx::Static(44.0),
                    bevel_width: StaticOrFx::Static(28.0),
                    dispersion_rgb: StaticOrFx::Static(0.011),
                    frost_amount: StaticOrFx::Static(0.24),
                    fresnel_intensity: StaticOrFx::Static(0.95),
                    prismatic_rim_intensity: StaticOrFx::Static(0.85),
                    ..OpticalCrystalParams::default()
                },
                ..LayerNode::default()
            },
            LayerNode {
                id: "default-clock-text".to_string(),
                name: "Relógio Óptico".to_string(),
                kind: LayerKind::Text,
                x: StaticOrFx::Static(0.5),
                y: StaticOrFx::Static(0.44),
                fill: FillMode::OpticalCrystal,
                color: StaticOrFx::Static("#FFFFFFFF".to_string()),
                parallax: ParallaxParams {
                    depth_z: StaticOrFx::Static(0.65),
                    mouse_strength: 0.4,
                    tilt_max_deg: 0.0,
                },
                crystal: OpticalCrystalParams {
                    refraction: StaticOrFx::Static(28.0),
                    bevel_width: StaticOrFx::Static(14.0),
                    dispersion_rgb: StaticOrFx::Static(0.012),
                    frost_amount: StaticOrFx::Static(0.14),
                    fresnel_intensity: StaticOrFx::Static(1.0),
                    prismatic_rim_intensity: StaticOrFx::Static(1.0),
                    tint_rgba: StaticOrFx::Static("#88FFFFFF".to_string()),
                    ..OpticalCrystalParams::default()
                },
                text: TextContent {
                    text: "$df(HH)$:$df(mm)$".to_string(),
                    font_id: "Outfit".to_string(),
                    size_px: StaticOrFx::Static(148.0),
                    weight: 700,
                    letter_spacing_em: -0.02,
                    align: TextAlign::Center,
                    stem_stretch: StemStretchParams {
                        stretch: StaticOrFx::Static(1.28),
                        ..StemStretchParams::default()
                    },
                    ..TextContent::default()
                },
                ..LayerNode::default()
            },
            LayerNode {
                id: "default-metrics-text".to_string(),
                name: "Data & Métricas".to_string(),
                kind: LayerKind::Text,
                x: StaticOrFx::Static(0.5),
                y: StaticOrFx::Static(0.63),
                fill: FillMode::Solid,
                color: StaticOrFx::Static("#E6F4F4F5".to_string()),
                parallax: ParallaxParams {
                    depth_z: StaticOrFx::Static(0.45),
                    mouse_strength: 0.25,
                    tilt_max_deg: 0.0,
                },
                text: TextContent {
                    text: "$tc(up, df(EEE dd MMM))$  ·  RAM $sys(ram)$%  ·  BAT $bi(level)$%".to_string(),
                    font_id: "Segoe UI".to_string(),
                    size_px: StaticOrFx::Static(22.0),
                    weight: 600,
                    letter_spacing_em: 0.12,
                    align: TextAlign::Center,
                    ..TextContent::default()
                },
                ..LayerNode::default()
            },
        ],
        ..PrismScene::default()
    }
}

struct PinnedWidget {
    scene: PrismScene,
    hwnd: HWND,
    gpu: Option<gpu_renderer::GpuRenderer>,
    deps: DirtyFlags,
}

struct EngineRuntime {
    hinstance: HINSTANCE,
    host_hwnd: HWND,
    ipc_rx: Receiver<IpcEnvelope>,
    gpu: Option<gpu_renderer::GpuRenderer>,
    wallpaper_scene: Option<PrismScene>,
    wallpaper_deps: DirtyFlags,
    widgets: HashMap<String, PinnedWidget>,
    eval_ctx: EvalContext,
    pending_dirty: DirtyFlags,
    session_locked: bool,
    manual_pause: bool,
    studio_preview_mode: bool,
    paused: bool,
    pause_reason: Option<String>,
    rendered_frames: u64,
    active_tick_ms: Option<u32>,
    storage_dir: PathBuf,
}

impl EngineRuntime {
    fn new(hinstance: HINSTANCE, host_hwnd: HWND, ipc_rx: Receiver<IpcEnvelope>) -> Self {
        let storage_dir = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("PrismView")
            .join("packages");
        let _ = std::fs::create_dir_all(&storage_dir);
        let (w, h) = unsafe {
            (
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1280) as u32,
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(720) as u32,
            )
        };
        let gpu = gpu_renderer::GpuRenderer::new(host_hwnd, w, h);
        let default_scene = build_default_prismatic_scene();
        let mut eval_ctx = EvalContext::default();
        eval_ctx.globals = default_scene.globals_map();
        let wallpaper_deps = FxEngine::collect_scene_dependencies(&default_scene, &eval_ctx);
        Self {
            hinstance,
            host_hwnd,
            ipc_rx,
            gpu,
            wallpaper_scene: Some(default_scene),
            wallpaper_deps,
            widgets: HashMap::new(),
            eval_ctx,
            pending_dirty: DirtyFlags::FULL_REBUILD,
            session_locked: false,
            manual_pause: false,
            studio_preview_mode: false,
            paused: false,
            pause_reason: None,
            rendered_frames: 0,
            active_tick_ms: None,
            storage_dir,
        }
    }

    fn total_deps(&self) -> DirtyFlags {
        let mut deps = self.wallpaper_deps;
        for w in self.widgets.values() {
            deps.insert(w.deps);
        }
        deps
    }

    fn refresh_system_telemetry(&mut self) {
        self.eval_ctx.now = Local::now();
        self.eval_ctx.system = SystemTelemetry::sample_live();
        if let Some(sc) = self.wallpaper_scene.as_ref() {
            if !sc.weather.city.is_empty() && self.eval_ctx.weather.city != sc.weather.city {
                self.eval_ctx.weather = WeatherTelemetry::from_config(&sc.weather);
            }
        }
    }

    fn update_visibility_and_timers(&mut self) {
        let check = visibility::evaluate_visibility(
            self.session_locked,
            self.manual_pause,
            self.studio_preview_mode,
        );
        let was_paused = self.paused;
        self.paused = check.paused;
        self.pause_reason = check.reason.map(str::to_string);
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.set_video_paused(self.paused);
        }

        unsafe {
            if self.paused {
                if self.active_tick_ms.take().is_some() {
                    let _ = KillTimer(self.host_hwnd, TIMER_TICK_ID);
                }
                return;
            }
            if was_paused {
                self.pending_dirty.insert(DirtyFlags::TIME_MINUTE);
            }

            let has_video = self.gpu.as_ref().map(|g| g.has_active_video()).unwrap_or(false);
            let deps = self.total_deps();
            let desired_ms = if has_video || deps.contains(DirtyFlags::POINTER) {
                Some(33u32)
            } else if deps.intersects(DirtyFlags::TIME_SECOND.union(DirtyFlags::SYSTEM_METRICS).union(DirtyFlags::MEDIA)) {
                Some(1000u32)
            } else if deps.contains(DirtyFlags::TIME_MINUTE) {
                let now = Local::now();
                let ms_to_next_min = ((60 - now.second()) * 1000)
                    .saturating_sub(now.nanosecond() / 1_000_000)
                    .max(250);
                Some(ms_to_next_min)
            } else {
                None
            };

            if desired_ms != self.active_tick_ms {
                if self.active_tick_ms.is_some() {
                    let _ = KillTimer(self.host_hwnd, TIMER_TICK_ID);
                }
                if let Some(ms) = desired_ms {
                    let _ = SetTimer(self.host_hwnd, TIMER_TICK_ID, ms, None);
                }
                self.active_tick_ms = desired_ms;
            }
        }
    }

    fn drain_ipc(&mut self) {
        while let Ok(env) = self.ipc_rx.try_recv() {
            let response = self.handle_ipc(env.req);
            let _ = env.reply.send(response);
        }
    }

    fn handle_ipc(&mut self, req: IpcRequest) -> IpcResponse {
        match req {
            IpcRequest::Ping => IpcResponse::Pong {
                protocol_version: MANIFEST_VERSION,
            },
            IpcRequest::GetStatus => IpcResponse::Status {
                engine: EngineStatus {
                    running: true,
                    paused: self.paused,
                    pause_reason: self.pause_reason.clone(),
                    active_wallpaper_id: self.wallpaper_scene.as_ref().map(|s| s.id.clone()),
                    pinned_widget_ids: self.widgets.keys().cloned().collect(),
                    active_monitor_count: 1,
                    rendered_frames: self.rendered_frames,
                    gpu_adapter: self
                        .gpu
                        .as_ref()
                        .map(|g| g.adapter_name.clone())
                        .unwrap_or_else(|| "Unavailable".to_string()),
                },
            },
            IpcRequest::LoadScene { scene, .. } => {
                self.eval_ctx.globals = scene.globals_map();
                self.wallpaper_deps = FxEngine::collect_scene_dependencies(&scene, &self.eval_ctx);
                self.wallpaper_scene = Some(*scene);
                self.pending_dirty.insert(DirtyFlags::FULL_REBUILD);
                self.update_visibility_and_timers();
                self.render_frame_if_dirty();
                IpcResponse::Ok
            }
            IpcRequest::PatchLayer { scene_id, layer } => {
                let mut patched = false;
                if let Some(scene) = self.wallpaper_scene.as_mut().filter(|s| s.id == scene_id) {
                    if let Some(slot) = scene.find_layer_mut(&layer.id) {
                        *slot = *layer.clone();
                        patched = true;
                    }
                    self.wallpaper_deps = FxEngine::collect_scene_dependencies(scene, &self.eval_ctx);
                } else if let Some(w) = self.widgets.get_mut(&scene_id) {
                    if let Some(slot) = w.scene.find_layer_mut(&layer.id) {
                        *slot = *layer;
                        patched = true;
                    }
                    w.deps = FxEngine::collect_scene_dependencies(&w.scene, &self.eval_ctx);
                }
                if patched {
                    self.pending_dirty.insert(DirtyFlags::GEOMETRY);
                    self.update_visibility_and_timers();
                    self.render_frame_if_dirty();
                    IpcResponse::Ok
                } else {
                    IpcResponse::Error {
                        message: "layer_not_found".to_string(),
                    }
                }
            }
            IpcRequest::SetGlobal { scene_id, name, value } => {
                if let Some(scene) = self.wallpaper_scene.as_mut().filter(|s| s.id == scene_id) {
                    scene.set_global(&name, value);
                    self.eval_ctx.globals = scene.globals_map();
                } else if let Some(w) = self.widgets.get_mut(&scene_id) {
                    w.scene.set_global(&name, value);
                }
                self.pending_dirty.insert(DirtyFlags::GLOBALS);
                self.render_frame_if_dirty();
                IpcResponse::Ok
            }
            IpcRequest::ToggleGlobal { scene_id, name } => {
                if let Some(scene) = self.wallpaper_scene.as_mut().filter(|s| s.id == scene_id) {
                    scene.toggle_global(&name);
                    self.eval_ctx.globals = scene.globals_map();
                } else if let Some(w) = self.widgets.get_mut(&scene_id) {
                    w.scene.toggle_global(&name);
                }
                self.pending_dirty.insert(DirtyFlags::GLOBALS);
                self.render_frame_if_dirty();
                IpcResponse::Ok
            }
            IpcRequest::TriggerTouch { scene_id, layer_id } => {
                if let Some(scene) = self.wallpaper_scene.as_mut().filter(|s| s.id == scene_id) {
                    if let Some(layer) = scene.find_layer(&layer_id).cloned() {
                        if !layer.touch.global_name.is_empty() {
                            scene.toggle_global(&layer.touch.global_name);
                            self.eval_ctx.globals = scene.globals_map();
                            self.pending_dirty.insert(DirtyFlags::GLOBALS);
                            self.render_frame_if_dirty();
                        }
                    }
                }
                IpcResponse::Ok
            }
            IpcRequest::ApplyPackage { package_path, .. } => {
                match import_package(&PathBuf::from(&package_path), &self.storage_dir) {
                    Ok(imported) => {
                        if imported.scene.package_type == PackageKind::Widget {
                            self.pin_widget_scene(imported.scene)
                        } else {
                            self.eval_ctx.globals = imported.scene.globals_map();
                            self.wallpaper_deps =
                                FxEngine::collect_scene_dependencies(&imported.scene, &self.eval_ctx);
                            self.wallpaper_scene = Some(imported.scene);
                            self.pending_dirty.insert(DirtyFlags::FULL_REBUILD);
                            self.update_visibility_and_timers();
                            self.render_frame_if_dirty();
                            IpcResponse::Ok
                        }
                    }
                    Err(e) => IpcResponse::Error {
                        message: format!("import_failed: {e}"),
                    },
                }
            }
            IpcRequest::PinWidget { scene } => self.pin_widget_scene(*scene),
            IpcRequest::UnpinWidget { widget_id } => {
                if let Some(w) = self.widgets.remove(&widget_id) {
                    unsafe {
                        let _ = DestroyWindow(w.hwnd);
                    }
                }
                self.update_visibility_and_timers();
                IpcResponse::Ok
            }
            IpcRequest::SetPaused { paused } => {
                self.manual_pause = paused;
                self.update_visibility_and_timers();
                IpcResponse::Ok
            }
            IpcRequest::SetStudioPreviewMode { active } => {
                self.studio_preview_mode = active;
                self.update_visibility_and_timers();
                IpcResponse::Ok
            }
            IpcRequest::ClearDesktop { .. } => IpcResponse::Error {
                message: "native_mode_only: inicie o engine com --native".to_string(),
            },
            IpcRequest::Shutdown => {
                unsafe {
                    PostQuitMessage(0);
                }
                IpcResponse::Ok
            }
        }
    }

    fn pin_widget_scene(&mut self, scene: PrismScene) -> IpcResponse {
        let id = scene.id.clone();
        if let Some(old) = self.widgets.remove(&id) {
            unsafe {
                let _ = DestroyWindow(old.hwnd);
            }
        }
        let Some(hwnd) = win32_host::create_widget_window(self.hinstance, &scene.title, &scene.widget)
        else {
            return IpcResponse::Error {
                message: "create_widget_window_failed".to_string(),
            };
        };
        let gpu = gpu_renderer::GpuRenderer::new(
            hwnd,
            scene.widget.width_px.max(120),
            scene.widget.height_px.max(80),
        );
        let deps = FxEngine::collect_scene_dependencies(&scene, &self.eval_ctx);
        self.widgets.insert(id, PinnedWidget { scene, hwnd, gpu, deps });
        self.pending_dirty.insert(DirtyFlags::FULL_REBUILD);
        self.update_visibility_and_timers();
        self.render_frame_if_dirty();
        IpcResponse::Ok
    }

    fn render_frame_if_dirty(&mut self) {
        if self.paused || self.pending_dirty.is_empty() {
            return;
        }
        let prev_ptr = self.eval_ctx.pointer_norm;
        let prev_min = self.eval_ctx.now.minute();
        let prev_cpu = self.eval_ctx.system.cpu_percent;
        let prev_ram = self.eval_ctx.system.ram_percent;
        let prev_bat = self.eval_ctx.system.battery_percent;
        let is_full = self.pending_dirty.intersects(
            DirtyFlags::FULL_REBUILD
                .union(DirtyFlags::GEOMETRY)
                .union(DirtyFlags::GLOBALS)
                .union(DirtyFlags::BACKGROUND),
        );

        let needs_frost = self
            .wallpaper_scene
            .as_ref()
            .map(|sc| {
                sc.flatten_layers().iter().any(|(l, _)| {
                    !l.hidden && matches!(l.fill, FillMode::OpticalCrystal | FillMode::ClassicBlur)
                })
            })
            .unwrap_or(false);
        let video_ticked = self
            .gpu
            .as_mut()
            .map(|g| g.poll_video_frame(needs_frost))
            .unwrap_or(false);

        self.refresh_system_telemetry();
        let deps = self.total_deps();
        if deps.contains(DirtyFlags::MEDIA) {
            gpu_renderer::poll_gsmtc_media(&mut self.eval_ctx);
        }
        if deps.contains(DirtyFlags::POINTER) {
            gpu_renderer::poll_pointer_norm(&mut self.eval_ctx);
        }

        let ptr_moved = (self.eval_ctx.pointer_norm[0] - prev_ptr[0]).abs() > 0.001
            || (self.eval_ctx.pointer_norm[1] - prev_ptr[1]).abs() > 0.001;
        let telemetry_changed = self.eval_ctx.now.minute() != prev_min
            || (self.eval_ctx.system.cpu_percent - prev_cpu).abs() >= 0.5
            || (self.eval_ctx.system.ram_percent - prev_ram).abs() >= 1.0
            || self.eval_ctx.system.battery_percent != prev_bat;

        if !is_full && !video_ticked && self.rendered_frames > 0 && !ptr_moved && !telemetry_changed {
            self.pending_dirty = DirtyFlags::NONE;
            return;
        }

        if let (Some(gpu), Some(scene)) = (self.gpu.as_mut(), self.wallpaper_scene.as_ref()) {
            if gpu.render_scene(scene, &self.eval_ctx) {
                self.rendered_frames = self.rendered_frames.saturating_add(1);
            }
        }
        for widget in self.widgets.values_mut() {
            if let Some(wgpu_surface) = widget.gpu.as_mut() {
                let mut w_ctx = self.eval_ctx.clone();
                w_ctx.globals = widget.scene.globals_map();
                if wgpu_surface.render_scene(&widget.scene, &w_ctx) {
                    self.rendered_frames = self.rendered_frames.saturating_add(1);
                }
            }
        }
        self.pending_dirty = DirtyFlags::NONE;
    }
}

thread_local! {
    static RUNTIME: RefCell<Option<EngineRuntime>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PRISM_WAKE => {
            RUNTIME.with(|cell| {
                if let Some(rt) = cell.borrow_mut().as_mut() {
                    rt.drain_ipc();
                }
            });
            LRESULT(0)
        }
        WM_PRISM_VIDEO_FRAME => {
            RUNTIME.with(|cell| {
                if let Some(rt) = cell.borrow_mut().as_mut() {
                    if !rt.paused {
                        rt.pending_dirty.insert(DirtyFlags::BACKGROUND);
                        rt.render_frame_if_dirty();
                    }
                }
            });
            LRESULT(0)
        }
        WM_TIMER => {
            RUNTIME.with(|cell| {
                if let Some(rt) = cell.borrow_mut().as_mut() {
                    if wparam.0 == TIMER_VISIBILITY_ID {
                        rt.update_visibility_and_timers();
                    } else if wparam.0 == TIMER_TICK_ID {
                        let deps = rt.total_deps();
                        rt.pending_dirty.insert(deps);
                        if rt.gpu.as_ref().map(|g| g.has_active_video()).unwrap_or(false) {
                            rt.pending_dirty.insert(DirtyFlags::BACKGROUND);
                        }
                        rt.update_visibility_and_timers();
                        rt.render_frame_if_dirty();
                    }
                }
            });
            LRESULT(0)
        }
        WM_WTSSESSION_CHANGE => {
            RUNTIME.with(|cell| {
                if let Some(rt) = cell.borrow_mut().as_mut() {
                    match wparam.0 as u32 {
                        WTS_SESSION_LOCK => rt.session_locked = true,
                        WTS_SESSION_UNLOCK => {
                            rt.session_locked = false;
                            rt.pending_dirty.insert(DirtyFlags::TIME_MINUTE);
                        }
                        _ => {}
                    }
                    rt.update_visibility_and_timers();
                    rt.render_frame_if_dirty();
                }
            });
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            RUNTIME.with(|cell| {
                if let Some(rt) = cell.borrow_mut().as_mut() {
                    let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1280) as u32;
                    let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(720) as u32;
                    let _ = SetWindowPos(hwnd, HWND_TOP, 0, 0, w as i32, h as i32, SWP_NOACTIVATE);
                    if let Some(gpu) = rt.gpu.as_mut() {
                        gpu.resize(w, h);
                    }
                    rt.pending_dirty.insert(DirtyFlags::FULL_REBUILD);
                    rt.render_frame_if_dirty();
                }
            });
            LRESULT(0)
        }
        WM_PRISM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP {
                let paused = RUNTIME.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|rt| rt.manual_pause || rt.paused)
                        .unwrap_or(false)
                });
                tray::show_context_menu(hwnd, paused);
            } else if event == WM_LBUTTONDBLCLK {
                tray::launch_studio();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd = wparam.0 & 0xFFFF;
            match cmd {
                CMD_OPEN_STUDIO => tray::launch_studio(),
                CMD_TOGGLE_PAUSE => {
                    RUNTIME.with(|cell| {
                        if let Some(rt) = cell.borrow_mut().as_mut() {
                            rt.manual_pause = !rt.manual_pause;
                            rt.update_visibility_and_timers();
                            rt.render_frame_if_dirty();
                        }
                    });
                }
                CMD_EXIT => PostQuitMessage(0),
                _ => {}
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            tray::remove_tray_icon(hwnd);
            let _ = WTSUnRegisterSessionNotification(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn main() {
    // Motor nativo (wgpu + platform/): `prismview-engine.exe --native`.
    if std::env::args().skip(1).any(|a| a == "--native") {
        std::process::exit(native::run());
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let hinstance = unsafe { HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap_or_default().0) };
    win32_host::register_window_classes(hinstance, wnd_proc);
    let Some(host_hwnd) = win32_host::create_wallpaper_host(hinstance) else {
        return;
    };
    tray::add_tray_icon(host_hwnd);

    let (ipc_tx, ipc_rx) = mpsc::channel();
    let wake_hwnd = host_hwnd.0 as isize;
    ipc_server::spawn_named_pipe_server(
        move || unsafe {
            let _ = PostMessageW(
                HWND(wake_hwnd as *mut _),
                WM_PRISM_WAKE,
                WPARAM(0),
                LPARAM(0),
            );
        },
        ipc_tx,
    );

    RUNTIME.with(|cell| {
        let mut rt = EngineRuntime::new(hinstance, host_hwnd, ipc_rx);
        rt.update_visibility_and_timers();
        rt.render_frame_if_dirty();
        *cell.borrow_mut() = Some(rt);
    });

    unsafe {
        let _ = SetTimer(host_hwnd, TIMER_VISIBILITY_ID, 750, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
