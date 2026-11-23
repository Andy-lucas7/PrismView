//! `MfVideoDecoder`: vídeo por hardware (Media Foundation) entregue direto como `wgpu::Texture`.
//!
//! Pipeline (zero cópia na CPU, sem conversão de cor na CPU):
//!
//! ```text
//!  arquivo ─► IMFSourceReader (MFT de decode HW, DXVA/D3D11)
//!          ─► video processor do MF na GPU (NV12/P010 → ARGB32 = DXGI B8G8R8A8)
//!          ─► CopySubresourceRegion (GPU→GPU, D3D11) para um slot do anel
//!               └ slot = ID3D11Texture2D MISC_SHARED | SHARED_NTHANDLE
//!          ─► ID3D12Device::OpenSharedHandle ─► wgpu-hal dx12 ─► wgpu::Texture (Bgra8Unorm)
//! ```
//!
//! - O device D3D11 é criado no **mesmo adapter físico** do device D3D12 do wgpu (LUID), com
//!   `VIDEO_SUPPORT` e proteção multithread (o MF usa o device das suas próprias threads).
//! - Uma thread de trabalho por vídeo decodifica à frente, preenchendo um anel de `RING_SLOTS`
//!   texturas. A cópia D3D11 é concluída (fence + evento) antes de o slot virar `Ready`, então
//!   o D3D12 sempre lê dados completos.
//! - A thread de render chama `advance()` 1× por frame: escolhe o slot cujo PTS já chegou no
//!   relógio de reprodução e devolve slots antigos ao worker após `RETIRE_TICKS` frames (com
//!   `desired_maximum_frame_latency = 2`, a GPU já terminou de amostrá-los).
//! - Nada é alocado por frame: o anel é criado no `open` (recriado só em troca de vídeo).
//!
//! Alpha: o video processor pode entregar ARGB32 com alpha 0. O vídeo é opaco: quem desenha
//! deve amostrar forçando alpha = 1.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ::windows::core::{Interface, GUID, PCWSTR, PROPVARIANT};
use ::windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HMODULE, WAIT_OBJECT_0};
use ::windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use ::windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11Multithread, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BOX, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_FENCE_FLAG_NONE, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use ::windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Resource};
use ::windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use ::windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory4, IDXGIResource1, DXGI_SHARED_RESOURCE_READ,
    DXGI_SHARED_RESOURCE_WRITE,
};
use ::windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFAttributes, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFSample, IMFSourceReader,
    MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateMediaType, MFCreateSourceReaderFromURL,
    MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx, MFVideoFormat_ARGB32, MFVideoFormat_AV1,
    MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_VP90, MFSTARTUP_NOSOCKET,
    MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_REGISTER_TYPE_INFO, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_D3D_MANAGER, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE, MF_VERSION,
};
use ::windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use ::windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::util::os_error;
use crate::platform::{
    DecodedFrame, GpuVideoFrame, HardwareVideoDecoder, PlatformError, PlatformResult, VideoCodec,
    VideoInfo,
};

// ───────────────────────── Constantes ─────────────────────────

/// Slots do anel: 1 exibido + até `RETIRE_TICKS` aguardando a GPU + folga para decodificar à
/// frente. 4K BGRA ≈ 33 MB de VRAM por slot.
const RING_SLOTS: usize = 5;
/// Frames renderizados até um slot trocado poder ser reescrito pelo D3D11.
const RETIRE_TICKS: u64 = 2;
/// Formato das texturas entregues (MF ARGB32 = DXGI B8G8R8A8_UNORM).
pub const VIDEO_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
const FENCE_TIMEOUT_MS: u32 = 2000;
const WORKER_IDLE_WAIT: Duration = Duration::from_millis(100);
const DEFAULT_FRAME_MS: u64 = 33;

const FIRST_VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIA_SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;
const FLAG_ERROR: u32 = MF_SOURCE_READERF_ERROR.0 as u32;
const FLAG_END_OF_STREAM: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
const FLAG_TYPE_CHANGED: u32 = MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32;

// ───────────────────────── Estado compartilhado (render ↔ worker) ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotState {
    /// Livre para o worker escrever.
    Free,
    /// O worker está copiando para ele.
    Writing,
    /// Cópia concluída na GPU; aguardando a hora de exibir.
    Ready,
    /// Frame exibido atualmente (a GPU do render lê dele).
    Shown,
    /// Saiu da tela no tick `n - RETIRE_TICKS`; vira `Free` no tick `n`.
    Retiring(u64),
}

#[derive(Clone, Copy, Debug)]
struct SlotMeta {
    state: SlotState,
    /// PTS monotônico (ms): soma a duração das voltas anteriores quando em loop.
    pts_ms: u64,
    /// Geração de seek a que o frame pertence.
    gen: u64,
}

const FREE_SLOT: SlotMeta = SlotMeta {
    state: SlotState::Free,
    pts_ms: 0,
    gen: 0,
};

struct SharedState {
    slots: [SlotMeta; RING_SLOTS],
    stop: bool,
    looping: bool,
    /// Seek pendente: (posição em ms, geração dos frames seguintes).
    seek: Option<(u64, u64)>,
    end_of_stream: bool,
    error: Option<String>,
}

struct Shared {
    state: Mutex<SharedState>,
    cv: Condvar,
}

