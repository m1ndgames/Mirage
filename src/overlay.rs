use std::cell::Cell;
use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, SetForegroundWindow, ShowWindow, SW_SHOW, WM_CREATE, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN,
};

use crate::capture;
use crate::d3d::D3d;
use crate::geometry::Rect;
use crate::identity::AttachedMonitor;
use crate::renderer::{Pipeline, SourceTexture, SwapChainTarget};
use crate::window;

const MIN_SIZE: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done { monitor_index: usize, rect: Rect },
    Cancelled,
}

/// Drag state shared by all overlay windows (single-threaded: only the render
/// thread touches it, from window procedures and the loop).
struct Shared {
    anchor: Cell<Option<(usize, i32, i32)>>,
    current: Cell<Option<(usize, Rect)>>,
    outcome: Cell<Option<Outcome>>,
}

/// Per-window context handed to the window procedure via GWLP_USERDATA.
struct WindowCtx {
    shared: *const Shared,
    index: usize,
    width: i32,
    height: i32,
}

struct OverlayWindow {
    hwnd: HWND,
    target: SwapChainTarget,
    frame: SourceTexture,
    index: usize,
    _ctx: Box<WindowCtx>,
    last_drawn: Option<Option<Rect>>,
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

pub struct Selection {
    // Declared first so the windows are destroyed before `shared`, which their
    // window procedures dereference until the last WM_DESTROY.
    windows: Vec<OverlayWindow>,
    shared: Box<Shared>,
}

impl Selection {
    /// Freezes every attached monitor and covers it with an overlay.
    pub fn start(d3d: &D3d, attached: &[AttachedMonitor]) -> anyhow::Result<Selection> {
        let shared = Box::new(Shared {
            anchor: Cell::new(None),
            current: Cell::new(None),
            outcome: Cell::new(None),
        });
        let mut windows = Vec::new();
        for (index, m) in attached.iter().enumerate() {
            let info = &m.info;
            let frame = capture::grab_one_frame(d3d, info.handle, info.width as u32, info.height as u32, 1000)?;
            let ctx = Box::new(WindowCtx {
                shared: &*shared as *const Shared,
                index,
                width: info.width,
                height: info.height,
            });
            let hwnd = window::create_overlay_window(
                info.x,
                info.y,
                info.width,
                info.height,
                &*ctx as *const WindowCtx as *mut c_void,
                Some(overlay_proc),
            )?;
            let target = SwapChainTarget::new(d3d, hwnd, info.width as u32, info.height as u32)?;
            windows.push(OverlayWindow {
                hwnd,
                target,
                frame,
                index,
                _ctx: ctx,
                last_drawn: None,
            });
        }
        for w in &windows {
            unsafe {
                let _ = ShowWindow(w.hwnd, SW_SHOW);
            }
        }
        if let Some(first) = windows.first() {
            unsafe {
                let _ = SetForegroundWindow(first.hwnd);
            }
        }
        Ok(Selection { windows, shared })
    }

    /// Repaints overlays whose selection state changed since the last draw.
    pub fn redraw(&mut self, d3d: &D3d, pipeline: &Pipeline) {
        let current = self.shared.current.get();
        for w in &mut self.windows {
            let sel = match current {
                Some((i, r)) if i == w.index => Some(r),
                _ => None,
            };
            if w.last_drawn == Some(sel) {
                continue;
            }
            w.target.wait_for_frame_slot();
            w.target.clear(&d3d.context);
            pipeline.draw_overlay(&d3d.context, &w.target, &w.frame, sel);
            let _ = w.target.present();
            w.last_drawn = Some(sel);
        }
    }

    pub fn poll(&self) -> Option<Outcome> {
        self.shared.outcome.get()
    }
}

fn normalised(ax: i32, ay: i32, x: i32, y: i32, width: i32, height: i32) -> Rect {
    let (x0, x1) = (ax.min(x), ax.max(x));
    let (y0, y1) = (ay.min(y), ay.max(y));
    Rect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(1),
        h: (y1 - y0).max(1),
    }
    .clamp_to(width, height)
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let ctx = window::userdata::<WindowCtx>(hwnd, msg, lparam);
        if msg == WM_CREATE {
            return LRESULT(0);
        }
        if ctx.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let ctx = &*ctx;
        let shared = &*ctx.shared;
        match msg {
            WM_LBUTTONDOWN => {
                let (x, y) = window::mouse_pos(lparam);
                SetCapture(hwnd);
                shared.anchor.set(Some((ctx.index, x, y)));
                shared.current.set(Some((ctx.index, Rect { x, y, w: 1, h: 1 })));
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some((i, ax, ay)) = shared.anchor.get() {
                    if i == ctx.index {
                        let (x, y) = window::mouse_pos(lparam);
                        shared
                            .current
                            .set(Some((i, normalised(ax, ay, x, y, ctx.width, ctx.height))));
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if let Some((i, ax, ay)) = shared.anchor.get() {
                    let _ = ReleaseCapture();
                    shared.anchor.set(None);
                    if i == ctx.index {
                        let (x, y) = window::mouse_pos(lparam);
                        let rect = normalised(ax, ay, x, y, ctx.width, ctx.height);
                        if rect.w >= MIN_SIZE && rect.h >= MIN_SIZE {
                            shared.outcome.set(Some(Outcome::Done { monitor_index: i, rect }));
                        } else {
                            shared.current.set(None);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                shared.outcome.set(Some(Outcome::Cancelled));
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                shared.outcome.set(Some(Outcome::Cancelled));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
