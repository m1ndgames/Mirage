use std::cell::Cell;
use std::ffi::c_void;

use anyhow::ensure;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetWindowLongPtrW, KillTimer, LoadCursorW, PeekMessageW,
    PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW, ShowWindow, TranslateMessage, CREATESTRUCTW,
    GWLP_USERDATA, HWND_MESSAGE, IDC_ARROW, IDC_CROSS, MA_NOACTIVATE, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE,
    WM_CREATE, WM_DESTROY, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_HOTKEY, WM_MOUSEACTIVATE, WM_QUIT, WM_TIMER, WNDCLASSW,
    WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

pub struct OutputWindow {
    pub hwnd: HWND,
    pub width: i32,
    pub height: i32,
}

fn register_class(name: PCWSTR, proc: WNDPROC, cursor: PCWSTR) -> anyhow::Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }?;
    let class = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: instance.into(),
        lpszClassName: name,
        hCursor: unsafe { LoadCursorW(None, cursor) }?,
        ..Default::default()
    };
    // Registering twice fails with ERROR_CLASS_ALREADY_EXISTS – harmless.
    unsafe { RegisterClassW(&class) };
    Ok(())
}

fn create(
    ex_style: WINDOW_EX_STYLE,
    class: PCWSTR,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: Option<HWND>,
    userdata: *mut c_void,
) -> anyhow::Result<HWND> {
    let instance = unsafe { GetModuleHandleW(None) }?;
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            class,
            w!("Mirage"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            parent,
            None,
            Some(instance.into()),
            Some(userdata as *const c_void),
        )
    }?;
    ensure!(!hwnd.is_invalid(), "CreateWindowExW returned null");
    Ok(hwnd)
}

/// A borderless window covering exactly the given monitor rectangle. Topmost so
/// nothing on the cockpit screen hides it, NOACTIVATE + TOOLWINDOW so it never
/// takes focus from the game and stays out of Alt-Tab.
pub fn create_output_window(x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<OutputWindow> {
    register_class(w!("MirageOutput"), Some(output_proc), IDC_ARROW)?;
    let hwnd = create(
        WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
        w!("MirageOutput"),
        x,
        y,
        width,
        height,
        None,
        std::ptr::null_mut(),
    )?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(OutputWindow { hwnd, width, height })
}

unsafe extern "system" fn output_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // A click on the cockpit screen must not activate us either.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Selection overlay window: topmost, *activatable* (needs keyboard + mouse
/// capture), crosshair cursor. `userdata` is stored in GWLP_USERDATA for `proc`.
pub fn create_overlay_window(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    userdata: *mut c_void,
    proc: WNDPROC,
) -> anyhow::Result<HWND> {
    register_class(w!("MirageOverlay"), proc, IDC_CROSS)?;
    create(WS_EX_TOPMOST, w!("MirageOverlay"), x, y, width, height, None, userdata)
}

/// Flags raised by the hidden message window, polled by the render loop.
#[derive(Default)]
pub struct WindowSignals {
    pub topology_changed: Cell<bool>,
    pub hotkey: Cell<bool>,
}

const TOPOLOGY_TIMER: usize = 1;

/// Hidden window that receives WM_DISPLAYCHANGE / WM_DEVICECHANGE (debounced
/// into `topology_changed` 500 ms after the last one) and WM_HOTKEY.
pub fn create_message_window(signals: *const WindowSignals) -> anyhow::Result<HWND> {
    register_class(w!("MirageMessages"), Some(message_proc), IDC_ARROW)?;
    create(WINDOW_EX_STYLE(0), w!("MirageMessages"), 0, 0, 0, 0, Some(HWND_MESSAGE), signals as *mut c_void)
}

unsafe extern "system" fn message_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let signals = userdata::<WindowSignals>(hwnd, msg, lparam);
        match msg {
            WM_CREATE => LRESULT(0),
            WM_DISPLAYCHANGE | WM_DEVICECHANGE => {
                SetTimer(Some(hwnd), TOPOLOGY_TIMER, 500, None);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == TOPOLOGY_TIMER => {
                let _ = KillTimer(Some(hwnd), TOPOLOGY_TIMER);
                if !signals.is_null() {
                    (*signals).topology_changed.set(true);
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                if !signals.is_null() {
                    (*signals).hotkey.set(true);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Reads the userdata pointer a window was created with (see `create_*`).
/// On WM_CREATE it also stores it, so call this first in every window procedure.
pub unsafe fn userdata<T>(hwnd: HWND, msg: u32, lparam: LPARAM) -> *const T {
    unsafe {
        if msg == WM_CREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const T
    }
}

/// Client coordinates from a mouse message's LPARAM (physical pixels).
pub fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// Drains the thread's message queue. Returns false once WM_QUIT was seen.
pub fn pump_messages() -> bool {
    let mut msg = MSG::default();
    while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        if msg.message == WM_QUIT {
            return false;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}
