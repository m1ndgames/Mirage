use anyhow::ensure;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, LoadCursorW, PeekMessageW, PostQuitMessage,
    RegisterClassW, ShowWindow, TranslateMessage, IDC_ARROW, MA_NOACTIVATE, MSG, PM_REMOVE,
    SW_SHOWNOACTIVATE, WM_DESTROY, WM_MOUSEACTIVATE, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

pub struct OutputWindow {
    pub hwnd: HWND,
    pub width: i32,
    pub height: i32,
}

/// A borderless window covering exactly the given monitor rectangle. Topmost so
/// nothing on the cockpit screen hides it, NOACTIVATE + TOOLWINDOW so it never
/// takes focus from the game and stays out of Alt-Tab.
pub fn create(x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<OutputWindow> {
    let instance = unsafe { GetModuleHandleW(None) }?;
    let class_name = w!("MirageOutput");
    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: class_name,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        ..Default::default()
    };
    ensure!(unsafe { RegisterClassW(&class) } != 0, "RegisterClassW failed");

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class_name,
            w!("Mirage"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(OutputWindow { hwnd, width, height })
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // A click on the cockpit screen must not activate us either.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
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
