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

/// Apresentação fora da swapchain do wgpu. O wgpu 24/DX12 só expõe `CompositeAlphaMode::Opaque`,
/// então widgets transparentes desenham numa textura própria e o host (`platform/windows`, via
/// DirectComposition + `DXGI_ALPHA_MODE_PREMULTIPLIED`) copia na GPU para o backbuffer e apresenta.
/// Nunca há leitura para a CPU.
pub trait ExternalPresenter {
    /// `texture` acabou de ser renderizada (último uso: color target) e submetida na fila do
    /// wgpu. Copia para o backbuffer da cadeia e apresenta.
    fn present(&mut self, gpu: &GpuContext, texture: &wgpu::Texture) -> FrameStatus;
    /// Novo tamanho dos buffers (px). `false` = cadeia inutilizável (refazer a janela).
    fn resize(&mut self, gpu: &GpuContext, width: u32, height: u32) -> bool;
}

/// Formato da textura de saída de superfícies externas (= backbuffer B8G8R8A8 do DXGI).
pub const EXTERNAL_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

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
        // Sem layer de validação D3D12 por padrão (economiza ~40–60 MB mesmo em builds debug;
        // pode ser reativada sob demanda via `WGPU_VALIDATION=1`).
        let instance = Self::create_instance();

        // SAFETY: o chamador garante que a janela vive enquanto a superfície existir.
        let surface = unsafe { instance.create_surface_unsafe(handles.target()) }
            .map_err(|e| GpuError::Surface(e.to_string()))?;

        let adapter = Self::pick_adapter(&instance, Some(&surface))?;
        let ctx = Self::with_adapter(instance, adapter)?;
        let window_surface = WindowSurface::configure_new(&ctx, surface, handles, width, height)?;
        Ok((ctx, window_surface))
    }

    /// Device sem superfície (só widgets nativos no ar; o wallpaper pode nunca existir).
    pub fn new_headless() -> Result<Self, GpuError> {
        let instance = Self::create_instance();
        let adapter = Self::pick_adapter(&instance, None)?;
        Self::with_adapter(instance, adapter)
    }

    fn create_instance() -> wgpu::Instance {
        wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            flags: wgpu::InstanceFlags::empty().with_env(),
            ..Default::default()
        })
    }

    fn with_adapter(instance: wgpu::Instance, adapter: wgpu::Adapter) -> Result<Self, GpuError> {
        let adapter_name = adapter.get_info().name;

        // BC7 (texturas KTX2/UASTC transcodificadas) só se o adapter suportar; o resto é core.
        let required_features = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("prismview_dx12_device"),
                required_features,
                // Blocos de subalocação mínimos (8 MB device / 4 MB host em vez de 256 MB / 64 MB).
                memory_hints: wgpu::MemoryHints::MemoryUsage,
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

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            adapter_name,
            device_lost,
            uncaptured_error,
        })
    }

    fn pick_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<wgpu::Adapter, GpuError> {
        for force_fallback_adapter in [false, true] {
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: surface,
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

    /// Superfície que desenha numa textura própria e entrega a `presenter` (widgets transparentes).
    pub fn create_external_surface(
        &self,
        presenter: Box<dyn ExternalPresenter>,
        width: u32,
        height: u32,
    ) -> WindowSurface {
        WindowSurface {
            backend: SurfaceBackend::External {
                presenter,
                target: None,
            },
            config: wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                format: EXTERNAL_SURFACE_FORMAT,
                width: width.max(1),
                height: height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 1,
                alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
                view_formats: Vec::new(),
            },
            stale: true,
        }
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
    #[allow(dead_code)] // API reservada (ainda sem chamador)
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

enum SurfaceBackend {
    /// Swapchain do próprio wgpu (wallpaper: janela filha do WorkerW, alpha opaco).
    Wgpu {
        surface: wgpu::Surface<'static>,
        handles: NativeHandles,
    },
    /// Textura própria + `ExternalPresenter` (widgets transparentes, DirectComposition).
    External {
        presenter: Box<dyn ExternalPresenter>,
        target: Option<(wgpu::Texture, wgpu::TextureView)>,
    },
}

pub struct WindowSurface {
    backend: SurfaceBackend,
    config: wgpu::SurfaceConfiguration,
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
            // Double-buffer (2 backbuffers em vez de 3): economiza 1 framebuffer inteiro por monitor.
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: Vec::new(),
        };
        let mut this = Self {
            backend: SurfaceBackend::Wgpu { surface, handles },
            config,
            stale: false,
        };
        if !this.reconfigure(gpu) {
            return Err(GpuError::Surface("falha ao configurar a swapchain".into()));
        }
        Ok(this)
    }

    /// Aplica `config`. `false` se o driver/janela rejeitou (erro capturado).
    fn reconfigure(&mut self, gpu: &GpuContext) -> bool {
        let (w, h) = (self.config.width, self.config.height);
        match &mut self.backend {
            SurfaceBackend::Wgpu { surface, .. } => {
                gpu.take_uncaptured_error();
                surface.configure(&gpu.device, &self.config);
                self.stale = false;
                !gpu.take_uncaptured_error()
            }
            SurfaceBackend::External { presenter, target } => {
                // Alvo só recriado em resize (regra: render targets fora do caminho por quadro).
                // O presenter espera a cópia pendente (que lê o alvo antigo) antes de soltá-lo.
                if !presenter.resize(gpu, w, h) {
                    return false;
                }
                *target = None;
                gpu.take_uncaptured_error();
                let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("prismview_widget_target"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: EXTERNAL_SURFACE_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                if gpu.take_uncaptured_error() {
                    return false;
                }
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                *target = Some((texture, view));
                self.stale = false;
                true
            }
        }
    }

    /// Recria a superfície a partir dos handles (após `SurfaceError::Lost`).
    fn recreate(&mut self, gpu: &GpuContext) -> bool {
        let SurfaceBackend::Wgpu { surface, handles } = &mut self.backend else {
            return self.reconfigure(gpu);
        };
        match gpu.raw_surface(handles) {
            Ok(fresh) => {
                *surface = fresh; // a antiga é solta aqui
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

    /// Superfície com alpha por pixel real (widget DirectComposition).
    pub fn is_transparent(&self) -> bool {
        matches!(self.backend, SurfaceBackend::External { .. })
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

        if let SurfaceBackend::External { presenter, target } = &mut self.backend {
            let Some((texture, view)) = target.as_ref() else {
                return FrameStatus::Unavailable;
            };
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("prismview_widget_frame"),
                });
            record(&mut encoder, view);
            gpu.queue.submit(std::iter::once(encoder.finish()));
            return presenter.present(gpu, texture);
        }
        let SurfaceBackend::Wgpu { surface, .. } = &mut self.backend else {
            return FrameStatus::Unavailable;
        };

        let frame = match surface.get_current_texture() {
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