impl Shared {
    fn new(looping: bool) -> Self {
        Self {
            state: Mutex::new(SharedState {
                slots: [FREE_SLOT; RING_SLOTS],
                stop: false,
                looping,
                seek: None,
                end_of_stream: false,
                error: None,
            }),
            cv: Condvar::new(),
        }
    }

    /// Lock tolerante a poison (uma thread que panicou não derruba a outra).
    fn lock(&self) -> MutexGuard<'_, SharedState> {
        self.state.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, SharedState>) -> MutexGuard<'a, SharedState> {
        match self.cv.wait_timeout(guard, WORKER_IDLE_WAIT) {
            Ok((guard, _)) => guard,
            Err(poison) => poison.into_inner().0,
        }
    }
}

// ───────────────────────── Handles Win32 ─────────────────────────

/// NT handles dos slots compartilhados, como `isize` para atravessar threads. Fecha no `Drop`
/// os que não foram consumidos (ex.: `open` desistiu por timeout).
struct SharedHandles(Vec<isize>);

impl SharedHandles {
    fn take(&mut self, index: usize) -> Option<HANDLE> {
        let raw = self.0.get_mut(index).map(|h| std::mem::replace(h, 0))?;
        (raw != 0).then_some(HANDLE(raw as *mut c_void))
    }
}

impl Drop for SharedHandles {
    fn drop(&mut self) {
        for &raw in &self.0 {
            if raw != 0 {
                let _ = unsafe { CloseHandle(HANDLE(raw as *mut c_void)) };
            }
        }
    }
}

struct OwnedEvent(HANDLE);

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// `CoInitializeEx(MTA)` com `CoUninitialize` pareado (só se a inicialização contou).
struct ComScope(bool);

impl ComScope {
    fn enter() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self(hr.is_ok())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// `MFShutdown` pareado com um `MFStartup` bem-sucedido (ambos contam referências).
struct MfScope;

impl MfScope {
    fn start() -> Result<Self, String> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
            .map(|()| Self)
            .map_err(|e| format!("MFStartup: {e}"))
    }
}

impl Drop for MfScope {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

fn ctx<T>(result: ::windows::core::Result<T>, what: &str) -> Result<T, String> {
    result.map_err(|e| format!("{what}: {e}"))
}

// ───────────────────────── Codecs ─────────────────────────

fn subtype_of(codec: VideoCodec) -> GUID {
    match codec {
        VideoCodec::H264 => MFVideoFormat_H264,
        VideoCodec::Hevc => MFVideoFormat_HEVC,
        VideoCodec::Vp9 => MFVideoFormat_VP90,
        VideoCodec::Av1 => MFVideoFormat_AV1,
    }
}

fn codec_from_subtype(subtype: GUID) -> Option<VideoCodec> {
    [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Vp9, VideoCodec::Av1]
        .into_iter()
        .find(|&c| subtype_of(c) == subtype)
}

/// Existe MFT de decodificação **por hardware** para `codec`?
fn hardware_decoder_available(codec: VideoCodec) -> bool {
    let _com = ComScope::enter();
    let Ok(_mf) = MfScope::start() else {
        return false;
    };
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype_of(codec),
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        let ok = MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input as *const MFT_REGISTER_TYPE_INFO),
            None,
            &mut activates,
            &mut count,
        )
        .is_ok();
        if !activates.is_null() {
            for i in 0..count as usize {
                std::ptr::drop_in_place(activates.add(i)); // Release de cada IMFActivate
            }
            CoTaskMemFree(Some(activates as *const c_void));
        }
        ok && count > 0
    }
}

// ───────────────────────── Ponte D3D11 ↔ D3D12/wgpu ─────────────────────────

struct GpuBridge {
    device: wgpu::Device,
    d3d12: ID3D12Device,
    d3d11: ID3D11Device,
}

/// Device D3D11 com suporte a vídeo no mesmo adapter (LUID) do device D3D12 do wgpu.
fn create_d3d11_on_same_adapter(d3d12: &ID3D12Device) -> PlatformResult<ID3D11Device> {
    unsafe {
        let luid = d3d12.GetAdapterLuid();
        let factory: IDXGIFactory4 =
            CreateDXGIFactory1().map_err(|e| os_error("CreateDXGIFactory1", e))?;
        let adapter: IDXGIAdapter = factory
            .EnumAdapterByLuid(luid)
            .map_err(|e| os_error("EnumAdapterByLuid", e))?;

        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .map_err(|e| os_error("D3D11CreateDevice", e))?;
        let device = device.ok_or_else(|| PlatformError::Os("D3D11CreateDevice sem device".into()))?;

        // O MF (decoder + video processor) usa o device a partir das threads dele.
        let multithread: ID3D11Multithread = device
            .cast()
            .map_err(|e| os_error("ID3D11Multithread", e))?;
        let _ = multithread.SetMultithreadProtected(BOOL::from(true));
        Ok(device)
    }
}

