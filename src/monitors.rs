use anyhow::ensure;
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
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

/// Explicit index, otherwise the primary monitor, otherwise the first one.
pub fn choose_source(monitors: &[MonitorInfo], wanted: Option<usize>) -> Result<usize, String> {
    match wanted {
        Some(i) => check_index(monitors, i),
        None => Ok(monitors.iter().position(|m| m.is_primary).unwrap_or(0)),
    }
}

/// Explicit index, otherwise the smallest monitor (by area) that is not the
/// source – the cockpit screen is always the smallest one attached.
pub fn choose_target(
    monitors: &[MonitorInfo],
    wanted: Option<usize>,
    source: usize,
) -> Result<usize, String> {
    let i = match wanted {
        Some(i) => check_index(monitors, i)?,
        None => monitors
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != source)
            .min_by_key(|(_, m)| m.width as i64 * m.height as i64)
            .map(|(i, _)| i)
            .ok_or_else(|| "need at least two monitors".to_string())?,
    };
    if i == source {
        return Err(format!("target monitor {i} is the source monitor"));
    }
    Ok(i)
}

fn check_index(monitors: &[MonitorInfo], i: usize) -> Result<usize, String> {
    if i < monitors.len() {
        Ok(i)
    } else {
        Err(format!("monitor index {i} out of range (0..{})", monitors.len()))
    }
}

pub fn describe(monitors: &[MonitorInfo]) -> String {
    let mut out = String::from("idx  device         position      size       \n");
    for (i, m) in monitors.iter().enumerate() {
        out.push_str(&format!(
            "{:<4} {:<14} {:>6},{:<6} {:>5}x{:<5} {}\n",
            i,
            m.device_name,
            m.x,
            m.y,
            m.width,
            m.height,
            if m.is_primary { "primary" } else { "" }
        ));
    }
    out
}

/// Asks Windows for every monitor of the current desktop. Requires the process to
/// be DPI aware already, otherwise the rectangles come back virtualised.
pub fn enumerate() -> anyhow::Result<Vec<MonitorInfo>> {
    unsafe extern "system" fn callback(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<MonitorInfo>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo as *mut MONITORINFO) }.as_bool() {
            let name_len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
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
    let ok = unsafe {
        EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut list as *mut _ as isize))
    };
    ensure!(ok.as_bool(), "EnumDisplayMonitors failed");
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(handle: isize, w: i32, h: i32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            handle,
            device_name: format!("\\\\.\\DISPLAY{handle}"),
            x: 0,
            y: 0,
            width: w,
            height: h,
            is_primary: primary,
        }
    }

    fn desk() -> Vec<MonitorInfo> {
        vec![mon(1, 2560, 1440, false), mon(2, 2560, 1440, true), mon(3, 768, 1024, false)]
    }

    #[test]
    fn source_defaults_to_primary() {
        assert_eq!(choose_source(&desk(), None), Ok(1));
    }

    #[test]
    fn source_honours_explicit_index() {
        assert_eq!(choose_source(&desk(), Some(2)), Ok(2));
    }

    #[test]
    fn source_index_out_of_range_is_an_error() {
        assert!(choose_source(&desk(), Some(7)).is_err());
    }

    #[test]
    fn source_without_primary_falls_back_to_first() {
        let d = vec![mon(5, 100, 100, false), mon(6, 100, 100, false)];
        assert_eq!(choose_source(&d, None), Ok(0));
    }

    #[test]
    fn target_defaults_to_smallest_non_source_monitor() {
        assert_eq!(choose_target(&desk(), None, 1), Ok(2));
    }

    #[test]
    fn target_honours_explicit_index() {
        assert_eq!(choose_target(&desk(), Some(0), 1), Ok(0));
    }

    #[test]
    fn target_may_not_equal_source() {
        assert!(choose_target(&desk(), Some(1), 1).is_err());
    }

    #[test]
    fn target_needs_a_second_monitor() {
        let d = vec![mon(1, 100, 100, true)];
        assert!(choose_target(&d, None, 0).is_err());
    }

    #[test]
    fn describe_lists_index_name_geometry_and_primary_flag() {
        let s = describe(&desk());
        assert!(s.contains("0"));
        assert!(s.contains("\\\\.\\DISPLAY3"));
        assert!(s.contains("768x1024"));
        assert!(s.contains("primary"));
    }
}
