mod args;
mod d3d;
mod geometry;
mod monitors;
mod renderer;
mod window;

use anyhow::anyhow;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

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
    let renderer = renderer::Renderer::new(&d3d, output.hwnd, output.width as u32, output.height as u32)?;
    println!("output window up – Ctrl+C to quit");

    while window::pump_messages() {
        renderer.wait_for_frame_slot();
        renderer.draw([0.0, 0.0, 1.0, 1.0])?;
        renderer.present()?;
    }
    Ok(())
}
