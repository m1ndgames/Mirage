#![windows_subsystem = "windows"]

mod args;
mod autostart;
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
mod transform;
mod ui;
mod window;

use std::sync::mpsc::channel;
use std::sync::Arc;

use anyhow::anyhow;
use windows::core::HSTRING;
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

use crate::config::Config;
use crate::engine::{Command, Event};

fn main() {
    // No console of our own (windows subsystem); borrow the parent's for CLI output.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    if let Err(e) = run() {
        eprintln!("{e:#}");
        unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(format!("{e:#}")),
                &HSTRING::from("Mirage"),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
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
    let config = loaded.config;
    let notice = loaded.notice;

    let start_hidden = args.minimized || config.start_minimized;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([640.0, 560.0])
            .with_title("Mirage"),
        ..Default::default()
    };
    eframe::run_native(
        "Mirage",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let (event_tx, event_rx) = channel::<Event>();
            let engine = engine::spawn(event_tx, Arc::new(move || ctx.request_repaint()))?;
            engine.send(Command::Apply(config.clone()));
            Ok(Box::new(ui::App::new(
                config,
                path,
                engine,
                event_rx,
                notice,
                start_hidden,
            )))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}
