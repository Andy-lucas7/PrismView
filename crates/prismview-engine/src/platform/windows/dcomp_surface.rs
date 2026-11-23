//! Apresentação do wallpaper **sem swapchain**: o quadro vai para uma `IDCompositionSurface`.
//!
//! Por que: overlays de terceiros (RivaTuner/MSI Afterburner, NVIDIA, Steam…) se penduram em
//! `IDXGISwapChain::Present` de qualquer processo DirectX e tratam o motor como um jogo,
//! desenhando o OSD por cima do wallpaper. Uma superfície DirectComposition é atualizada com
//! `BeginDraw`/`EndDraw` + `Commit` e quem compõe é o DWM — não há `Present` no nosso processo,
//! então não há onde um overlay desenhar. Vale para qualquer overlay, sem configuração.
//!
//! Por quadro (tudo na GPU, sem readback, sem alocação):
//! 1. D3D12 (fila do wgpu): espera a leitura anterior do D3D11 (`Wait` na fence compartilhada),
//!    copia a textura do wgpu para a textura compartilhada e sinaliza `v`.
//! 2. D3D11 (mesmo adapter): espera `v`, copia para a superfície (`BeginDraw`), `EndDraw`,
//!    sinaliza `v + 1` (leitura concluída), `Flush` e `Commit`.
//! 3. `DwmFlush` cadencia o loop no vsync do compositor (como o `Present(1)` fazia).

use std::mem::ManuallyDrop;

use ::windows::core::{Interface, PCWSTR};
use ::windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, HWND, POINT};
use ::windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use ::windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Device5, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_FENCE_FLAG_SHARED, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use ::windows::Win32::Graphics::Direct3D12::{
    ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue, ID3D12Device, ID3D12Fence,
    ID3D12GraphicsCommandList, ID3D12Resource, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATES, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12_RESOURCE_TRANSITION_BARRIER,
};
use ::windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual2,
};
use ::windows::Win32::Graphics::Dwm::DwmFlush;
use ::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE, DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_SAMPLE_DESC,
};
use ::windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIDevice, IDXGIFactory4, IDXGIResource1, DXGI_SHARED_RESOURCE_READ,
    DXGI_SHARED_RESOURCE_WRITE,
};
use ::windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::util::os_error;
use crate::engine_core::gpu::{ExternalPresenter, FrameStatus, GpuContext};
use crate::platform::{PlatformError, PlatformResult};

const SLOTS: usize = 2;
/// Outra superfície já esperou o compositor há menos que isto: não espera de novo.
const FLUSH_MIN_GAP: std::time::Duration = std::time::Duration::from_millis(4);

thread_local! {
    /// Última espera do compositor nesta thread (todas as superfícies vivem no loop).
    static LAST_FLUSH: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}
/// Espera máxima pela GPU antes de reusar um allocator / soltar recursos.
const FENCE_WAIT_MS: u32 = 2000;
/// `GENERIC_ALL` para o handle compartilhado da fence.
const GENERIC_ALL: u32 = 0x1000_0000;

/// Textura compartilhada D3D11 ⇄ D3D12 (criada no D3D11, aberta no D3D12).
struct SharedTarget {
    d3d11: ID3D11Texture2D,
    d3d12: ID3D12Resource,
}

pub struct DCompSurfacePresenter {
    // Ordem de drop: lista/allocators e recursos antes dos devices; visual/target antes do DComp.
    list: ID3D12GraphicsCommandList,
    list_base: ID3D12CommandList,
    allocators: [ID3D12CommandAllocator; SLOTS],
    slot_values: [u64; SLOTS],
    frame: usize,
    /// Fence compartilhada: D3D12 sinaliza cópias (`v`), D3D11 sinaliza leituras (`v + 1`).
    fence12: ID3D12Fence,
    fence11: ID3D11Fence,
    fence_event: HANDLE,
    next_value: u64,
    /// Última leitura concluída pelo D3D11 (o D3D12 espera antes de sobrescrever).
    read_done: u64,
    shared: Option<SharedTarget>,
    surface: Option<IDCompositionSurface>,
    visual: IDCompositionVisual2,
    _target: IDCompositionTarget,
    dcomp: IDCompositionDesktopDevice,
    context: ID3D11DeviceContext4,
    device11: ID3D11Device,
    queue: ID3D12CommandQueue,
    device12: ID3D12Device,
    /// `IGNORE` (wallpaper opaco) ou `PREMULTIPLIED` (widgets transparentes).
    alpha: DXGI_ALPHA_MODE,
    size: (u32, u32),
}

