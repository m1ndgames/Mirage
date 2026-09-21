use anyhow::ensure;
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

/// One monitor of the current desktop. Plain data so selection logic is testable
/// without Win32; `handle` is the `HMONITOR` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub handle: isize,
    pub device_name: String,
    /// Virtual-desktop position and size in physical pixels.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub is_primary: bool,
}

/// Asks Windows for every monitor of the current desktop. Requires the process to
/// be DPI aware already, otherwise the rectangles come back virtualised.
pub fn enumerate() -> anyhow::Result<Vec<MonitorInfo>> {
    unsafe extern "system" fn callback(hmonitor: HMONITOR, _hdc: HDC, _rect: *mut RECT, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<MonitorInfo>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut MONITORINFO) }.as_bool() {
            let name_len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let r = info.monitorInfo.rcMonitor;
            list.push(MonitorInfo {
                handle: hmonitor.0 as isize,
                device_name: String::from_utf16_lossy(&info.szDevice[..name_len]),
                x: r.left,
                y: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        TRUE
    }

    let mut list: Vec<MonitorInfo> = Vec::new();
    let ok = unsafe { EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut list as *mut _ as isize)) };
    ensure!(ok.as_bool(), "EnumDisplayMonitors failed");
    Ok(list)
}
