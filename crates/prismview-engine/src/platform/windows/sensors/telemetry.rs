//! Telemetria do sistema → `SignalBus`.
//!
//! - CPU: delta de `GetSystemTimes` (porte do Estúdio, sem estado global).
//! - RAM: `GlobalMemoryStatusEx`.
//! - GPU: PDH `\GPU Engine(*engtype_3D)\Utilization Percentage` (soma das instâncias, como o
//!   Gerenciador de Tarefas) + VRAM via `IDXGIAdapter3::QueryVideoMemoryInfo`. A query PDH só
//!   existe enquanto há demanda `TELEMETRY` (fechada ao sair).
//! - Energia: `GetSystemPowerStatus` (bateria, tomada, economia de bateria). Barata: amostrada em
//!   `power_interval` também quando só `POWER` é pedido (governador).
//!
//! Sem demanda a thread estaciona (`park`) → 0 % de CPU.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_QUERY_VIDEO_MEMORY_INFO,
};
use ::windows::core::Interface;

use super::{SensorsConfig, Shared};
use crate::engine_core::signals::{PowerValues, SignalKind, TelemetryValues};

const GPU_COUNTER_PATH: &str = "\\GPU Engine(*engtype_3D)\\Utilization Percentage";
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;
const BYTES_PER_MB: f32 = 1024.0 * 1024.0;
const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

// ───────────── FFI kernel32 / pdh (tipos estáveis, sem depender de features do crate) ─────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    fn as_u64(self) -> u64 {
        ((self.high as u64) << 32) | self.low as u64
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
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
}

/// `PDH_FMT_COUNTERVALUE` com formato double (união de 8 B alinhada após `CStatus`).
#[repr(C)]
#[derive(Clone, Copy)]
struct PdhFmtCounterValue {
    c_status: u32,
    double_value: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PdhFmtCounterValueItemW {
    name: *mut u16,
    value: PdhFmtCounterValue,
}

const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const PDH_MORE_DATA: u32 = 0x8000_07D2;
const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

#[link(name = "pdh", kind = "raw-dylib")]
extern "system" {
    fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut isize) -> u32;
    fn PdhAddEnglishCounterW(query: isize, path: *const u16, user_data: usize, counter: *mut isize) -> u32;
    fn PdhCollectQueryData(query: isize) -> u32;
    fn PdhGetFormattedCounterArrayW(
        counter: isize,
        format: u32,
        buffer_size: *mut u32,
        item_count: *mut u32,
        items: *mut PdhFmtCounterValueItemW,
    ) -> u32;
    fn PdhCloseQuery(query: isize) -> u32;
}

// ───────────── amostradores ─────────────

/// RAM `(percent, usada_gb, total_gb)`.
pub(super) fn sample_memory() -> (f32, f32, f32) {
    let mut ms = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut ms) } == 0 || ms.total_phys == 0 {
        return (0.0, 0.0, 0.0);
    }
    let used = ms.total_phys.saturating_sub(ms.avail_phys);
    let pct = (used as f64 / ms.total_phys as f64 * 100.0).clamp(0.0, 100.0);
    (
        round1(pct as f32),
        round1((used as f64 / BYTES_PER_GB) as f32),
        round1((ms.total_phys as f64 / BYTES_PER_GB) as f32),
    )
}

pub(super) fn sample_power() -> PowerValues {
    let mut ps = SystemPowerStatus::default();
    if unsafe { GetSystemPowerStatus(&mut ps) } == 0 {
        return PowerValues::default();
    }
    // battery_flag 128 = sem bateria; 255 = desconhecido.
    let has_battery = ps.battery_flag & 128 == 0 && ps.battery_flag != 255;
    PowerValues {
        battery_percent: (has_battery && ps.battery_life_percent <= 100)
            .then_some(ps.battery_life_percent as f32),
        on_battery: has_battery && ps.ac_line_status == 0,
        battery_saver: ps.system_status_flag == 1,
    }
}

#[inline]
fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

#[derive(Default)]
struct CpuSampler {
    prev: Option<(u64, u64)>,
    last: f32,
}

impl CpuSampler {
    fn sample(&mut self) -> f32 {
        let (mut idle, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default());
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return self.last;
        }
        // kernel inclui idle.
        let idle = idle.as_u64();
        let total = kernel.as_u64().saturating_add(user.as_u64());
        if let Some((pi, pt)) = self.prev {
            let di = idle.saturating_sub(pi);
            let dt = total.saturating_sub(pt);
            if dt > 0 {
                self.last = round1((dt.saturating_sub(di) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0) as f32);
            }
        }
        self.prev = Some((idle, total));
        self.last
    }
}

struct GpuUsage {
    query: isize,
    counter: isize,
    primed: bool,
    items: Vec<PdhFmtCounterValueItemW>,
}