struct RingTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Abre o NT handle no D3D12 do wgpu e embrulha como `wgpu::Texture`. Fecha o handle sempre.
fn import_slot(bridge: &GpuBridge, handle: HANDLE, width: u32, height: u32) -> PlatformResult<RingTexture> {
    let mut resource: Option<ID3D12Resource> = None;
    let opened = unsafe { bridge.d3d12.OpenSharedHandle(handle, &mut resource) };
    let _ = unsafe { CloseHandle(handle) }; // o recurso aberto mantém a memória viva
    opened.map_err(|e| os_error("ID3D12Device::OpenSharedHandle", e))?;
    let resource =
        resource.ok_or_else(|| PlatformError::Os("OpenSharedHandle sem recurso".into()))?;

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let desc = wgpu::TextureDescriptor {
        label: Some("prismview_video_slot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: VIDEO_TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    // SAFETY: o recurso vem do mesmo adapter (LUID) do device do wgpu, foi criado com o formato,
    // tamanho, 1 mip e 1 amostra declarados em `desc`, e o conteúdo é escrito só pelo D3D11
    // com fence concluída antes de o slot ser exibido. Estado inicial COMMON (UNINITIALIZED).
    let texture = unsafe {
        let hal_texture = wgpu::hal::dx12::Device::texture_from_raw(
            resource,
            VIDEO_TEXTURE_FORMAT,
            wgpu::TextureDimension::D2,
            size,
            1,
            1,
        );
        bridge
            .device
            .create_texture_from_hal::<wgpu::hal::api::Dx12>(hal_texture, &desc)
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(RingTexture { texture, view })
}

// ───────────────────────── Worker (thread de decodificação) ─────────────────────────

/// Acorda a thread de render (ex.: `LoopProxy::wake`) quando a abertura assíncrona termina.
pub type VideoWaker = Arc<dyn Fn() + Send + Sync>;

struct WorkerArgs {
    path: PathBuf,
    device: ID3D11Device,
    wake: Option<VideoWaker>,
}

struct OpenedStream {
    info: VideoInfo,
    handles: SharedHandles,
}

type OpenReply = Result<OpenedStream, String>;

fn worker_main(args: WorkerArgs, shared: Arc<Shared>, reply: mpsc::Sender<OpenReply>) {
    // Resposta da abertura + acorda o render (que faz `try_recv` em `advance`).
    let send = |msg: OpenReply| -> bool {
        let delivered = reply.send(msg).is_ok();
        if let Some(wake) = args.wake.as_ref() {
            wake();
        }
        delivered
    };
    // Ordem de drop (inversa): pipeline → MF → COM.
    let _com = ComScope::enter();
    let _mf = match MfScope::start() {
        Ok(mf) => mf,
        Err(e) => {
            send(Err(e));
            return;
        }
    };
    let (mut pipeline, opened) = match Pipeline::open(&args) {
        Ok(ok) => ok,
        Err(e) => {
            send(Err(e));
            return;
        }
    };
    if !send(Ok(opened)) {
        return; // `open` desistiu (timeout/close): os handles foram fechados no drop da mensagem
    }
    drop(reply);
    pipeline.run(&shared);
}

struct Pipeline {
    reader: IMFSourceReader,
    _manager: IMFDXGIDeviceManager,
    context: ID3D11DeviceContext4,
    multithread: ID3D11Multithread,
    fence: ID3D11Fence,
    fence_value: u64,
    event: OwnedEvent,
    ring: Vec<ID3D11Texture2D>,
    width: u32,
    height: u32,
    /// Tamanho real das texturas do MF (podem ter padding, ex.: 1088 linhas).
    src_extent: Option<(u32, u32)>,
    frame_ms: u64,
}

enum Step {
    /// Frame copiado para o slot (PTS da mídia em ms).
    Frame(u64),
    /// Nada a publicar (tick de stream, frame pulado após seek).
    Skip,
    EndOfStream,
}

impl Pipeline {
    fn open(args: &WorkerArgs) -> Result<(Self, OpenedStream), String> {
        unsafe {
            let device = &args.device;

            // Gerenciador DXGI: decoder e video processor do MF trabalham no nosso device.
            let mut reset_token = 0u32;
            let mut manager: Option<IMFDXGIDeviceManager> = None;
            ctx(
                MFCreateDXGIDeviceManager(&mut reset_token, &mut manager),
                "MFCreateDXGIDeviceManager",
            )?;
            let manager = manager.ok_or("MFCreateDXGIDeviceManager sem gerenciador")?;
            ctx(manager.ResetDevice(device, reset_token), "IMFDXGIDeviceManager::ResetDevice")?;

            let mut attrs: Option<IMFAttributes> = None;
            ctx(MFCreateAttributes(&mut attrs, 3), "MFCreateAttributes")?;
            let attrs = attrs.ok_or("MFCreateAttributes sem atributos")?;
            ctx(
                attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &manager),
                "MF_SOURCE_READER_D3D_MANAGER",
            )?;
            ctx(
                attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1),
                "MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS",
            )?;
            // Conversão YUV → BGRA pelo video processor do MF, na GPU.
            ctx(
                attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1),
                "MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING",
            )?;

            let wide: Vec<u16> = args
                .path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let reader = ctx(
                MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs),
                "MFCreateSourceReaderFromURL",
            )?;
            ctx(
                reader.SetStreamSelection(ALL_STREAMS, BOOL::from(false)),
                "SetStreamSelection(todos)",
            )?;
            ctx(
                reader.SetStreamSelection(FIRST_VIDEO_STREAM, BOOL::from(true)),
                "SetStreamSelection(vídeo)",
            )?;

            let codec = reader
                .GetNativeMediaType(FIRST_VIDEO_STREAM, 0)
                .ok()
                .and_then(|t| t.GetGUID(&MF_MT_SUBTYPE).ok())
                .and_then(codec_from_subtype);

            let output = ctx(MFCreateMediaType(), "MFCreateMediaType")?;
            ctx(output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video), "MF_MT_MAJOR_TYPE")?;
            ctx(output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_ARGB32), "MF_MT_SUBTYPE")?;
            ctx(
                reader.SetCurrentMediaType(FIRST_VIDEO_STREAM, None, &output),
                "SetCurrentMediaType(ARGB32)",
            )?;

            let current = ctx(
                reader.GetCurrentMediaType(FIRST_VIDEO_STREAM),
                "GetCurrentMediaType",
            )?;
            let packed = ctx(current.GetUINT64(&MF_MT_FRAME_SIZE), "MF_MT_FRAME_SIZE")?;
            let (width, height) = ((packed >> 32) as u32, packed as u32);
            if width == 0 || height == 0 {
                return Err(format!("dimensões de vídeo inválidas: {width}x{height}"));
            }
            let (rate_num, rate_den) = current
                .GetUINT64(&MF_MT_FRAME_RATE)
                .map(|r| ((r >> 32) as u32, r as u32))
                .ok()
                .filter(|&(n, d)| n > 0 && d > 0)
                .unwrap_or((30, 1));
            let frame_ms = (u64::from(rate_den) * 1000 / u64::from(rate_num)).max(1);
            let duration_ms = reader
                .GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION)
                .ok()
                .and_then(|pv| u64::try_from(&pv).ok())
                .map(|hns| hns / 10_000);

            // Sincronização da cópia: fence D3D11 + evento (espera só nesta thread).
            let device5: ID3D11Device5 = ctx(device.cast(), "ID3D11Device5")?;
            let mut fence: Option<ID3D11Fence> = None;
            ctx(
                device5.CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence),
                "ID3D11Device5::CreateFence",
            )?;
            let fence = fence.ok_or("CreateFence sem fence")?;
            let event = OwnedEvent(ctx(
                CreateEventW(None, BOOL::from(false), BOOL::from(false), PCWSTR::null()),
                "CreateEventW",
            )?);
            let context: ID3D11DeviceContext4 = ctx(
                ctx(device.GetImmediateContext(), "GetImmediateContext")?.cast(),
                "ID3D11DeviceContext4",
            )?;
            let multithread: ID3D11Multithread = ctx(device.cast(), "ID3D11Multithread")?;

            // Anel de texturas compartilháveis (NT handle) com o D3D12 do wgpu.
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0)
                    as u32,
            };
            let mut ring = Vec::with_capacity(RING_SLOTS);
            let mut handles = SharedHandles(Vec::with_capacity(RING_SLOTS));
            for _ in 0..RING_SLOTS {
                let mut texture: Option<ID3D11Texture2D> = None;
                ctx(
                    device.CreateTexture2D(&desc, None, Some(&mut texture)),
                    "CreateTexture2D(slot compartilhado)",
                )?;
                let texture = texture.ok_or("CreateTexture2D sem textura")?;
                let resource: IDXGIResource1 = ctx(texture.cast(), "IDXGIResource1")?;
                let handle = ctx(
                    resource.CreateSharedHandle(
                        None,
                        DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                        PCWSTR::null(),
                    ),
                    "IDXGIResource1::CreateSharedHandle",
                )?;
                handles.0.push(handle.0 as isize);
                ring.push(texture);
            }

            let info = VideoInfo {
                width,
                height,
                frame_rate_num: rate_num,
                frame_rate_den: rate_den,
                duration_ms,
                codec,
            };
            let pipeline = Self {
                reader,
                _manager: manager,
                context,
                multithread,
                fence,
                fence_value: 0,
                event,
                ring,
                width,
                height,
                src_extent: None,
                frame_ms,
            };
            Ok((pipeline, OpenedStream { info, handles }))
        }
    }

    fn seek(&self, position_ms: u64) -> Result<(), String> {
        let hns = i64::try_from(position_ms.saturating_mul(10_000)).unwrap_or(i64::MAX);
        let position = PROPVARIANT::from(hns);
        // GUID nulo = unidades de 100 ns.
        ctx(
            unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &position) },
            "IMFSourceReader::SetCurrentPosition",
        )
    }

    /// Lê a próxima amostra e, se houver frame, copia (GPU→GPU) para `slot` e espera a cópia.
    fn decode_into(&mut self, slot: usize) -> Result<Step, String> {
        let mut flags = 0u32;
        let mut timestamp = 0i64;
        let mut sample: Option<IMFSample> = None;
        ctx(
            unsafe {
                self.reader.ReadSample(
                    FIRST_VIDEO_STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
            },
            "IMFSourceReader::ReadSample",
        )?;
        if flags & FLAG_ERROR != 0 {
            return Err("erro no stream de vídeo (MF_SOURCE_READERF_ERROR)".into());
        }
        if flags & FLAG_TYPE_CHANGED != 0 {
            self.check_media_type()?;
        }
        if flags & FLAG_END_OF_STREAM != 0 {
            return Ok(Step::EndOfStream);
        }
        let Some(sample) = sample else {
            return Ok(Step::Skip); // gap/stream tick
        };
        self.copy_sample(&sample, slot)?;
        Ok(Step::Frame(u64::try_from(timestamp).unwrap_or(0) / 10_000))
    }

    /// Troca de tipo no meio do stream: só aceitamos se a resolução não mudou (o anel tem
    /// tamanho fixo; resolução nova exige reabrir o vídeo).
    fn check_media_type(&mut self) -> Result<(), String> {
        let current = ctx(
            unsafe { self.reader.GetCurrentMediaType(FIRST_VIDEO_STREAM) },
            "GetCurrentMediaType",
        )?;
        let packed = ctx(unsafe { current.GetUINT64(&MF_MT_FRAME_SIZE) }, "MF_MT_FRAME_SIZE")?;
        let (w, h) = ((packed >> 32) as u32, packed as u32);
        if (w, h) != (self.width, self.height) {
            return Err(format!(
                "resolução do vídeo mudou de {}x{} para {w}x{h}",
                self.width, self.height
            ));
        }
        self.src_extent = None; // reavalia formato/tamanho das texturas do MF
        Ok(())
    }

    fn copy_sample(&mut self, sample: &IMFSample, slot: usize) -> Result<(), String> {
        let dst = self.ring.get(slot).ok_or("slot inválido")?;
        unsafe {
            let buffer = ctx(sample.GetBufferByIndex(0), "IMFSample::GetBufferByIndex")?;
            let dxgi: IMFDXGIBuffer = ctx(
                buffer.cast(),
                "IMFDXGIBuffer (amostra não está na GPU: decode por hardware indisponível?)",
            )?;
            let mut raw: *mut c_void = std::ptr::null_mut();
            ctx(
                dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw),
                "IMFDXGIBuffer::GetResource",
            )?;
            if raw.is_null() {
                return Err("IMFDXGIBuffer::GetResource retornou nulo".into());
            }
            let src = ID3D11Texture2D::from_raw(raw);
            let subresource = ctx(dxgi.GetSubresourceIndex(), "IMFDXGIBuffer::GetSubresourceIndex")?;

            let (src_w, src_h) = match self.src_extent {
                Some(extent) => extent,
                None => {
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    src.GetDesc(&mut desc);
                    if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                        return Err(format!(
                            "video processor entregou {:?}, esperado B8G8R8A8_UNORM",
                            desc.Format
                        ));
                    }
                    self.src_extent = Some((desc.Width, desc.Height));
                    (desc.Width, desc.Height)
                }
            };
            let region = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: self.width.min(src_w),
                bottom: self.height.min(src_h),
                back: 1,
            };

            // Cópia GPU→GPU + sinal da fence, serializados com as threads do MF.
            self.fence_value += 1;
            self.multithread.Enter();
            self.context
                .CopySubresourceRegion(dst, 0, 0, 0, 0, &src, subresource, Some(&region));
            let signaled = self.context.Signal(&self.fence, self.fence_value);
            self.context.Flush();
            self.multithread.Leave();
            ctx(signaled, "ID3D11DeviceContext4::Signal")?;

            ctx(
                self.fence.SetEventOnCompletion(self.fence_value, self.event.0),
                "ID3D11Fence::SetEventOnCompletion",
            )?;
            if WaitForSingleObject(self.event.0, FENCE_TIMEOUT_MS) != WAIT_OBJECT_0 {
                return Err("timeout esperando a cópia do frame na GPU".into());
            }
        }
        // `sample` (superfície do decoder) é liberada pelo chamador após o retorno.
        Ok(())
    }

    fn run(&mut self, shared: &Shared) {
        let mut gen = 0u64;
        let mut loop_base_ms = 0u64;
        let mut last_ts_ms = 0u64;
        let mut skip_until_ms: Option<u64> = None;

        loop {
            // 1. Comandos e um slot livre (sem trabalho → dorme na condvar).
            let (slot, seek, looping) = {
                let mut st = shared.lock();
                loop {
                    if st.stop {
                        return;
                    }
                    if let Some(seek) = st.seek.take() {
                        break (None, Some(seek), st.looping);
                    }
                    if !st.end_of_stream && st.error.is_none() {
                        if let Some(i) = st.slots.iter().position(|s| s.state == SlotState::Free) {
                            st.slots[i].state = SlotState::Writing;
                            break (Some(i), None, st.looping);
                        }
                    }
                    st = shared.wait(st);
                }
            };

            if let Some((position_ms, new_gen)) = seek {
                match self.seek(position_ms) {
                    Ok(()) => {
                        gen = new_gen;
                        loop_base_ms = 0;
                        last_ts_ms = 0;
                        skip_until_ms = Some(position_ms);
                    }
                    Err(e) => fail(shared, None, e),
                }
                continue;
            }
            let Some(slot) = slot else {
                continue;
            };

            // 2. Decodifica e copia (fora do lock: ReadSample bloqueia).
            match self.decode_into(slot) {
                Ok(Step::Frame(ts_ms)) => {
                    let early = skip_until_ms
                        .is_some_and(|target| ts_ms.saturating_add(self.frame_ms / 2) < target);
                    if early {
                        release(shared, slot); // pré-roll do keyframe antes do alvo do seek
                        continue;
                    }
                    skip_until_ms = None;
                    last_ts_ms = ts_ms;
                    let mut st = shared.lock();
                    st.slots[slot] = SlotMeta {
                        state: SlotState::Ready,
                        pts_ms: loop_base_ms + ts_ms,
                        gen,
                    };
                    drop(st);
                    shared.cv.notify_all();
                }
                Ok(Step::Skip) => release(shared, slot),
                Ok(Step::EndOfStream) => {
                    release(shared, slot);
                    if looping {
                        // PTS continua monotônico entre voltas (o relógio do render não salta).
                        loop_base_ms += last_ts_ms + self.frame_ms;
                        last_ts_ms = 0;
                        if let Err(e) = self.seek(0) {
                            fail(shared, None, e);
                        }
                    } else {
                        shared.lock().end_of_stream = true;
                        shared.cv.notify_all();
                    }
                }
                Err(e) => fail(shared, Some(slot), e),
            }
        }
    }
}

