use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Device_ID_Size, CM_Get_Parent, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
};

use crate::monitors::{self, MonitorInfo};

pub type MonitorId = String;

/// One monitor of the current desktop with its hardware identity attached.
/// Plain data so config logic is testable without Win32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedMonitor {
    pub id: MonitorId,
    /// `\\.\DISPLAYn` – only valid until the next topology change.
    pub gdi_name: String,
    pub friendly_name: String,
    pub info: MonitorInfo,
    /// True when the id is a `path:` fallback bound to the connector.
    pub port_bound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    pub manufacturer: String,
    pub product: u16,
    pub serial_u32: u32,
    pub serial_str: Option<String>,
}

/// Parses the base EDID block. Returns None for anything that is not a
/// 128-byte block with the standard header.
pub fn parse_edid(bytes: &[u8]) -> Option<Edid> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if bytes.len() < 128 || bytes[..8] != HEADER {
        return None;
    }
    let mfg = u16::from_be_bytes([bytes[8], bytes[9]]);
    let letter = |v: u16| (b'A' + (v as u8).wrapping_sub(1)) as char;
    let manufacturer: String = [(mfg >> 10) & 0x1F, (mfg >> 5) & 0x1F, mfg & 0x1F]
        .iter()
        .map(|&v| letter(v))
        .collect();
    let product = u16::from_le_bytes([bytes[10], bytes[11]]);
    let serial_u32 = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let serial_str = [54usize, 72, 90, 108].iter().find_map(|&off| {
        let d = &bytes[off..off + 18];
        (d[0] == 0 && d[1] == 0 && d[2] == 0 && d[3] == 0xFF).then(|| {
            let text: String = d[5..18]
                .iter()
                .take_while(|&&b| b != 0x0A && b != 0)
                .map(|&b| b as char)
                .collect();
            text.trim().to_string()
        })
    });
    Some(Edid {
        manufacturer,
        product,
        serial_u32,
        serial_str,
    })
}

/// Serial strings that manufacturers use as placeholders: too short, or one
/// repeated character ("0", "1", "0000000", "AAAA").
pub fn is_weak_serial(s: &str) -> bool {
    let s = s.trim();
    let mut chars = s.chars();
    match chars.next() {
        None => true,
        Some(first) => s.len() < 4 || chars.all(|c| c == first),
    }
}

/// `\\?\DISPLAY#HPN3582#5&2f036cc&1&UID41219#{guid}` → `DISPLAY\HPN3582\5&2f036cc&1&UID41219`
pub fn instance_id_from_device_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("\\\\?\\")?;
    let without_guid = match rest.rfind("#{") {
        Some(i) => &rest[..i],
        None => rest,
    };
    let id = without_guid.replace('#', "\\");
    (id.matches('\\').count() >= 2).then_some(id)
}

/// The first ancestor that is a USB device whose instance id ends in a real
/// serial (Windows-generated ids contain '&', serials don't).
pub fn usb_serial(ancestors: &[String]) -> Option<String> {
    ancestors.iter().find_map(|id| {
        let last = id.rsplit('\\').next()?;
        (id.starts_with("USB\\") && !last.contains('&') && last.len() >= 4).then(|| last.to_string())
    })
}

/// Identity rules from the spec. Returns the id and whether it is port-bound.
pub fn choose_id(edid: Option<&Edid>, ancestors: &[String], instance_id: &str) -> (MonitorId, bool) {
    if let Some(e) = edid {
        if let Some(s) = e.serial_str.as_deref().filter(|s| !is_weak_serial(s)) {
            return (format!("edid:{}:{:04X}:{}", e.manufacturer, e.product, s), false);
        }
        if e.serial_u32 != 0 {
            return (
                format!("edid:{}:{:04X}:{}", e.manufacturer, e.product, e.serial_u32),
                false,
            );
        }
    }
    if let Some(s) = usb_serial(ancestors) {
        return (format!("usb:{s}"), false);
    }
    (format!("path:{instance_id}"), true)
}

fn utf16_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct TargetInfo {
    device_path: String,
    friendly_name: String,
}

/// GDI name (`\\.\DISPLAYn`) → target device path + friendly name, via DisplayConfig.
fn display_config_targets() -> anyhow::Result<HashMap<String, TargetInfo>> {
    let mut n_paths = 0u32;
    let mut n_modes = 0u32;
    let err = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes) };
    anyhow::ensure!(err == ERROR_SUCCESS, "GetDisplayConfigBufferSizes: {err:?}");
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
    let err = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut n_paths,
            paths.as_mut_ptr(),
            &mut n_modes,
            modes.as_mut_ptr(),
            None,
        )
    };
    anyhow::ensure!(err == ERROR_SUCCESS, "QueryDisplayConfig: {err:?}");

    let mut out = HashMap::new();
    for path in &paths[..n_paths as usize] {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };
        let ok_source = unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0;
        let ok_target = unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } == 0;
        if ok_source && ok_target {
            out.insert(
                utf16_to_string(&source.viewGdiDeviceName),
                TargetInfo {
                    device_path: utf16_to_string(&target.monitorDevicePath),
                    friendly_name: utf16_to_string(&target.monitorFriendlyDeviceName),
                },
            );
        }
    }
    Ok(out)
}

