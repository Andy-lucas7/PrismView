//! Contexto wgpu do motor nativo: DX12 + `PowerPreference::LowPower`.
//!
//! - `GpuContext`: instância, adapter, device e queue (1 por processo).
//! - `WindowSurface`: swapchain de uma janela de wallpaper (1 por monitor).
//!
//! Regras (PLANO_NATIVO): zero `unwrap/expect`; `SurfaceError::Lost/Outdated` nunca derrubam o
//! loop: a superfície é reconfigurada ou recriada e o resultado vem em `FrameStatus` para o
//! chamador decidir (repetir, refazer a janela, refazer o device). Render targets só são
//! recriados em resize/recriação. Este módulo não conhece Win32.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

// ───────────────────────── Erros ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuError {
    /// Nenhum adapter DX12 compatível (nem o de software).
    NoAdapter,
    /// Falha ao criar/configurar a superfície da janela.
    Surface(String),
    /// Falha ao criar o device.
    Device(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter => write!(f, "nenhum adapter DX12 compatível"),
            Self::Surface(m) => write!(f, "superfície: {m}"),
            Self::Device(m) => write!(f, "device: {m}"),
        }
    }
}

impl std::error::Error for GpuError {}

// ───────────────────────── Handles nativos ─────────────────────────

/// Par de handles brutos da janela. Quem cria deve garantir que a janela vive mais que a
/// `WindowSurface` (soltar a superfície antes de destruir a janela).
#[derive(Debug, Clone, Copy)]
pub struct NativeHandles {
    pub window: RawWindowHandle,
    pub display: RawDisplayHandle,
}

impl NativeHandles {
    fn target(&self) -> wgpu::SurfaceTargetUnsafe {
        wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: self.display,
            raw_window_handle: self.window,
        }
    }
}

// ───────────────────────── GpuContext ─────────────────────────

pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    device_lost: Arc<AtomicBool>,
    /// Erro de validação/driver capturado pelo handler (em vez do panic padrão do wgpu).
    uncaptured_error: Arc<AtomicBool>,
}

impl GpuContext {
    /// Cria a instância DX12, a primeira superfície e o adapter `LowPower` compatível com ela
    /// (cai para o adapter de software se não houver GPU compatível).
    pub fn new(
        handles: NativeHandles,
        width: u32,
        height: u32,
    ) -> Result<(Self, WindowSurface), GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..Default::default()
        });

        // SAFETY: o chamador garante que a janela vive enquanto a superfície existir.
        let surface = unsafe { instance.create_surface_unsafe(handles.target()) }
            .map_err(|e| GpuError::Surface(e.to_string()))?;

        let adapter = Self::pick_adapter(&instance, &surface)?;
        let adapter_name = adapter.get_info().name;

        // BC7 (texturas KTX2/UASTC transcodificadas) só se o adapter suportar; o resto é core.
        let required_features = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("prismview_dx12_device"),
                required_features,
                ..Default::default()
            },
            None,
        ))
        .map_err(|e| GpuError::Device(e.to_string()))?;

        let device_lost = Arc::new(AtomicBool::new(false));
        let uncaptured_error = Arc::new(AtomicBool::new(false));
        {
            let flag = Arc::clone(&device_lost);
            device.set_device_lost_callback(move |_reason, _message| {
                flag.store(true, Ordering::SeqCst);
            });
            let flag = Arc::clone(&uncaptured_error);
            device.on_uncaptured_error(Box::new(move |error| {
                eprintln!("[prismview-engine] wgpu: {error}");
                flag.store(true, Ordering::SeqCst);
            }));
        }

        let ctx = Self {
            instance,
            adapter,
            device,
            queue,
            adapter_name,
            device_lost,
            uncaptured_error,
        };
        let window_surface = WindowSurface::configure_new(&ctx, surface, handles, width, height)?;
        Ok((ctx, window_surface))
    }

    fn pick_adapter(
        instance: &wgpu::Instance,
        surface: &wgpu::Surface<'_>,
    ) -> Result<wgpu::Adapter, GpuError> {
        for force_fallback_adapter in [false, true] {
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(surface),
                force_fallback_adapter,
            }));
            if let Some(adapter) = adapter {
                return Ok(adapter);
            }
        }
        Err(GpuError::NoAdapter)
    }

    /// Superfície para outra janela (outro monitor) no mesmo device.
    pub fn create_surface(
        &self,
        handles: NativeHandles,
        width: u32,
        height: u32,
    ) -> Result<WindowSurface, GpuError> {
        let surface = self.raw_surface(&handles)?;
        WindowSurface::configure_new(self, surface, handles, width, height)
    }

    fn raw_surface(&self, handles: &NativeHandles) -> Result<wgpu::Surface<'static>, GpuError> {
        // SAFETY: ver `new`; o chamador mantém a janela viva.
        unsafe { self.instance.create_surface_unsafe(handles.target()) }
            .map_err(|e| GpuError::Surface(e.to_string()))
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// O device aceita texturas BC (usar em `LoaderConfig::allow_bc7`).
    pub fn supports_bc(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    /// O driver removeu/resetou o device (TDR, GPU desligada): recriar o `GpuContext`
    /// e todas as superfícies.
    pub fn is_device_lost(&self) -> bool {
        self.device_lost.load(Ordering::SeqCst)
    }

    /// Lê e zera a flag de erro capturado (configure em janela inválida, etc.).
    fn take_uncaptured_error(&self) -> bool {
        self.uncaptured_error.swap(false, Ordering::SeqCst)
    }
}