/// Barreira de transição que **empresta** `resource` (sem AddRef/Release: `ManuallyDrop`).
fn transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: cópia bit a bit do ponteiro COM, nunca liberada (ManuallyDrop); o recurso
                // vive até a command list terminar (fence antes de soltá-lo).
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

fn create_d3d11(device12: &ID3D12Device) -> PlatformResult<ID3D11Device> {
    // SAFETY: chamadas DXGI/D3D11 com parâmetros válidos; o device retornado é nosso.
    unsafe {
        let luid = device12.GetAdapterLuid();
        let factory: IDXGIFactory4 = CreateDXGIFactory1().map_err(|e| os_error("CreateDXGIFactory1", e))?;
        let adapter: IDXGIAdapter = factory.EnumAdapterByLuid(luid).map_err(|e| os_error("EnumAdapterByLuid", e))?;
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
        .map_err(|e| os_error("D3D11CreateDevice", e))?;
        device.ok_or_else(|| PlatformError::Os("D3D11CreateDevice sem device".into()))
    }
}

impl DCompSurfacePresenter {
    /// `transparent`: alpha pré-multiplicado por pixel (widgets); senão opaco (wallpaper).
    pub fn new(gpu: &GpuContext, hwnd: HWND, width: u32, height: u32, transparent: bool) -> PlatformResult<Self> {
        // SAFETY: só lemos device/fila D3D12 (AddRef via clone); o wgpu continua dono deles.
        let raw: Option<(ID3D12Device, ID3D12CommandQueue)> = unsafe {
            gpu.device().as_hal::<wgpu::hal::api::Dx12, _, _>(|hal| {
                hal.map(|d| (d.raw_device().clone(), d.raw_queue().clone()))
            })
        };
        let (device12, queue) =
            raw.ok_or(PlatformError::Unsupported("superfície DirectComposition exige o backend DX12"))?;
        let device11 = create_d3d11(&device12)?;
        // SAFETY: interfaces COM válidas; handles fechados após abrir do outro lado.
        unsafe {
            let context: ID3D11DeviceContext4 = device11
                .GetImmediateContext()
                .map_err(|e| os_error("GetImmediateContext", e))?
                .cast()
                .map_err(|e| os_error("ID3D11DeviceContext4", e))?;
            let device11_5: ID3D11Device5 = device11.cast().map_err(|e| os_error("ID3D11Device5", e))?;
            let mut fence11: Option<ID3D11Fence> = None;
            device11_5
                .CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence11)
                .map_err(|e| os_error("ID3D11Device5::CreateFence", e))?;
            let fence11 = fence11.ok_or_else(|| PlatformError::Os("CreateFence sem fence".into()))?;
            let handle = fence11
                .CreateSharedHandle(None, GENERIC_ALL, PCWSTR::null())
                .map_err(|e| os_error("ID3D11Fence::CreateSharedHandle", e))?;
            let mut fence12: Option<ID3D12Fence> = None;
            let opened = device12.OpenSharedHandle(handle, &mut fence12);
            let _ = CloseHandle(handle); // a fence aberta mantém o objeto vivo
            opened.map_err(|e| os_error("OpenSharedHandle(fence)", e))?;
            let fence12 = fence12.ok_or_else(|| PlatformError::Os("OpenSharedHandle sem fence".into()))?;

            let dxgi: IDXGIDevice = device11.cast().map_err(|e| os_error("IDXGIDevice", e))?;
            let dcomp: IDCompositionDesktopDevice =
                DCompositionCreateDevice2(&dxgi).map_err(|e| os_error("DCompositionCreateDevice2", e))?;
            let target = dcomp
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| os_error("CreateTargetForHwnd", e))?;
            let visual = dcomp.CreateVisual().map_err(|e| os_error("CreateVisual", e))?;
            target.SetRoot(&visual).map_err(|e| os_error("SetRoot", e))?;

