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

    let attached = identity::attached()?;
    println!("idx  id                                   name             gdi           size       primary");
    for (i, m) in attached.iter().enumerate() {
        println!(
            "{:<4} {:<36} {:<16} {:<13} {:>5}x{:<5} {}{}",
            i,
            m.id,
            m.friendly_name,
            m.gdi_name,
            m.info.width,
            m.info.height,
            if m.info.is_primary { "primary" } else { "" },
            if m.port_bound { " (port-bound)" } else { "" }
        );
    }
    if args.list {
        return Ok(());
    }
    let monitors: Vec<monitors::MonitorInfo> = attached.iter().map(|m| m.info.clone()).collect();

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

    let output = window::create_output_window(tgt.x, tgt.y, tgt.width, tgt.height)?;
    let pipeline = renderer::Pipeline::new(&d3d)?;
    let target = renderer::SwapChainTarget::new(&d3d, output.hwnd, output.width as u32, output.height as u32)?;
    let mut capture = capture::MonitorCapture::new(d3d.winrt_device()?, src.handle)?;
    let mut source: Option<renderer::SourceTexture> = None;
    println!("capturing – Ctrl+C to quit");

    let mut stats = Stats::new();
    loop {
        let _ = unsafe {
            MsgWaitForMultipleObjectsEx(Some(&[capture.frame_event()]), 1000, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if !window::pump_messages() {
            break;
        }
        let mut newest = None;
        while let Some(frame) = capture.try_next_frame()? {
            newest = Some(frame);
            stats.captured += 1;
        }
        if let Some(frame) = newest {
            let (fw, fh) = (frame.width as u32, frame.height as u32);
            if source.as_ref().map_or(true, |s| s.width != fw || s.height != fh) {
                source = Some(renderer::SourceTexture::new(&d3d.device, fw, fh)?);
            }
            let src_tex = source.as_ref().expect("just created");
            src_tex.copy_from(&d3d.context, &frame.texture);
            let (w, h) = capture.size();
            target.wait_for_frame_slot();
            target.clear(&d3d.context);
            let dst = Rect { x: 0, y: 0, w: output.width, h: output.height };
            pipeline.draw_mirror(&d3d.context, src_tex, rect.clamp_to(w, h).to_uv(w, h), dst);
            target.present()?;
            stats.presented += 1;
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
