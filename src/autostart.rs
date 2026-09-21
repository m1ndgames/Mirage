use anyhow::ensure;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("Mirage");

/// The command Windows runs at login: this executable, minimised to the tray.
fn command() -> anyhow::Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" --minimized", exe.display()))
}

fn open(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> anyhow::Result<HKEY> {
    let mut key = HKEY::default();
    let err = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            access,
            None,
            &mut key,
            None,
        )
    };
    ensure!(err == ERROR_SUCCESS, "opening HKCU\\...\\Run failed: {err:?}");
    Ok(key)
}

/// True when the Run entry exists and points at this executable.
pub fn is_enabled() -> bool {
    let Ok(key) = open(KEY_QUERY_VALUE) else { return false };
    let mut len = 0u32;
    let mut enabled = false;
    if unsafe { RegQueryValueExW(key, VALUE, None, None, None, Some(&mut len)) } == ERROR_SUCCESS && len >= 2 {
        let mut buf = vec![0u8; len as usize];
        if unsafe { RegQueryValueExW(key, VALUE, None, None, Some(buf.as_mut_ptr()), Some(&mut len)) } == ERROR_SUCCESS
        {
            let wide: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let text = String::from_utf16_lossy(&wide);
            let text = text.trim_end_matches('\0');
            enabled = command().map(|c| c.eq_ignore_ascii_case(text)).unwrap_or(false);
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    enabled
}

/// Creates or removes the Run entry.
pub fn set(enabled: bool) -> anyhow::Result<()> {
    let key = open(KEY_SET_VALUE)?;
    let err = if enabled {
        let wide: Vec<u16> = command()?.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
        unsafe { RegSetValueExW(key, VALUE, None, REG_SZ, Some(&bytes)) }
    } else {
        unsafe { RegDeleteValueW(key, VALUE) }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    // Deleting an entry that isn't there is fine.
    ensure!(
        err == ERROR_SUCCESS || !enabled,
        "writing the Run entry failed: {err:?}"
    );
    Ok(())
}
