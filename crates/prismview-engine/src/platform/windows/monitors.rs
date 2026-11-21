//! Enumeração de monitores com DPI independente por monitor.
//!
//! O processo roda em `PER_MONITOR_AWARE_V2`: todas as coordenadas (retângulos, cursor,
//! `SetWindowPos`) são pixels físicos do desktop virtual, e cada monitor reporta o próprio
//! DPI efetivo (`scale_factor = dpi / 96`). Cada janela de wallpaper cobre exatamente um
//! monitor, então cada superfície wgpu tem tamanho físico e escala próprios.

use ::windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use ::windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use ::windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use ::windows::Win32::UI::WindowsAndMessaging::{MONITORINFOF_PRIMARY, USER_DEFAULT_SCREEN_DPI};

use super::util::last_os_error;
use crate::platform::{MonitorInfo, PlatformError, PlatformResult};

#[derive(Debug, Clone, PartialEq)]
pub struct Win32Monitor {
    /// Dados neutros (id = nome do dispositivo, ex. `\\.\DISPLAY1`).
    pub info: MonitorInfo,
    /// `HMONITOR` bruto. Pode mudar após reconfiguração sem o layout mudar.
    pub hmonitor: isize,
    /// DPI efetivo (96 = 100%).
    pub dpi: u32,
    /// Área útil (sem barra de tarefas), pixels físicos.
    pub work_area: RECT,
}

impl Win32Monitor {
    /// Retângulo do monitor em pixels físicos do desktop virtual.
    pub fn rect(&self) -> RECT {
        RECT {
            left: self.info.x,
            top: self.info.y,
            right: self.info.x.saturating_add(self.info.width as i32),
            bottom: self.info.y.saturating_add(self.info.height as i32),
        }
    }

    fn same_layout(&self, other: &Self) -> bool {
        self.info == other.info && self.dpi == other.dpi && self.work_area == other.work_area
    }
}

/// Layout (ids, retângulos, DPI, primário) idêntico, ignorando `HMONITOR`.
pub fn same_layout(a: &[Win32Monitor], b: &[Win32Monitor]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same_layout(y))
}

/// Ativa DPI Per-Monitor v2. Se o processo já tiver contexto (manifesto/Tauri),
/// garante ao menos a thread atual — é ela que cria as janelas e o loop.
pub fn ensure_per_monitor_dpi_awareness() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }
}

/// Preenche `out` (reaproveitando a capacidade) com os monitores ativos:
/// primário primeiro, depois da esquerda para a direita, de cima para baixo.
pub fn enumerate(out: &mut Vec<Win32Monitor>) -> PlatformResult<()> {
    out.clear();
    let ok = unsafe {
        EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_enum_proc),
            LPARAM(&mut *out as *mut Vec<Win32Monitor> as isize),
        )
    };
    if !ok.as_bool() {
        return Err(last_os_error("EnumDisplayMonitors"));
    }
    if out.is_empty() {
        return Err(PlatformError::Os("nenhum monitor ativo".into()));
    }
    out.sort_by(|a, b| {
        b.info
            .primary
            .cmp(&a.info.primary)
            .then(a.info.x.cmp(&b.info.x))
            .then(a.info.y.cmp(&b.info.y))
    });
    Ok(())
}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let out = &mut *(data.0 as *mut Vec<Win32Monitor>);
    if let Some(monitor) = query_monitor(hmonitor) {
        out.push(monitor);
    }
    BOOL(1)
}

unsafe fn query_monitor(hmonitor: HMONITOR) -> Option<Win32Monitor> {
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(hmonitor, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
        return None;
    }

    let r = mi.monitorInfo.rcMonitor;
    let width = u32::try_from(r.right.saturating_sub(r.left)).unwrap_or(0);
    let height = u32::try_from(r.bottom.saturating_sub(r.top)).unwrap_or(0);
    if width == 0 || height == 0 {
        return None;
    }

    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    let dpi = match GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
        Ok(()) if dpi_x > 0 => dpi_x,
        _ => USER_DEFAULT_SCREEN_DPI,
    };

    let name_len = mi
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(mi.szDevice.len());
    let mut id = String::from_utf16_lossy(&mi.szDevice[..name_len]);
    if id.is_empty() {
        id = format!("monitor@{},{}", r.left, r.top);
    }

    Some(Win32Monitor {
        info: MonitorInfo {
            id,
            x: r.left,
            y: r.top,
            width,
            height,
            scale_factor: dpi as f32 / USER_DEFAULT_SCREEN_DPI as f32,
            primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        },
        hmonitor: hmonitor.0 as isize,
        dpi,
        work_area: mi.monitorInfo.rcWork,
    })
}
