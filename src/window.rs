use std::cell::Cell;
use std::ffi::c_void;

use anyhow::ensure;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos,
    GetWindowLongPtrW, KillTimer, LoadCursorW, LoadIconW, PeekMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, ShowWindow, TrackPopupMenu,
    TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, IDC_ARROW, IDC_CROSS, IDI_APPLICATION, MA_NOACTIVATE, MF_SEPARATOR,
    MF_STRING, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
    WM_APP, WM_CREATE, WM_DESTROY, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
    WM_MOUSEACTIVATE, WM_QUIT, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
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

#[allow(clippy::too_many_arguments)]
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

/// What the user picked in the tray icon's menu (or by clicking it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowSettings = 1,
    SelectRegion = 2,
    Quit = 3,
}

/// Flags raised by the hidden message window, polled by the render loop.
#[derive(Default)]
pub struct WindowSignals {
    pub topology_changed: Cell<bool>,
    pub hotkey: Cell<bool>,
    pub tray: Cell<Option<TrayAction>>,
}

const TOPOLOGY_TIMER: usize = 1;
const TRAY_ID: u32 = 1;
const WM_TRAY: u32 = WM_APP + 1;

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default(),
        ..Default::default()
    };
    for (dst, src) in data.szTip.iter_mut().zip("Mirage".encode_utf16()) {
        *dst = src;
    }
    data
}

/// Adds the tray icon to the message window. Explorer restarts are handled
/// inside the window procedure (the "TaskbarCreated" broadcast re-adds it).
pub fn add_tray_icon(hwnd: HWND) -> anyhow::Result<()> {
    ensure!(
        unsafe { Shell_NotifyIconW(NIM_ADD, &tray_data(hwnd)) }.as_bool(),
        "Shell_NotifyIcon(NIM_ADD) failed"
    );
    Ok(())
}

pub fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
    }
}

/// Right-click menu of the tray icon. Blocks until the user picks or dismisses.
unsafe fn tray_menu(hwnd: HWND) -> Option<TrayAction> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(menu, MF_STRING, TrayAction::ShowSettings as usize, w!("Settings"));
        let _ = AppendMenuW(menu, MF_STRING, TrayAction::SelectRegion as usize, w!("Select region"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, TrayAction::Quit as usize, w!("Quit"));
        let mut pos = POINT::default();
        let _ = GetCursorPos(&mut pos);
        // Without this the menu does not close when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pos.x,
            pos.y,
            None,
            hwnd,
            None,
        )
        .0;
        let _ = DestroyMenu(menu);
        match picked {
            1 => Some(TrayAction::ShowSettings),
            2 => Some(TrayAction::SelectRegion),
            3 => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

/// Hidden window that receives WM_DISPLAYCHANGE / WM_DEVICECHANGE (debounced
/// into `topology_changed` 500 ms after the last one) and WM_HOTKEY. It must
/// be a real top-level window (never shown): message-only windows do not get
/// broadcast messages.
pub fn create_message_window(signals: *const WindowSignals) -> anyhow::Result<HWND> {
    register_class(w!("MirageMessages"), Some(message_proc), IDC_ARROW)?;
    create(
        WS_EX_TOOLWINDOW,
        w!("MirageMessages"),
        0,
        0,
        0,
        0,
        None,
        signals as *mut c_void,
    )
}

unsafe extern "system" fn message_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let signals = userdata::<WindowSignals>(hwnd, msg, lparam);
        static TASKBAR_CREATED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let taskbar_created = *TASKBAR_CREATED.get_or_init(|| RegisterWindowMessageW(w!("TaskbarCreated")));
        match msg {
            WM_CREATE => LRESULT(0),
            WM_TRAY => {
                let action = match (lparam.0 & 0xFFFF) as u32 {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => Some(TrayAction::ShowSettings),
                    WM_RBUTTONUP => tray_menu(hwnd),
                    _ => None,
                };
                if let (Some(a), false) = (action, signals.is_null()) {
                    (*signals).tray.set(Some(a));
                }
                LRESULT(0)
            }
            m if m == taskbar_created => {
                let _ = add_tray_icon(hwnd);
                LRESULT(0)
            }
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
