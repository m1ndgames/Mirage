mod args;
mod capture;
mod config;
mod d3d;
mod geometry;
mod identity;
mod monitors;
mod renderer;
mod window;

use anyhow::anyhow;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT};

use crate::geometry::Rect;

fn main() -> anyhow::Result<()> {
    let args = args::Args::parse(std::env::args().skip(1))
        .map_err(|e| anyhow!("{e}\n\n{}", args::USAGE))?;

    // Must happen before any monitor enumeration or window creation.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }?;
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }?;

    let monitors = monitors::enumerate()?;
    print!("{}", monitors::describe(&monitors));
    if args.list {
        return Ok(());
    }

    let source = monitors::choose_source(&monitors, args.source).map_err(|e| anyhow!(e))?;
    let target = monitors::choose_target(&monitors, args.target, source).map_err(|e| anyhow!(e))?;
    let (src, tgt) = (&monitors[source], &monitors[target]);
    let rect = args
        .rect
        .unwrap_or(Rect { x: 0, y: 0, w: src.width, h: src.height })
        .clamp_to(src.width, src.height);
    println!(
        "source {} ({}x{})  ->  target {} at {},{} ({}x{})  crop {:?}",
        src.device_name, src.width, src.height, tgt.device_name, tgt.x, tgt.y, tgt.width, tgt.height, rect
    );

    let d3d = d3d::create_for_monitor(src.handle)?;
    println!("rendering on: {}", d3d.adapter_name);

    let output = window::create(tgt.x, tgt.y, tgt.width, tgt.height)?;
    let mut renderer = renderer::Renderer::new(&d3d, output.hwnd, output.width as u32, output.height as u32)?;
    let mut capture = capture::MonitorCapture::new(d3d.winrt_device()?, src.handle)?;
    println!("capturing – Ctrl+C to quit");

    // Event driven: sleep until WGC signals a frame or a window message arrives,
    // then present exactly once per newest frame. Presenting is never paced by
    // Present() blocking – on this machine it doesn't (see PLAN.md, M0 results).
    let mut stats = Stats::new();
    loop {
        let wait = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&[capture.frame_event()]), 1000, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if !window::pump_messages() {
            break;
        }
        if wait == WAIT_OBJECT_0 {
            // Drain the pool so a slow target only ever shows the newest frame.
            let mut newest = None;
            while let Some(frame) = capture.try_next_frame()? {
                newest = Some(frame);
                stats.captured += 1;
            }
            if let Some(frame) = newest {
                renderer.wait_for_frame_slot();
                renderer.ensure_source(frame.width as u32, frame.height as u32)?;
                renderer.copy_frame(&frame.texture);
                let (w, h) = capture.size();
                renderer.draw(rect.clamp_to(w, h).to_uv(w, h))?;
                renderer.present()?;
                stats.presented += 1;
            }
        }
        stats.report_if_due();
    }
    Ok(())
}

/// Once-per-second throughput report. Capture rate follows the source monitor,
/// present rate follows the target (60 Hz on the DisplayLink screen).
struct Stats {
    started: std::time::Instant,
    captured: u32,
    presented: u32,
}

impl Stats {
    fn new() -> Self {
        Stats { started: std::time::Instant::now(), captured: 0, presented: 0 }
    }

    fn report_if_due(&mut self) {
        let elapsed = self.started.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            println!(
                "captured {:.0} fps / presented {:.0} fps",
                self.captured as f32 / elapsed,
                self.presented as f32 / elapsed
            );
            *self = Stats::new();
        }
    }
}