fn release(shared: &Shared, slot: usize) {
    let mut st = shared.lock();
    if let Some(meta) = st.slots.get_mut(slot) {
        meta.state = SlotState::Free;
    }
}

fn fail(shared: &Shared, slot: Option<usize>, message: String) {
    let mut st = shared.lock();
    if let Some(meta) = slot.and_then(|i| st.slots.get_mut(i)) {
        meta.state = SlotState::Free;
    }
    st.error = Some(message);
    drop(st);
    shared.cv.notify_all();
}

fn stop_worker(shared: &Shared, worker: Option<JoinHandle<()>>, join: bool) {
    shared.lock().stop = true;
    shared.cv.notify_all();
    if let (Some(worker), true) = (worker, join) {
        let _ = worker.join();
    }
}

// ───────────────────────── Sessão (lado do render) ─────────────────────────

struct Session {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    ring: Vec<RingTexture>,
    info: VideoInfo,
    frame_interval: Duration,
    /// Geração de seek cujos frames são aceitos.
    gen: u64,
    shown: Option<usize>,
    shown_pts: u64,
    /// Após abrir/seek: exibir o primeiro frame que chegar e ancorar o relógio nele.
    awaiting_sync: bool,
    anchor: Instant,
    anchor_ms: u64,
    paused_ms: Option<u64>,
    tick: u64,
}

