use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::{Interface, HSTRING};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Graphics::DirectX::Direct3D11::IDirect3DSurface;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11SurfaceFromDXGISurface;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub struct WinRtVideoPlayer {
    pub source_key: String,
    player: MediaPlayer,
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    gpu_tex: ID3D11Texture2D,
    staging_tex: ID3D11Texture2D,
    winrt_surface: IDirect3DSurface,
    width: u32,
    height: u32,
    frame_ready: Arc<AtomicBool>,
    paused: bool,
    rgba_scratch: Vec<u8>,
}

fn path_to_file_uri(path: &Path) -> String {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let raw = abs.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    let forward = stripped.replace('\\', "/");
    let mut encoded = String::with_capacity(forward.len() + 16);
    encoded.push_str("file:///");
    for b in forward.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

fn create_d3d11_device() -> Option<(ID3D11Device, ID3D11DeviceContext)> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        let hr = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            None,
            flags,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        );
        if hr.is_err() {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                None,
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .ok()?;
        }
        Some((device?, context?))
    }
}

fn create_surface_pair(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Option<(ID3D11Texture2D, ID3D11Texture2D, IDirect3DSurface)> {
    let w = width.clamp(64, 1920);
    let h = height.clamp(64, 1080);
    unsafe {
        let gpu_desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut gpu_tex: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&gpu_desc, None, Some(&mut gpu_tex)).ok()?;
        let gpu_tex = gpu_tex?;

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..gpu_desc
        };
        let mut staging_tex: Option<ID3D11Texture2D> = None;
        device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging_tex))
            .ok()?;
        let staging_tex = staging_tex?;

        let dxgi_surface: IDXGISurface = gpu_tex.cast().ok()?;
        let inspectable = CreateDirect3D11SurfaceFromDXGISurface(&dxgi_surface).ok()?;
        let winrt_surface: IDirect3DSurface = inspectable.cast().ok()?;
        Some((gpu_tex, staging_tex, winrt_surface))
    }
}

impl WinRtVideoPlayer {
    pub fn open(
        video_path: &Path,
        target_w: u32,
        target_h: u32,
        muted: bool,
        wake_hwnd: isize,
        wake_msg: u32,
    ) -> Option<Self> {
        if !video_path.is_file() {
            return None;
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        let (d3d_device, d3d_context) = create_d3d11_device()?;
        let w = target_w.clamp(64, 1920);
        let h = target_h.clamp(64, 1080);
        let (gpu_tex, staging_tex, winrt_surface) = create_surface_pair(&d3d_device, w, h)?;

        let player = MediaPlayer::new().ok()?;
        player.SetIsVideoFrameServerEnabled(true).ok()?;
        player.SetIsLoopingEnabled(true).ok()?;
        player.SetIsMuted(muted).ok()?;
        let _ = player.SetVolume(if muted { 0.0 } else { 0.85 });

        let uri_str = path_to_file_uri(video_path);
        let uri = Uri::CreateUri(&HSTRING::from(uri_str)).ok()?;
        let source = MediaSource::CreateFromUri(&uri).ok()?;
        player.SetSource(&source).ok()?;

        let frame_ready = Arc::new(AtomicBool::new(false));
        let ready_clone = Arc::clone(&frame_ready);
        let handler = TypedEventHandler::new(move |_sender, _args| {
            ready_clone.store(true, Ordering::Release);
            if wake_hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(
                        HWND(wake_hwnd as *mut core::ffi::c_void),
                        wake_msg,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            Ok(())
        });
        let _ = player.VideoFrameAvailable(&handler);
        let _ = player.Play();

        Some(Self {
            source_key: video_path.to_string_lossy().to_string(),
            player,
            d3d_device,
            d3d_context,
            gpu_tex,
            staging_tex,
            winrt_surface,
            width: w,
            height: h,
            frame_ready,
            paused: false,
            rgba_scratch: vec![0u8; (w * h * 4) as usize],
        })
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if paused {
            let _ = self.player.Pause();
        } else {
            let _ = self.player.Play();
        }
    }

    fn sync_natural_dimensions(&mut self) {
        let Ok(session) = self.player.PlaybackSession() else {
            return;
        };
        let nw = session.NaturalVideoWidth().unwrap_or(0);
        let nh = session.NaturalVideoHeight().unwrap_or(0);
        if nw < 64 || nh < 64 {
            return;
        }
        let clamped_w = nw.min(1920);
        let clamped_h = nh.min(1080);
        if clamped_w != self.width || clamped_h != self.height {
            if let Some((gt, st, ws)) = create_surface_pair(&self.d3d_device, clamped_w, clamped_h) {
                self.gpu_tex = gt;
                self.staging_tex = st;
                self.winrt_surface = ws;
                self.width = clamped_w;
                self.height = clamped_h;
                self.rgba_scratch.resize((clamped_w * clamped_h * 4) as usize, 0);
            }
        }
    }

    pub fn poll_rgba_frame(&mut self) -> Option<(&[u8], u32, u32)> {
        if self.paused || !self.frame_ready.swap(false, Ordering::AcqRel) {
            return None;
        }
        self.sync_natural_dimensions();

        // Hardware GPU copy from MediaPlayer Frame Server to D3D11 surface
        if self.player.CopyFrameToVideoSurface(&self.winrt_surface).is_err() {
            // Fallback via SoftwareBitmap RGBA8 if D3D11 surface copy is transiently busy
            let _sb = SoftwareBitmap::Create(
                BitmapPixelFormat::Rgba8,
                self.width as i32,
                self.height as i32,
            );
            let _ = BitmapAlphaMode::Premultiplied;
            return None;
        }

        unsafe {
            self.d3d_context
                .CopyResource(&self.staging_tex, &self.gpu_tex);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if self
                .d3d_context
                .Map(&self.staging_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .is_err()
            {
                return None;
            }
            let pitch = mapped.RowPitch as usize;
            let src_ptr = mapped.pData as *const u8;
            let w = self.width as usize;
            let h = self.height as usize;

            if !src_ptr.is_null() && pitch >= w * 4 {
                for y in 0..h {
                    let row_src = std::slice::from_raw_parts(src_ptr.add(y * pitch), w * 4);
                    let row_dst = &mut self.rgba_scratch[y * w * 4..(y + 1) * w * 4];
                    for x in 0..w {
                        let i = x * 4;
                        // BGRA8 -> RGBA8
                        row_dst[i] = row_src[i + 2];
                        row_dst[i + 1] = row_src[i + 1];
                        row_dst[i + 2] = row_src[i];
                        row_dst[i + 3] = 255;
                    }
                }
            }
            self.d3d_context.Unmap(&self.staging_tex, 0);
        }

        Some((&self.rgba_scratch, self.width, self.height))
    }
}

impl Drop for WinRtVideoPlayer {
    fn drop(&mut self) {
        let _ = self.player.Pause();
        let _ = self.player.Close();
    }
}