            let allocators = [
                device12
                    .CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .map_err(|e| os_error("CreateCommandAllocator", e))?,
                device12
                    .CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .map_err(|e| os_error("CreateCommandAllocator", e))?,
            ];
            let list: ID3D12GraphicsCommandList = device12
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None)
                .map_err(|e| os_error("CreateCommandList", e))?;
            list.Close().map_err(|e| os_error("CommandList::Close", e))?;
            let list_base: ID3D12CommandList = list.cast().map_err(|e| os_error("ID3D12CommandList", e))?;
            let fence_event = CreateEventW(None, false, false, None).map_err(|e| os_error("CreateEventW", e))?;

            let mut this = Self {
                list,
                list_base,
                allocators,
                slot_values: [0; SLOTS],
                frame: 0,
                fence12,
                fence11,
                fence_event,
                next_value: 1,
                read_done: 0,
                shared: None,
                surface: None,
                visual,
                _target: target,
                dcomp,
                context,
                device11,
                queue,
                device12,
                alpha: if transparent { DXGI_ALPHA_MODE_PREMULTIPLIED } else { DXGI_ALPHA_MODE_IGNORE },
                size: (0, 0),
            };
            this.build_targets(width.max(1), height.max(1))?;
            Ok(this)
        }
    }

    /// (Re)cria a textura compartilhada e a superfície no tamanho pedido (só em resize).
    fn build_targets(&mut self, width: u32, height: u32) -> PlatformResult<()> {
        self.shared = None;
        self.surface = None;
        // SAFETY: descritores válidos; o handle NT é fechado após abrir no D3D12.
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
            };
            let mut texture: Option<ID3D11Texture2D> = None;
            self.device11
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|e| os_error("CreateTexture2D(compartilhada)", e))?;
            let d3d11 = texture.ok_or_else(|| PlatformError::Os("CreateTexture2D sem textura".into()))?;
            let resource: IDXGIResource1 = d3d11.cast().map_err(|e| os_error("IDXGIResource1", e))?;
            let handle = resource
                .CreateSharedHandle(None, DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0, PCWSTR::null())
                .map_err(|e| os_error("IDXGIResource1::CreateSharedHandle", e))?;
            let mut d3d12: Option<ID3D12Resource> = None;
            let opened = self.device12.OpenSharedHandle(handle, &mut d3d12);
            let _ = CloseHandle(handle);
            opened.map_err(|e| os_error("OpenSharedHandle(textura)", e))?;
            let d3d12 = d3d12.ok_or_else(|| PlatformError::Os("OpenSharedHandle sem recurso".into()))?;

            // Wallpaper opaco (sem blend no DWM); widgets com alpha pré-multiplicado.
            let surface = self
                .dcomp
                .CreateSurface(width, height, DXGI_FORMAT_B8G8R8A8_UNORM, self.alpha)
                .map_err(|e| os_error("CreateSurface", e))?;
            self.visual.SetContent(&surface).map_err(|e| os_error("SetContent", e))?;
            self.dcomp.Commit().map_err(|e| os_error("DComp Commit", e))?;
            self.shared = Some(SharedTarget { d3d11, d3d12 });
            self.surface = Some(surface);
        }
        self.size = (width, height);
        Ok(())
    }

    /// Espera a GPU concluir `value` da fence (com teto). `false` = timeout.
    fn wait_fence(&self, value: u64) -> bool {
        // SAFETY: fence e evento válidos durante a vida do presenter.
        unsafe {
            if value == 0 || self.fence12.GetCompletedValue() >= value {
                return true;
            }
            if self.fence12.SetEventOnCompletion(value, self.fence_event).is_err() {
                return false;
            }
            WaitForSingleObject(self.fence_event, FENCE_WAIT_MS).0 == 0
        }
    }

    /// Nenhuma cópia (D3D12) nem leitura (D3D11) pendente.
    fn wait_idle(&self) -> bool {
        self.wait_fence(self.next_value.saturating_sub(1)) && self.wait_fence(self.read_done)
    }
}