impl GpuUsage {
    fn open() -> Option<Self> {
        let path: Vec<u16> = GPU_COUNTER_PATH.encode_utf16().chain(std::iter::once(0)).collect();
        let mut query = 0isize;
        if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }
        let mut counter = 0isize;
        if unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) } != 0 {
            unsafe { PdhCloseQuery(query) };
            return None;
        }
        let mut s = Self {
            query,
            counter,
            primed: false,
            items: Vec::with_capacity(64),
        };
        // Contador de taxa: a 1ª coleta só prepara a referência.
        s.primed = unsafe { PdhCollectQueryData(query) } == 0;
        Some(s)
    }

    /// Soma da utilização 3D de todas as instâncias (processos × engines), limitada a 100.
    fn sample(&mut self) -> Option<f32> {
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        if !self.primed {
            self.primed = true;
            return None;
        }
        let item_size = std::mem::size_of::<PdhFmtCounterValueItemW>() as u32;
        for _ in 0..3 {
            let mut bytes = (self.items.capacity() as u32).saturating_mul(item_size);
            let mut count = 0u32;
            let ptr = if self.items.capacity() == 0 {
                std::ptr::null_mut()
            } else {
                self.items.as_mut_ptr()
            };
            let status = unsafe {
                PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE | PDH_FMT_NOCAP100,
                    &mut bytes,
                    &mut count,
                    ptr,
                )
            };
            if status == PDH_MORE_DATA {
                // Buffer inclui as strings dos nomes após os itens: cresce em itens inteiros.
                let need = (bytes as usize).div_ceil(item_size as usize) + 8;
                self.items.reserve(need.saturating_sub(self.items.len()));
                continue;
            }
            if status != 0 {
                return None;
            }
            let count = (count as usize).min(self.items.capacity());
            // SAFETY: PDH escreveu `count` itens válidos no buffer (capacidade reservada).
            let items = unsafe { std::slice::from_raw_parts(self.items.as_ptr(), count) };
            let mut sum = 0.0f64;
            for item in items {
                let st = item.value.c_status;
                if st == PDH_CSTATUS_VALID_DATA || st == PDH_CSTATUS_NEW_DATA {
                    let v = item.value.double_value;
                    if v.is_finite() && v > 0.0 {
                        sum += v;
                    }
                }
            }
            return Some(round1(sum.clamp(0.0, 100.0) as f32));
        }
        None
    }
}

impl Drop for GpuUsage {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// VRAM local do adapter (mesmo LUID do renderer quando informado; senão o 1º adapter de hardware).
struct GpuMemory {
    adapter: IDXGIAdapter3,
}

impl GpuMemory {
    fn open(luid: Option<(u32, i32)>) -> Option<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut index = 0u32;
            while let Ok(adapter) = factory.EnumAdapters1(index) {
                index += 1;
                let Ok(desc) = adapter.GetDesc1() else {
                    continue;
                };
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 {
                    continue;
                }
                let matches = match luid {
                    Some((low, high)) => {
                        desc.AdapterLuid.LowPart == low && desc.AdapterLuid.HighPart == high
                    }
                    None => true,
                };
                if matches {
                    if let Ok(adapter) = adapter.cast::<IDXGIAdapter3>() {
                        return Some(Self { adapter });
                    }
                }
            }
            None
        }
    }

    /// `(usada_mb, orçamento_mb)`.
    fn sample(&self) -> Option<(f32, f32)> {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        unsafe {
            self.adapter
                .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
                .ok()?;
        }
        Some((
            round1(info.CurrentUsage as f32 / BYTES_PER_MB),
            round1(info.Budget as f32 / BYTES_PER_MB),
        ))
    }
}

// ───────────── worker ─────────────

pub(super) fn run(shared: Arc<Shared>, config: SensorsConfig) {
    let mut cpu = CpuSampler::default();
    let mut gpu: Option<GpuUsage> = None;
    let mut vram: Option<GpuMemory> = None;
    let mut gpu_tried = false;
    let mut last_power: Option<Instant> = None;

    while !shared.stop.load(Ordering::Relaxed) {
        let demand = shared.bus.demand();
        let gpu_telemetry = demand.contains(SignalKind::GPU_TELEMETRY);
        let telemetry = demand.contains(SignalKind::TELEMETRY) || gpu_telemetry;
        let power = demand.contains(SignalKind::POWER) || telemetry;

        if !gpu_telemetry && (gpu.is_some() || vram.is_some() || gpu_tried) {
            // Libera a query PDH e o adapter DXGI assim que ninguém mais pede telemetria de GPU.
            gpu = None;
            vram = None;
            gpu_tried = false;
        }
        if !telemetry {
            cpu = CpuSampler::default();
        }

        if telemetry {
            if gpu_telemetry && !gpu_tried {
                gpu_tried = true;
                gpu = GpuUsage::open();
                vram = GpuMemory::open(config.gpu_adapter_luid);
            }
            let (ram_percent, ram_used_gb, ram_total_gb) = sample_memory();
            let (gpu_mem_used_mb, gpu_mem_budget_mb) = match vram.as_ref().and_then(GpuMemory::sample) {
                Some((u, b)) => (Some(u), Some(b)),
                None => (None, None),
            };
            let values = TelemetryValues {
                cpu_percent: cpu.sample(),
                ram_percent,
                ram_used_gb,
                ram_total_gb,
                gpu_percent: gpu.as_mut().and_then(GpuUsage::sample),
                gpu_mem_used_mb,
                gpu_mem_budget_mb,
            };
            shared.bus.publish_telemetry(&values);
        }

        if power {
            let due = last_power.map_or(true, |t| {
                telemetry || t.elapsed() >= config.power_interval
            });
            if due {
                shared.bus.publish_power(&sample_power());
                last_power = Some(Instant::now());
            }
        }

        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        if telemetry {
            std::thread::park_timeout(config.telemetry_interval.max(Duration::from_millis(250)));
        } else if power {
            std::thread::park_timeout(config.power_interval.max(Duration::from_secs(1)));
        } else {
            shared.idle_wait();
        }
    }
}
