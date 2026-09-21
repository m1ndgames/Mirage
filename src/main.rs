mod args;
mod capture;
mod config;
mod d3d;
mod engine;
mod geometry;
mod hotkey;
mod identity;
mod monitors;
mod renderer;
mod window;

use std::sync::mpsc::channel;
use std::sync::Arc;

use anyhow::anyhow;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

use crate::config::Config;
use crate::engine::{Command, Event};

fn main() -> anyhow::Result<()> {
    let args = args::Args::parse(std::env::args().skip(1)).map_err(|e| anyhow!("{e}\n\n{}", args::USAGE))?;

    // Must happen before any monitor enumeration or window creation.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }?;

    if args.list {
        for (i, m) in identity::attached()?.iter().enumerate() {
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
        return Ok(());
    }

    let path = Config::default_path();
    let loaded = Config::load(&path);
    if let Some(n) = loaded.notice {
        println!("notice: {n}");
    }
    println!("config: {}", path.display());

    let (event_tx, event_rx) = channel::<Event>();
    let engine = engine::spawn(event_tx, Arc::new(|| {}))?;
    engine.send(Command::Apply(loaded.config));
    println!("engine running – Ctrl+C to quit");
    while let Ok(event) = event_rx.recv() {
        match event {
            Event::MonitorsChanged(list) => println!("monitors: {:?}", list.iter().map(|m| &m.id).collect::<Vec<_>>()),
            Event::Error(e) => println!("error: {e}"),
            other => println!("event: {other:?}"),
        }
    }
    Ok(())
}
