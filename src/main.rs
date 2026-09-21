mod args;
mod capture;
mod config;
mod d3d;
mod engine;
mod geometry;
mod hotkey;
mod identity;
mod monitors;
mod overlay;
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
    let mut config = loaded.config;
    engine.send(Command::Apply(config.clone()));
    println!("engine running – {} selects a region, Ctrl+C quits", config.select_region_hotkey);
    while let Ok(event) = event_rx.recv() {
        match event {
            Event::MonitorsChanged(list) => {
                if config.note_attached(&list) {
                    config.save(&path)?;
                }
                println!("monitors: {:?}", list.iter().map(|m| &m.id).collect::<Vec<_>>());
            }
            Event::RegionSelected(r) => {
                println!("selected {:?} on {} (primary={})", r.rect, r.monitor, r.is_primary);
                let target = config.last_target.clone().unwrap_or_default();
                let profile = config.active_profile_mut();
                if profile.mappings.is_empty() {
                    profile.mappings.push(config::Mapping::default());
                }
                let m = &mut profile.mappings[0];
                m.source = if r.is_primary { config::MonitorRef::Primary } else { config::MonitorRef::Id(r.monitor) };
                m.source_rect = [r.rect.x, r.rect.y, r.rect.w, r.rect.h];
                m.source_size = [r.monitor_size.0, r.monitor_size.1];
                if m.target.is_empty() {
                    m.target = target;
                }
                config.save(&path)?;
                engine.send(Command::Apply(config.clone()));
            }
            Event::SelectionCancelled => println!("selection cancelled"),
            Event::Error(e) => println!("error: {e}"),
        }
    }
    Ok(())
}