/// `HKLM\SYSTEM\CurrentControlSet\Enum\<instance>\Device Parameters\EDID`
fn read_edid(instance_id: &str) -> Option<Vec<u8>> {
    let sub = to_wide(&format!(
        "SYSTEM\\CurrentControlSet\\Enum\\{instance_id}\\Device Parameters"
    ));
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(sub.as_ptr()), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return None;
    }
    let mut len = 0u32;
    let mut result = None;
    if unsafe { RegQueryValueExW(key, w!("EDID"), None, None, None, Some(&mut len)) } == ERROR_SUCCESS && len >= 128 {
        let mut buf = vec![0u8; len as usize];
        if unsafe { RegQueryValueExW(key, w!("EDID"), None, None, Some(buf.as_mut_ptr()), Some(&mut len)) }
            == ERROR_SUCCESS
        {
            result = Some(buf);
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

/// Instance ids of the device's parent, grandparent, … (at most four levels).
fn ancestor_ids(instance_id: &str) -> Vec<String> {
    let id = to_wide(instance_id);
    let mut dev = 0u32;
    if unsafe { CM_Locate_DevNodeW(&mut dev, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) } != CR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for _ in 0..4 {
        let mut parent = 0u32;
        if unsafe { CM_Get_Parent(&mut parent, dev, 0) } != CR_SUCCESS {
            break;
        }
        let mut len = 0u32;
        if unsafe { CM_Get_Device_ID_Size(&mut len, parent, 0) } != CR_SUCCESS {
            break;
        }
        let mut buf = vec![0u16; len as usize + 1];
        if unsafe { CM_Get_Device_IDW(parent, &mut buf, 0) } != CR_SUCCESS {
            break;
        }
        out.push(utf16_to_string(&buf));
        dev = parent;
    }
    out
}

/// Every active monitor with its identity. Monitors DisplayConfig does not
/// know about (should not happen) fall back to a `path:` id on the GDI name.
pub fn attached() -> anyhow::Result<Vec<AttachedMonitor>> {
    let targets = display_config_targets()?;
    Ok(monitors::enumerate()?
        .into_iter()
        .map(|info| {
            let gdi_name = info.device_name.clone();
            let (id, friendly_name, port_bound) = match targets.get(&gdi_name) {
                Some(t) => {
                    let instance =
                        instance_id_from_device_path(&t.device_path).unwrap_or_else(|| t.device_path.clone());
                    let edid = read_edid(&instance).and_then(|b| parse_edid(&b));
                    let (id, port_bound) = choose_id(edid.as_ref(), &ancestor_ids(&instance), &instance);
                    let name = if t.friendly_name.is_empty() {
                        "Generic Monitor".to_string()
                    } else {
                        t.friendly_name.clone()
                    };
                    (id, name, port_bound)
                }
                None => (format!("path:{gdi_name}"), "Unknown monitor".to_string(), true),
            };
            AttachedMonitor {
                id,
                gdi_name,
                friendly_name,
                info,
                port_bound,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // First 128 EDID bytes of the HP 27xq with serial CNK04611FZ (dev machine).
    const HP: [u8; 128] = [
        0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x22, 0x0E, 0x82, 0x35, 0x00, 0x00, 0x00, 0x00, 0x2E, 0x1E,
        0x01, 0x04, 0xA5, 0x3C, 0x22, 0x78, 0x3B, 0x8C, 0xE5, 0xA5, 0x58, 0x50, 0xA0, 0x23, 0x0B, 0x50, 0x54, 0xA5,
        0x4B, 0x00, 0xD1, 0xC0, 0xA9, 0xC0, 0x81, 0xC0, 0xD1, 0x00, 0xB3, 0x00, 0x95, 0x00, 0x81, 0x00, 0xA9, 0x40,
        0x56, 0x5E, 0x00, 0xA0, 0xA0, 0xA0, 0x29, 0x50, 0x30, 0x20, 0x35, 0x00, 0x55, 0x50, 0x21, 0x00, 0x00, 0x1A,
        0x00, 0x00, 0x00, 0xFD, 0x00, 0x30, 0x90, 0xDF, 0xDF, 0x3C, 0x01, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x00, 0x00, 0x00, 0xFC, 0x00, 0x48, 0x50, 0x20, 0x32, 0x37, 0x78, 0x71, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x00, 0x00, 0x00, 0xFF, 0x00, 0x43, 0x4E, 0x4B, 0x30, 0x34, 0x36, 0x31, 0x31, 0x46, 0x5A, 0x0A, 0x20, 0x20,
        0x01, 0xD4,
    ];
    // The Winwing DisplayLink screen: serial descriptor holds just "1".
    const WINWING: [u8; 128] = [
        0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x48, 0xA7, 0x19, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01, 0x1E,
        0x01, 0x03, 0xA5, 0x1C, 0x10, 0x78, 0x02, 0xE1, 0xE7, 0xA1, 0x4F, 0x4C, 0x89, 0x23, 0x27, 0x4A, 0x54, 0x00,
        0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x64, 0x19, 0x00, 0x40, 0x41, 0x00, 0x26, 0x30, 0x00, 0x40, 0x06, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1A,
        0x00, 0x00, 0x00, 0xFD, 0x00, 0x0F, 0xF0, 0x0F, 0x7F, 0x11, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x00, 0x00, 0x00, 0xFC, 0x00, 0x55, 0x53, 0x42, 0x5F, 0x4D, 0x6F, 0x6E, 0x69, 0x74, 0x6F, 0x72, 0x0A, 0x20,
        0x00, 0x00, 0x00, 0xFF, 0x00, 0x31, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A,
        0x00, 0xDE,
    ];

    #[test]
    fn parses_hp_edid() {
        let e = parse_edid(&HP).unwrap();
        assert_eq!(e.manufacturer, "HPN");
        assert_eq!(e.product, 0x3582);
        assert_eq!(e.serial_u32, 0);
        assert_eq!(e.serial_str.as_deref(), Some("CNK04611FZ"));
    }

    #[test]
    fn parses_winwing_edid_with_placeholder_serial() {
        let e = parse_edid(&WINWING).unwrap();
        assert_eq!(e.manufacturer, "REG");
        assert_eq!(e.product, 0x0319);
        assert_eq!(e.serial_str.as_deref(), Some("1"));
    }

    #[test]
    fn rejects_short_or_unsigned_blobs() {
        assert!(parse_edid(&HP[..100]).is_none());
        let mut bad = HP;
        bad[1] = 0x00;
        assert!(parse_edid(&bad).is_none());
    }

    #[test]
    fn weak_serials() {
        assert!(is_weak_serial("1"));
        assert!(is_weak_serial("0"));
        assert!(is_weak_serial(""));
        assert!(is_weak_serial("0000000"));
        assert!(is_weak_serial("AAAA"));
        assert!(!is_weak_serial("CNK04611FZ"));
        assert!(!is_weak_serial("16780800"));
    }

    #[test]
    fn instance_id_from_device_path_strips_prefix_and_guid() {
        assert_eq!(
            instance_id_from_device_path(
                "\\\\?\\DISPLAY#HPN3582#5&2f036cc&1&UID41219#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some("DISPLAY\\HPN3582\\5&2f036cc&1&UID41219")
        );
        assert!(instance_id_from_device_path("garbage").is_none());
    }

    #[test]
    fn usb_serial_walks_to_the_first_real_usb_serial() {
        let ancestors = vec![
            "USB\\VID_17E9&PID_FF00&MI_00\\B&2D19B3BE&1&0000".to_string(),
            "USB\\VID_17E9&PID_FF00\\WWIN29320251005160820".to_string(),
            "USB\\ROOT_HUB30\\5&1234&0&0".to_string(),
        ];
        assert_eq!(usb_serial(&ancestors).as_deref(), Some("WWIN29320251005160820"));
        let gpu_only = vec!["PCI\\VEN_10DE&DEV_2C05\\76A3AD1E012DB04800".to_string()];
        assert_eq!(usb_serial(&gpu_only), None, "a PCI id is not a USB serial");
    }

    #[test]
    fn choose_id_prefers_edid_then_usb_then_path() {
        let hp = parse_edid(&HP).unwrap();
        let ww = parse_edid(&WINWING).unwrap();
        let ww_parents = vec!["USB\\VID_17E9&PID_FF00\\WWIN29320251005160820".to_string()];
        assert_eq!(
            choose_id(Some(&hp), &[], "DISPLAY\\HPN3582\\5&x"),
            ("edid:HPN:3582:CNK04611FZ".to_string(), false)
        );
        assert_eq!(
            choose_id(Some(&ww), &ww_parents, "DISPLAY\\REG0319\\c&x"),
            ("usb:WWIN29320251005160820".to_string(), false)
        );
        assert_eq!(
            choose_id(Some(&ww), &[], "DISPLAY\\REG0319\\c&x"),
            ("path:DISPLAY\\REG0319\\c&x".to_string(), true)
        );
        assert_eq!(
            choose_id(None, &[], "DISPLAY\\Z\\1"),
            ("path:DISPLAY\\Z\\1".to_string(), true)
        );
        let mut numeric = hp.clone();
        numeric.serial_str = None;
        numeric.serial_u32 = 16780800;
        assert_eq!(choose_id(Some(&numeric), &[], "x").0, "edid:HPN:3582:16780800");
    }
}