impl Session {
    fn media_ms(&self, now: Instant) -> u64 {
        match self.paused_ms {
            Some(ms) => ms,
            None => {
                let elapsed = now.saturating_duration_since(self.anchor).as_millis();
                self.anchor_ms
                    .saturating_add(u64::try_from(elapsed).unwrap_or(u64::MAX))
            }
        }
    }
}

/// Abertura em curso: o worker abre o arquivo no MF (pode levar segundos com 4K/HEVC) enquanto
/// o loop segue desenhando. Concluída em `advance()` por `try_recv`.
struct PendingOpen {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    reply: mpsc::Receiver<OpenReply>,
    started: Instant,
}

// ───────────────────────── MfVideoDecoder ─────────────────────────

/// Decodificador Media Foundation com saída zero-copy em `wgpu::Texture` (backend DX12).
/// Um por vídeo; usar na thread de render (o worker é interno).
pub struct MfVideoDecoder {
    bridge: Option<GpuBridge>,
    session: Option<Session>,
    pending: Option<PendingOpen>,
    /// Falha da abertura assíncrona (a sessão não existe para guardar o erro).
    open_error: Option<String>,
    waker: Option<VideoWaker>,
    looping: bool,
    paused: bool,
    generation: u64,
}

impl Default for MfVideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MfVideoDecoder {
    pub fn new() -> Self {
        Self {
            bridge: None,
            session: None,
            pending: None,
            open_error: None,
            waker: None,
            looping: true,
            paused: false,
            generation: 0,
        }
    }