// ───────────────────────── WindowSurface ─────────────────────────

/// Resultado de tentar apresentar um frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStatus {
    /// Frame apresentado.
    Presented,
    /// Sem frame desta vez (timeout, swapchain reconfigurada/recriada). Tentar de novo logo.
    Retry,
    /// A superfície não pôde ser recriada (janela destruída?): refazer janela + superfície.
    Unavailable,
    /// Device perdido ou sem memória: refazer `GpuContext` e todas as superfícies.
    DeviceLost,
}

pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    handles: NativeHandles,
    /// Reconfigurar antes do próximo frame (resize, Outdated, suboptimal).
    stale: bool,
}

impl WindowSurface {
    fn configure_new(
        gpu: &GpuContext,
        surface: wgpu::Surface<'static>,
        handles: NativeHandles,
        width: u32,
        height: u32,
    ) -> Result<Self, GpuError> {
        let caps = surface.get_capabilities(&gpu.adapter);
        // Sem sRGB: o clear/pipeline escreve valores já na codificação da tela.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| GpuError::Surface("superfície sem formatos suportados".into()))?;
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes
                .first()
                .copied()
                .ok_or_else(|| GpuError::Surface("superfície sem alpha modes".into()))?
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: Vec::new(),
        };
        let mut this = Self {
            surface,
            config,
            handles,
            stale: false,
        };
        if !this.reconfigure(gpu) {
            return Err(GpuError::Surface("falha ao configurar a swapchain".into()));
        }
        Ok(this)
    }

    /// Aplica `config`. `false` se o driver/janela rejeitou (erro capturado).
    fn reconfigure(&mut self, gpu: &GpuContext) -> bool {
        gpu.take_uncaptured_error();
        self.surface.configure(&gpu.device, &self.config);
        self.stale = false;
        !gpu.take_uncaptured_error()
    }

    /// Recria a superfície a partir dos handles (após `SurfaceError::Lost`).
    fn recreate(&mut self, gpu: &GpuContext) -> bool {
        match gpu.raw_surface(&self.handles) {
            Ok(surface) => {
                self.surface = surface; // a antiga é solta aqui
                self.reconfigure(gpu)
            }
            Err(_) => false,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Registra novo tamanho; a swapchain é refeita no próximo frame. Ignora 0×0 (minimizada).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.config.width != width || self.config.height != height {
            self.config.width = width;
            self.config.height = height;
            self.stale = true;
        }
    }

    /// Adquire o frame, deixa `record` gravar comandos e apresenta. Nunca dá panic:
    /// `Outdated` reconfigura, `Lost` recria a superfície, ambos pedem `Retry`.
    pub fn render_with<F>(&mut self, gpu: &GpuContext, record: F) -> FrameStatus
    where
        F: FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
    {
        if gpu.is_device_lost() {
            return FrameStatus::DeviceLost;
        }
        if self.stale && !self.reconfigure(gpu) {
            return FrameStatus::Unavailable;
        }

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated) => {
                return if self.reconfigure(gpu) {
                    FrameStatus::Retry
                } else {
                    FrameStatus::Unavailable
                };
            }
            Err(wgpu::SurfaceError::Lost) => {
                return if self.recreate(gpu) {
                    FrameStatus::Retry
                } else {
                    FrameStatus::Unavailable
                };
            }
            Err(wgpu::SurfaceError::OutOfMemory) => return FrameStatus::DeviceLost,
            // Timeout (oclusão/compositor ocupado) e erros futuros: tentar no próximo ciclo.
            Err(_) => return FrameStatus::Retry,
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("prismview_frame"),
            });
        record(&mut encoder, &view);
        gpu.queue.submit(std::iter::once(encoder.finish()));

        if frame.suboptimal {
            self.stale = true;
        }
        frame.present();
        FrameStatus::Presented
    }

    /// Pinta a superfície inteira com `color` (RGBA 0..1) e apresenta.
    pub fn clear(&mut self, gpu: &GpuContext, color: [f32; 4]) -> FrameStatus {
        let clear = wgpu::Color {
            r: f64::from(color[0]),
            g: f64::from(color[1]),
            b: f64::from(color[2]),
            a: f64::from(color[3]),
        };
        self.render_with(gpu, |encoder, view| {
            // O pass só existe para executar o LoadOp::Clear.
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prismview_clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        })
    }
}
