//! Apresentação transparente por pixel para widgets nativos (Entrega 3).
//!
//! O wgpu 24/DX12 só oferece `CompositeAlphaMode::Opaque`. Aqui a cadeia é nossa:
//! `IDXGIFactory2::CreateSwapChainForComposition` na **mesma fila D3D12 do wgpu**, com
//! `DXGI_ALPHA_MODE_PREMULTIPLIED`, ligada ao HWND por DirectComposition
//! (`IDCompositionDesktopDevice` → target → visual → `SetContent`).
//!
//! Por quadro (`ExternalPresenter::present`): o wgpu já renderizou e submeteu a textura do widget
//! (estado RENDER_TARGET); gravamos numa command list própria: barreiras → `CopyResource` para o
//! backbuffer → barreiras de volta, executamos na mesma fila (ordem garantida) e `Present`. Tudo
//! na GPU, nunca há readback para a CPU. Sem alocação por quadro: allocators/fence/lista/buffers
//! criados uma vez (buffers só no resize); as barreiras emprestam o recurso sem AddRef.

use std::mem::ManuallyDrop;

use ::windows::core::{IUnknown, Interface};
use ::windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HWND};
use ::windows::Win32::Graphics::Direct3D12::{
    ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue, ID3D12Device, ID3D12Fence,
    ID3D12GraphicsCommandList, ID3D12Resource, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_NONE,
    D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_TRANSITION_BARRIER,
};
use ::windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionTarget, IDCompositionVisual2,
};
use ::windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use ::windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIFactory2, IDXGISwapChain3, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use ::windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::util::os_error;
use crate::engine_core::gpu::{ExternalPresenter, FrameStatus, GpuContext};
use crate::platform::{PlatformError, PlatformResult};

const BUFFER_COUNT: u32 = 2;
/// Espera máxima pela GPU antes de reusar um allocator (evita travar o loop para sempre).
const FENCE_WAIT_MS: u32 = 2000;

pub struct DCompPresenter {
    // Ordem de drop: lista/allocators antes do device; visual/target antes do device DComp.
    list: ID3D12GraphicsCommandList,
    list_base: ID3D12CommandList,
    allocators: [ID3D12CommandAllocator; BUFFER_COUNT as usize],
    fence: ID3D12Fence,
    fence_event: HANDLE,
    fence_values: [u64; BUFFER_COUNT as usize],
    next_fence: u64,
    frame: usize,
    buffers: Vec<ID3D12Resource>,
    swapchain: IDXGISwapChain3,
    queue: ID3D12CommandQueue,
    _visual: IDCompositionVisual2,
    _target: IDCompositionTarget,
    dcomp: IDCompositionDesktopDevice,
    size: (u32, u32),
}

impl DCompPresenter {
    /// Cria a cadeia de composição (pré-multiplicada) para `hwnd` no device do wgpu.
    pub fn new(gpu: &GpuContext, hwnd: HWND, width: u32, height: u32) -> PlatformResult<Self> {
        // SAFETY: só lemos device/fila D3D12 (AddRef via clone); o wgpu continua dono deles.
        let raw: Option<(ID3D12Device, ID3D12CommandQueue)> = unsafe {
            gpu.device().as_hal::<wgpu::hal::api::Dx12, _, _>(|hal| {
                hal.map(|d| (d.raw_device().clone(), d.raw_queue().clone()))
            })
        };
        let (device, queue) =
            raw.ok_or(PlatformError::Unsupported("widgets transparentes exigem o backend DX12"))?;
        let (width, height) = (width.max(1), height.max(1));

        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))
                .map_err(|e| os_error("CreateDXGIFactory2", e))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: BOOL(0),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: BUFFER_COUNT,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swapchain = factory
                .CreateSwapChainForComposition(&queue, &desc, None)
                .map_err(|e| os_error("CreateSwapChainForComposition", e))?
                .cast::<IDXGISwapChain3>()
                .map_err(|e| os_error("IDXGISwapChain3", e))?;

            let dcomp: IDCompositionDesktopDevice =
                DCompositionCreateDevice2(None::<&IUnknown>).map_err(|e| os_error("DCompositionCreateDevice2", e))?;
            let target = dcomp
                .CreateTargetForHwnd(hwnd, true)
                .map_err(|e| os_error("CreateTargetForHwnd", e))?;
            let visual = dcomp.CreateVisual().map_err(|e| os_error("CreateVisual", e))?;
            visual.SetContent(&swapchain).map_err(|e| os_error("SetContent", e))?;
            target.SetRoot(&visual).map_err(|e| os_error("SetRoot", e))?;
            dcomp.Commit().map_err(|e| os_error("DComp Commit", e))?;

            let allocators = [
                device
                    .CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .map_err(|e| os_error("CreateCommandAllocator", e))?,
                device
                    .CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .map_err(|e| os_error("CreateCommandAllocator", e))?,
            ];
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None)
                .map_err(|e| os_error("CreateCommandList", e))?;
            list.Close().map_err(|e| os_error("CommandList::Close", e))?;
            let list_base: ID3D12CommandList = list.cast().map_err(|e| os_error("ID3D12CommandList", e))?;
            let fence: ID3D12Fence =
                device.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(|e| os_error("CreateFence", e))?;
            let fence_event =
                CreateEventW(None, false, false, None).map_err(|e| os_error("CreateEventW", e))?;

            let mut this = Self {
                list,
                list_base,
                allocators,
                fence,
                fence_event,
                fence_values: [0; BUFFER_COUNT as usize],
                next_fence: 1,
                frame: 0,
                buffers: Vec::with_capacity(BUFFER_COUNT as usize),
                swapchain,
                queue,
                _visual: visual,
                _target: target,
                dcomp,
                size: (width, height),
            };
            this.fetch_buffers()?;
            Ok(this)
        }
    }

    unsafe fn fetch_buffers(&mut self) -> PlatformResult<()> {
        self.buffers.clear();
        for i in 0..BUFFER_COUNT {
            let buffer: ID3D12Resource =
                self.swapchain.GetBuffer(i).map_err(|e| os_error("IDXGISwapChain::GetBuffer", e))?;
            self.buffers.push(buffer);
        }
        Ok(())
    }

    /// Espera a GPU concluir o valor `value` da fence (com teto). `false` = timeout.
    fn wait_fence(&self, value: u64) -> bool {
        unsafe {
            if value == 0 || self.fence.GetCompletedValue() >= value {
                return true;
            }
            if self.fence.SetEventOnCompletion(value, self.fence_event).is_err() {
                return false;
            }
            WaitForSingleObject(self.fence_event, FENCE_WAIT_MS).0 == 0
        }
    }

    fn wait_idle(&self) -> bool {
        let last = self.next_fence.saturating_sub(1);
        self.wait_fence(last)
    }
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
                // SAFETY: cópia bit a bit do ponteiro COM, nunca liberada (ManuallyDrop); o
                // recurso vive até a command list terminar (fence antes de soltá-lo).
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