    pub fn info(&self) -> Option<&VideoInfo> {
        self.session.as_ref().map(|s| &s.info)
    }

    /// Posição atual de reprodução (ms, monotônica entre voltas do loop).
    pub fn position_ms(&self) -> Option<u64> {
        self.session.as_ref().map(|s| s.media_ms(Instant::now()))
    }

    /// Acorda o loop quando uma abertura assíncrona termina (sucesso ou erro).
    pub fn set_waker(&mut self, waker: VideoWaker) {
        self.waker = Some(waker);
    }

    /// Abre sem bloquear: o worker abre o arquivo e o anel é importado no `advance()` seguinte à
    /// resposta. Erros imediatos (arquivo ausente, sem GPU) voltam aqui; os demais em `last_error`.
    pub fn open_async(&mut self, path: &Path) -> PlatformResult<()> {
        self.close();
        let bridge = self.bridge.as_ref().ok_or_else(|| {
            PlatformError::Os("MfVideoDecoder: chame bind_gpu antes de open".into())
        })?;
        if !path.is_file() {
            return Err(PlatformError::Os(format!("vídeo não encontrado: {}", path.display())));
        }

        let shared = Arc::new(Shared::new(self.looping));
        let (reply_tx, reply_rx) = mpsc::channel::<OpenReply>();
        let args = WorkerArgs {
            path: path.to_path_buf(),
            device: bridge.d3d11.clone(),
            wake: self.waker.clone(),
        };
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("prismview-mf-video".into())
            .spawn(move || worker_main(args, worker_shared, reply_tx))
            .map_err(|e| PlatformError::Os(format!("thread de vídeo: {e}")))?;
        self.pending = Some(PendingOpen {
            shared,
            worker: Some(worker),
            reply: reply_rx,
            started: Instant::now(),
        });
        Ok(())
    }