impl ExternalPresenter for DCompSurfacePresenter {
    fn present(&mut self, gpu: &GpuContext, texture: &wgpu::Texture) -> FrameStatus {
        if gpu.is_device_lost() {
            return FrameStatus::DeviceLost;
        }
        // SAFETY: lemos o ID3D12Resource da textura do wgpu (AddRef, sem alocação de heap).
        let source: Option<ID3D12Resource> = unsafe {
            texture.as_hal::<wgpu::hal::api::Dx12, _, _>(|t| t.map(|t| t.raw_resource().clone()))
        };
        let (Some(source), Some(shared), Some(surface)) = (source, self.shared.as_ref(), self.surface.as_ref())
        else {
            return FrameStatus::Unavailable;
        };
        let slot = self.frame % SLOTS;
        if !self.wait_fence(self.slot_values[slot]) {
            return FrameStatus::Retry;
        }
        // SAFETY: recursos vivos até as fences concluírem; barreiras emprestam os ponteiros.
        unsafe {
            // 1) D3D12: textura do wgpu → textura compartilhada.
            let allocator = &self.allocators[slot];
            if allocator.Reset().is_err() || self.list.Reset(allocator, None).is_err() {
                return FrameStatus::Retry;
            }
            // O wgpu deixou a textura em RENDER_TARGET e continua acreditando nisso: voltamos no fim.
            self.list.ResourceBarrier(&[
                transition(&source, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE),
                transition(&shared.d3d12, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST),
            ]);
            self.list.CopyResource(&shared.d3d12, &source);
            self.list.ResourceBarrier(&[
                transition(&source, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
                transition(&shared.d3d12, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COMMON),
            ]);
            if self.list.Close().is_err() {
                return FrameStatus::Retry;
            }
            // Não sobrescrever enquanto o D3D11 ainda lê o quadro anterior (espera na GPU).
            if self.read_done > 0 && self.queue.Wait(&self.fence12, self.read_done).is_err() {
                return FrameStatus::DeviceLost;
            }
            self.queue.ExecuteCommandLists(&[Some(self.list_base.clone())]);
            let copied = self.next_value;
            if self.queue.Signal(&self.fence12, copied).is_err() {
                return FrameStatus::DeviceLost;
            }
            self.slot_values[slot] = copied;
            self.next_value = copied + 2;
            self.frame = self.frame.wrapping_add(1);

            // 2) D3D11: textura compartilhada → superfície DirectComposition.
            if self.context.Wait(&self.fence11, copied).is_err() {
                return FrameStatus::Retry;
            }
            let mut offset = POINT::default();
            let dst: ID3D11Texture2D = match surface.BeginDraw(None, &mut offset) {
                Ok(dst) => dst,
                Err(_) => return if gpu.is_device_lost() { FrameStatus::DeviceLost } else { FrameStatus::Retry },
            };
            self.context.CopySubresourceRegion(
                &dst,
                0,
                offset.x.max(0) as u32,
                offset.y.max(0) as u32,
                0,
                &shared.d3d11,
                0,
                None,
            );
            drop(dst);
            if surface.EndDraw().is_err() {
                return FrameStatus::Retry;
            }
            if self.context.Signal(&self.fence11, copied + 1).is_err() {
                return FrameStatus::Retry;
            }
            self.context.Flush();
            self.read_done = copied + 1;
            if self.dcomp.Commit().is_err() {
                return FrameStatus::Retry;
            }
            // 3) Cadência no vsync do compositor (sem `Present` para bloquear): no máximo uma espera
            // por ciclo do loop, mesmo com vários monitores/widgets apresentando em sequência.
            let now = std::time::Instant::now();
            let due = LAST_FLUSH.with(|c| c.get().is_none_or(|t| now.duration_since(t) >= FLUSH_MIN_GAP));
            if due {
                let _ = DwmFlush();
                LAST_FLUSH.with(|c| c.set(Some(std::time::Instant::now())));
            }
        }
        FrameStatus::Presented
    }

    fn resize(&mut self, _gpu: &GpuContext, width: u32, height: u32) -> bool {
        let (width, height) = (width.max(1), height.max(1));
        // Nenhuma cópia/leitura pendente pode tocar os alvos antigos.
        if !self.wait_idle() {
            return false;
        }
        if (width, height) == self.size && self.shared.is_some() {
            return true;
        }
        match self.build_targets(width, height) {
            Ok(()) => true,
            Err(err) => {
                eprintln!("[prismview-engine] superfície DComp {width}x{height}: {err}");
                false
            }
        }
    }
}

impl Drop for DCompSurfacePresenter {
    fn drop(&mut self) {
        // A textura do wgpu e a compartilhada só podem cair depois da última cópia/leitura.
        let _ = self.wait_idle();
        self.shared = None;
        self.surface = None;
        // SAFETY: evento criado em `new`, fechado uma vez.
        unsafe {
            let _ = CloseHandle(self.fence_event);
        }
    }
}

/// Apresentador do wallpaper para a janela `window` (ver doc do módulo).
pub fn wallpaper_presenter(
    gpu: &GpuContext,
    window: crate::platform::NativeWindowId,
    width: u32,
    height: u32,
) -> PlatformResult<Box<dyn ExternalPresenter>> {
    let hwnd = HWND(window.0 as *mut core::ffi::c_void);
    Ok(Box::new(DCompSurfacePresenter::new(gpu, hwnd, width, height, false)?))
}