impl ExternalPresenter for DCompPresenter {
    fn present(&mut self, gpu: &GpuContext, texture: &wgpu::Texture) -> FrameStatus {
        if gpu.is_device_lost() {
            return FrameStatus::DeviceLost;
        }
        // SAFETY: lemos o ID3D12Resource da textura do wgpu (AddRef, sem alocação de heap).
        let source: Option<ID3D12Resource> = unsafe {
            texture.as_hal::<wgpu::hal::api::Dx12, _, _>(|t| t.map(|t| t.raw_resource().clone()))
        };
        let Some(source) = source else {
            return FrameStatus::Unavailable;
        };
        let slot = self.frame % BUFFER_COUNT as usize;
        if !self.wait_fence(self.fence_values[slot]) {
            return FrameStatus::Retry;
        }
        unsafe {
            let index = self.swapchain.GetCurrentBackBufferIndex() as usize;
            let Some(back) = self.buffers.get(index) else {
                return FrameStatus::Unavailable;
            };
            let allocator = &self.allocators[slot];
            if allocator.Reset().is_err() || self.list.Reset(allocator, None).is_err() {
                return FrameStatus::Retry;
            }
            // O wgpu deixou a textura em RENDER_TARGET (último uso) e continua acreditando nisso:
            // voltamos a ela no fim para o rastreador dele seguir correto.
            let before = [
                transition(&source, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE),
                transition(back, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_COPY_DEST),
            ];
            self.list.ResourceBarrier(&before);
            self.list.CopyResource(back, &source);
            let after = [
                transition(&source, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET),
                transition(back, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PRESENT),
            ];
            self.list.ResourceBarrier(&after);
            if self.list.Close().is_err() {
                return FrameStatus::Retry;
            }
            self.queue.ExecuteCommandLists(&[Some(self.list_base.clone())]);
            let value = self.next_fence;
            self.next_fence += 1;
            if self.queue.Signal(&self.fence, value).is_err() {
                return FrameStatus::DeviceLost;
            }
            self.fence_values[slot] = value;
            self.frame = self.frame.wrapping_add(1);

            let hr = self.swapchain.Present(1, DXGI_PRESENT(0));
            if hr.is_err() {
                // DXGI_ERROR_DEVICE_REMOVED/RESET → refazer device; resto: tentar de novo.
                return if gpu.is_device_lost() { FrameStatus::DeviceLost } else { FrameStatus::Retry };
            }
        }
        FrameStatus::Presented
    }

    fn resize(&mut self, _gpu: &GpuContext, width: u32, height: u32) -> bool {
        let (width, height) = (width.max(1), height.max(1));
        // Nenhuma cópia pendente pode tocar backbuffers/alvo antigos.
        if !self.wait_idle() {
            return false;
        }
        if (width, height) == self.size && self.buffers.len() == BUFFER_COUNT as usize {
            return true;
        }
        unsafe {
            self.buffers.clear();
            if self
                .swapchain
                .ResizeBuffers(BUFFER_COUNT, width, height, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))
                .is_err()
            {
                return false;
            }
            if self.fetch_buffers().is_err() {
                return false;
            }
            self.frame = 0;
            let _ = self.dcomp.Commit();
        }
        self.size = (width, height);
        true
    }
}

impl Drop for DCompPresenter {
    fn drop(&mut self) {
        // A textura do wgpu e os backbuffers só podem cair depois da última cópia.
        let _ = self.wait_idle();
        self.buffers.clear();
        unsafe {
            let _ = CloseHandle(self.fence_event);
        }
    }
}