    /// Conclui a abertura assíncrona se o worker já respondeu (ou estourou o tempo).
    fn poll_pending(&mut self) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let reply = match pending.reply.try_recv() {
            Ok(reply) => reply,
            Err(mpsc::TryRecvError::Empty) if pending.started.elapsed() < OPEN_TIMEOUT => return,
            Err(mpsc::TryRecvError::Empty) => Err("timeout abrindo o vídeo".to_string()),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("worker de vídeo encerrou sem responder".to_string())
            }
        };
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        match reply {
            Ok(opened) => {
                if let Err(err) = self.finish_open(&mut pending, opened) {
                    self.open_error = Some(err.to_string());
                }
            }
            Err(message) => {
                // Worker travado no MF (timeout) ou já encerrado: não espera a thread.
                stop_worker(&pending.shared, pending.worker.take(), false);
                self.open_error = Some(message);
            }
        }
    }

    /// Bloqueia até a abertura terminar (contrato síncrono do trait `open`).
    fn open_session(&mut self, path: &Path) -> PlatformResult<VideoInfo> {
        self.open_async(path)?;
        let Some(mut pending) = self.pending.take() else {
            return Err(PlatformError::Os("abertura de vídeo perdida".into()));
        };
        let reply = pending
            .reply
            .recv_timeout(OPEN_TIMEOUT)
            .unwrap_or_else(|_| Err("timeout abrindo o vídeo".to_string()));
        match reply {
            Ok(opened) => self.finish_open(&mut pending, opened),
            Err(message) => {
                stop_worker(&pending.shared, pending.worker.take(), false);
                Err(PlatformError::Os(message))
            }
        }
    }

    /// Importa o anel de texturas compartilhadas no D3D12/wgpu e cria a sessão.
    fn finish_open(
        &mut self,
        pending: &mut PendingOpen,
        mut opened: OpenedStream,
    ) -> PlatformResult<VideoInfo> {
        let shared = Arc::clone(&pending.shared);
        let worker = pending.worker.take();
        let Some(bridge) = self.bridge.as_ref() else {
            stop_worker(&shared, worker, true);
            return Err(PlatformError::Os("MfVideoDecoder: GPU desvinculada".into()));
        };

        let (width, height) = (opened.info.width, opened.info.height);
        let mut ring = Vec::with_capacity(RING_SLOTS);
        for i in 0..RING_SLOTS {
            let imported = match opened.handles.take(i) {
                Some(handle) => import_slot(bridge, handle, width, height),
                None => Err(PlatformError::Os("handle de slot ausente".into())),
            };
            match imported {
                Ok(slot) => ring.push(slot),
                Err(err) => {
                    stop_worker(&shared, worker, true);
                    return Err(err);
                }
            }
        }

        let info = opened.info.clone();
        let frame_ms = if info.frame_rate_num > 0 && info.frame_rate_den > 0 {
            (u64::from(info.frame_rate_den) * 1000 / u64::from(info.frame_rate_num)).max(1)
        } else {
            DEFAULT_FRAME_MS
        };
        self.generation += 1;
        self.session = Some(Session {
            shared,
            worker,
            ring,
            info: info.clone(),
            frame_interval: Duration::from_millis(frame_ms),
            gen: 0,
            shown: None,
            shown_pts: 0,
            awaiting_sync: true,
            anchor: Instant::now(),
            anchor_ms: 0,
            paused_ms: self.paused.then_some(0),
            tick: 0,
        });
        Ok(info)
    }
}

impl HardwareVideoDecoder for MfVideoDecoder {
    fn supports(&self, codec: VideoCodec) -> bool {
        hardware_decoder_available(codec)
    }

    fn bind_gpu(&mut self, device: &wgpu::Device) -> PlatformResult<()> {
        self.close();
        self.bridge = None;
        // SAFETY: só lemos o ID3D12Device (AddRef via clone); o device do wgpu continua dono.
        let d3d12: Option<ID3D12Device> = unsafe {
            device.as_hal::<wgpu::hal::api::Dx12, _, _>(|hal| hal.map(|d| d.raw_device().clone()))
        };
        let d3d12 = d3d12.ok_or(PlatformError::Unsupported(
            "vídeo zero-copy exige o backend DX12 do wgpu",
        ))?;
        let d3d11 = create_d3d11_on_same_adapter(&d3d12)?;
        self.bridge = Some(GpuBridge {
            device: device.clone(),
            d3d12,
            d3d11,
        });
        Ok(())
    }

    fn open(&mut self, path: &Path) -> PlatformResult<VideoInfo> {
        self.close();
        self.open_session(path)
    }

    fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
        if let Some(s) = self.session.as_ref() {
            let mut st = s.shared.lock();
            st.looping = looping;
            if looping && st.end_of_stream {
                // Chegou ao fim sem loop e agora pediram loop: recomeça do início.
                st.end_of_stream = false;
                drop(st);
                let _ = self.seek_ms(0);
            }
        }
    }

    fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let now = Instant::now();
        if paused {
            s.paused_ms = Some(s.media_ms(now));
        } else if let Some(ms) = s.paused_ms.take() {
            s.anchor = now;
            s.anchor_ms = ms;
        }
        // Pausado o worker para sozinho quando o anel enche (nenhum slot é liberado).
    }

    fn seek_ms(&mut self, position_ms: u64) -> PlatformResult<()> {
        let paused = self.paused;
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| PlatformError::Os("nenhum vídeo aberto".into()))?;
        let position_ms = match s.info.duration_ms {
            Some(d) if d > 0 => position_ms.min(d.saturating_sub(1)),
            _ => position_ms,
        };
        s.gen += 1;
        s.awaiting_sync = true;
        if paused {
            s.paused_ms = Some(position_ms);
        }
        let mut st = s.shared.lock();
        st.seek = Some((position_ms, s.gen));
        st.end_of_stream = false;
        st.error = None;
        drop(st);
        s.shared.cv.notify_all();
        Ok(())
    }

    fn next_frame(&mut self) -> Option<DecodedFrame<'_>> {
        None // backend zero-copy: frames nunca passam pela CPU
    }

    fn advance(&mut self) -> bool {
        self.poll_pending();
        let paused = self.paused;
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        s.tick += 1;
        let tick = s.tick;
        let now = Instant::now();
        let media_now = s.media_ms(now);
        let sync = s.awaiting_sync || s.shown.is_none();
        let gen = s.gen;

        let mut st = s.shared.lock();
        let mut notify = false;

        // Slots aposentados cujo uso pela GPU já terminou; frames de seek antigo.
        for meta in st.slots.iter_mut() {
            match meta.state {
                SlotState::Retiring(until) if until <= tick => {
                    meta.state = SlotState::Free;
                    notify = true;
                }
                SlotState::Ready if meta.gen != gen => {
                    meta.state = SlotState::Free;
                    notify = true;
                }
                _ => {}
            }
        }

        // Sincronizando: o frame mais antigo. Senão: o mais novo cujo PTS já chegou.
        let mut pick: Option<(usize, u64)> = None;
        for (i, meta) in st.slots.iter().enumerate() {
            if meta.state != SlotState::Ready {
                continue;
            }
            let pts = meta.pts_ms;
            let better = if sync {
                pick.map_or(true, |(_, best)| pts < best)
            } else {
                pts <= media_now && pick.map_or(true, |(_, best)| pts > best)
            };
            if better {
                pick = Some((i, pts));
            }
        }

        let mut changed = false;
        if let Some((index, pts)) = pick {
            // Frames prontos mais antigos que o escolhido nunca serão exibidos (atraso → drop).
            for (j, meta) in st.slots.iter_mut().enumerate() {
                if j != index && meta.state == SlotState::Ready && meta.pts_ms < pts {
                    meta.state = SlotState::Free;
                    notify = true;
                }
            }
            if let Some(old) = s.shown {
                if let Some(meta) = st.slots.get_mut(old) {
                    meta.state = SlotState::Retiring(tick + RETIRE_TICKS);
                }
            }
            st.slots[index].state = SlotState::Shown;
            s.shown = Some(index);
            s.shown_pts = pts;
            if sync {
                s.awaiting_sync = false;
                s.anchor = now;
                s.anchor_ms = pts;
                if paused {
                    s.paused_ms = Some(pts);
                }
            }
            changed = true;
        }
        drop(st);
        if notify {
            s.shared.cv.notify_all();
        }
        changed
    }

    fn current_gpu_frame(&self) -> Option<GpuVideoFrame<'_>> {
        let s = self.session.as_ref()?;
        let index = s.shown?;
        let slot = s.ring.get(index)?;
        Some(GpuVideoFrame {
            texture: &slot.texture,
            view: &slot.view,
            width: s.info.width,
            height: s.info.height,
            format: VIDEO_TEXTURE_FORMAT,
            timestamp_ms: s.shown_pts,
            slot: index,
            generation: self.generation,
        })
    }

    fn time_until_next_frame(&self) -> Option<Duration> {
        if let Some(pending) = self.pending.as_ref() {
            // O worker acorda o loop ao responder; isto só garante o timeout.
            return Some(OPEN_TIMEOUT.saturating_sub(pending.started.elapsed()));
        }
        let s = self.session.as_ref()?;
        let sync = s.awaiting_sync || s.shown.is_none();
        if self.paused && !sync {
            return None;
        }
        let st = s.shared.lock();
        if st.error.is_some() {
            return None;
        }
        let next = st
            .slots
            .iter()
            .filter(|m| m.state == SlotState::Ready && m.gen == s.gen)
            .filter(|m| sync || m.pts_ms > s.shown_pts)
            .map(|m| m.pts_ms)
            .min();
        if sync {
            return Some(if next.is_some() { Duration::ZERO } else { s.frame_interval });
        }
        match next {
            Some(pts) => Some(Duration::from_millis(
                pts.saturating_sub(s.media_ms(Instant::now())),
            )),
            // Fim sem loop: último frame fica na tela, nada a esperar.
            None if st.end_of_stream && !st.looping => None,
            // Decoder atrás do relógio: tenta de novo em um intervalo de frame.
            None => Some(s.frame_interval),
        }
    }

    fn last_error(&self) -> Option<String> {
        if let Some(err) = self.open_error.as_ref() {
            return Some(err.clone());
        }
        self.session.as_ref().and_then(|s| s.shared.lock().error.clone())
    }

    fn close(&mut self) {
        self.open_error = None;
        if let Some(mut pending) = self.pending.take() {
            // Sem join: o MF pode estar travado abrindo; o worker sai ao ver o canal fechado.
            stop_worker(&pending.shared, pending.worker.take(), false);
        }
        if let Some(mut session) = self.session.take() {
            stop_worker(&session.shared, session.worker.take(), true);
            // As wgpu::Texture caem aqui; o wgpu adia a destruição até a GPU terminar.
            drop(session);
            self.generation += 1;
        }
    }
}

impl Drop for MfVideoDecoder {
    fn drop(&mut self) {
        self.close();
    }
}
