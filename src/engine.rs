use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};

use crate::capture::MonitorCapture;
use crate::config::{resolve, ActiveMapping, Config, Resolved};
use crate::d3d::{self, D3d};
use crate::geometry::Rect;
use crate::hotkey;
use crate::identity::{self, AttachedMonitor, MonitorId};
use crate::overlay::{Outcome, Selection};
use crate::renderer::{Pipeline, SourceTexture, SwapChainTarget};
use crate::transform;
use crate::window::{self, OutputWindow, TrayAction, WindowSignals};

pub enum Command {
    Apply(Config),
    SelectRegion,
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct RegionSelected {
    pub monitor: MonitorId,
    pub is_primary: bool,
    pub rect: Rect,
    pub monitor_size: (i32, i32),
}

#[derive(Debug, Clone)]
pub enum Event {
    MonitorsChanged(Vec<AttachedMonitor>),
    RegionSelected(RegionSelected),
    SelectionCancelled,
    /// The user clicked the tray icon or picked "Settings" in its menu.
    ShowSettings,
    /// The user picked "Quit" in the tray menu.
    Quit,
    Error(String),
}

/// Handle owned by the UI thread.
pub struct EngineHandle {
    tx: Sender<Command>,
    wake: isize,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
        unsafe {
            let _ = SetEvent(HANDLE(self.wake as *mut _));
        }
    }

    pub fn shutdown(mut self) {
        self.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.send(Command::Shutdown);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
        unsafe {
            let _ = CloseHandle(HANDLE(self.wake as *mut _));
        }
    }
}

/// Starts the render thread. `repaint` is called after every event so the UI
/// wakes up (eframe's `Context::request_repaint`).
pub fn spawn(events: Sender<Event>, repaint: Arc<dyn Fn() + Send + Sync>) -> anyhow::Result<EngineHandle> {
    let (tx, rx) = std::sync::mpsc::channel();
    let wake = unsafe { CreateEventW(None, false, false, None) }?.0 as isize;
    let thread = std::thread::Builder::new()
        .name("mirage-render".into())
        .spawn(move || {
            if let Err(e) = run(rx, wake, events.clone(), repaint.clone()) {
                let _ = events.send(Event::Error(format!("render thread stopped: {e:#}")));
                repaint();
            }
        })?;
    Ok(EngineHandle {
        tx,
        wake,
        thread: Some(thread),
    })
}

struct Source {
    capture: MonitorCapture,
    texture: Option<SourceTexture>,
    dirty: bool,
}

struct Output {
    window: OutputWindow,
    target: SwapChainTarget,
    mappings: Vec<ActiveMapping>,
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.window.hwnd);
        }
    }
}

struct Engine {
    d3d: D3d,
    pipeline: Pipeline,
    signals: Box<WindowSignals>,
    message_hwnd: HWND,
    events: Sender<Event>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    config: Config,
    attached: Vec<AttachedMonitor>,
    active: Vec<ActiveMapping>,
    sources: HashMap<isize, Source>,
    outputs: HashMap<isize, Output>,
    built_once: bool,
    /// The hotkey text currently registered, so Apply only re-registers on change.
    registered_hotkey: Option<String>,
    selection: Option<Selection>,
}

const HOTKEY_ID: i32 = 1;

fn run(
    rx: Receiver<Command>,
    wake: isize,
    events: Sender<Event>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) -> anyhow::Result<()> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }?;
    let signals = Box::new(WindowSignals::default());
    let message_hwnd = window::create_message_window(&*signals as *const WindowSignals)?;
    if let Err(e) = window::add_tray_icon(message_hwnd) {
        let _ = events.send(Event::Error(format!("tray icon unavailable: {e:#}")));
    }
    // The device is created for whatever is primary now; a primary on another
    // GPU would need a rebuild of the device – not supported (PLAN.md: one GPU).
    let primary = identity::attached()?
        .into_iter()
        .find(|m| m.info.is_primary)
        .context("no primary monitor")?;
    let d3d = d3d::create_for_monitor(primary.info.handle)?;
    println!("rendering on {}", d3d.adapter_name);
    let pipeline = Pipeline::new(&d3d)?;
    let mut engine = Engine {
        d3d,
        pipeline,
        signals,
        message_hwnd,
        events,
        repaint,
        config: Config::default(),
        attached: Vec::new(),
        active: Vec::new(),
        sources: HashMap::new(),
        outputs: HashMap::new(),
        built_once: false,
        registered_hotkey: None,
        selection: None,
    };
    engine.rebuild();

    loop {
        let mut handles = vec![HANDLE(wake as *mut _)];
        handles.extend(engine.sources.values().map(|s| s.capture.frame_event()));
        let _ = unsafe { MsgWaitForMultipleObjectsEx(Some(&handles), 1000, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        if !window::pump_messages() {
            break;
        }
        loop {
            match rx.try_recv() {
                Ok(Command::Apply(config)) => {
                    engine.config = config;
                    engine.register_hotkey();
                    engine.rebuild();
                }
                Ok(Command::SelectRegion) => engine.start_selection(),
                Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                    engine.outputs.clear();
                    window::remove_tray_icon(engine.message_hwnd);
                    unsafe {
                        let _ = DestroyWindow(engine.message_hwnd);
                    }
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if engine.signals.topology_changed.take() {
            engine.rebuild();
        }
        if engine.signals.hotkey.take() {
            engine.start_selection();
        }
        match engine.signals.tray.take() {
            Some(TrayAction::ShowSettings) => engine.emit(Event::ShowSettings),
            Some(TrayAction::SelectRegion) => engine.start_selection(),
            Some(TrayAction::Quit) => engine.emit(Event::Quit),
            None => {}
        }
        engine.pump_frames();
        engine.present_dirty();
        engine.pump_selection();
    }
    Ok(())
}

impl Engine {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        (self.repaint)();
    }

    /// (Re)registers the global selection hotkey from the config. A failure
    /// (unparsable, or taken by another program) is reported, not fatal.
    fn register_hotkey(&mut self) {
        let wanted = self.config.select_region_hotkey.trim().to_string();
        if self.registered_hotkey.as_deref() == Some(wanted.as_str()) {
            return;
        }
        if self.registered_hotkey.take().is_some() {
            unsafe {
                let _ = UnregisterHotKey(Some(self.message_hwnd), HOTKEY_ID);
            }
        }
        let Some(hk) = hotkey::parse(&wanted) else {
            self.emit(Event::Error(format!(
                "hotkey '{wanted}' is not valid – use e.g. Ctrl+Shift+R or F9"
            )));
            return;
        };
        let modifiers = HOT_KEY_MODIFIERS(hk.modifiers) | MOD_NOREPEAT;
        match unsafe { RegisterHotKey(Some(self.message_hwnd), HOTKEY_ID, modifiers, hk.vk) } {
            Ok(()) => self.registered_hotkey = Some(wanted),
            Err(e) => self.emit(Event::Error(format!(
                "hotkey '{wanted}' could not be registered ({e}) – another program uses it; change select_region_hotkey in config.toml"
            ))),
        }
    }

    /// Re-enumerates monitors, resolves the active profile and recreates
    /// sources/outputs only when the resolved set actually changed.
    fn rebuild(&mut self) {
        let attached = match identity::attached() {
            Ok(a) => a,
            Err(e) => {
                self.emit(Event::Error(format!("monitor enumeration failed: {e:#}")));
                return;
            }
        };
        let attached_changed = attached != self.attached;
        self.attached = attached;
        self.emit(Event::MonitorsChanged(self.attached.clone()));
        if attached_changed && self.selection.is_some() {
            // The overlays no longer match the desktop; the user starts over.
            self.selection = None;
            self.emit(Event::SelectionCancelled);
        }

        let active: Vec<ActiveMapping> = resolve(self.config.active_profile(), &self.attached)
            .into_iter()
            .filter_map(|r| match r {
                Resolved::Active(a) => Some(a),
                Resolved::Dormant { .. } => None,
            })
            .collect();
        if self.built_once && !attached_changed && active == self.active {
            return;
        }
        self.built_once = true;
        self.active = active;
        self.outputs.clear();
        self.sources.clear();

        let source_handles: HashSet<isize> = self.active.iter().map(|m| m.source_handle).collect();
        for handle in source_handles {
            match self.d3d.winrt_device().and_then(|dev| MonitorCapture::new(dev, handle)) {
                Ok(capture) => {
                    self.sources.insert(
                        handle,
                        Source {
                            capture,
                            texture: None,
                            dirty: false,
                        },
                    );
                }
                Err(e) => self.emit(Event::Error(format!("capture of a source monitor failed: {e:#}"))),
            }
        }

        let target_handles: HashSet<isize> = self.active.iter().map(|m| m.target_handle).collect();
        for handle in target_handles {
            let Some(monitor) = self.attached.iter().find(|m| m.info.handle == handle) else {
                continue;
            };
            let info = &monitor.info;
            let result = window::create_output_window(info.x, info.y, info.width, info.height)
                .and_then(|w| SwapChainTarget::new(&self.d3d, w.hwnd, w.width as u32, w.height as u32).map(|t| (w, t)));
            match result {
                Ok((window, target)) => {
                    let mappings = self
                        .active
                        .iter()
                        .filter(|m| m.target_handle == handle)
                        .cloned()
                        .collect();
                    self.outputs.insert(
                        handle,
                        Output {
                            window,
                            target,
                            mappings,
                        },
                    );
                }
                Err(e) => self.emit(Event::Error(format!("output window failed: {e:#}"))),
            }
        }
        // First paint even before a frame arrives, so the target goes black at once.
        for out in self.outputs.values() {
            out.target.clear(&self.d3d.context);
            let _ = out.target.present();
        }
    }

    /// Copies the newest frame of every source that delivered one.
    fn pump_frames(&mut self) {
        let ctx = &self.d3d.context;
        let device = &self.d3d.device;
        for (handle, source) in self.sources.iter_mut() {
            let mut newest = None;
            loop {
                match source.capture.try_next_frame() {
                    Ok(Some(frame)) => newest = Some(frame),
                    Ok(None) => break,
                    Err(e) => {
                        let _ = self
                            .events
                            .send(Event::Error(format!("capture error on monitor {handle}: {e:#}")));
                        break;
                    }
                }
            }
            if let Some(frame) = newest {
                let (fw, fh) = (frame.width as u32, frame.height as u32);
                if source.texture.as_ref().is_none_or(|t| t.width != fw || t.height != fh) {
                    match SourceTexture::new(device, fw, fh) {
                        Ok(t) => source.texture = Some(t),
                        Err(e) => {
                            let _ = self
                                .events
                                .send(Event::Error(format!("texture creation failed: {e:#}")));
                            continue;
                        }
                    }
                }
                if let Some(t) = &source.texture {
                    t.copy_from(ctx, &frame.texture);
                    source.dirty = true;
                }
            }
        }
    }

    /// Redraws every output that shows at least one dirty source.
    fn present_dirty(&mut self) {
        let ctx = &self.d3d.context;
        for out in self.outputs.values() {
            let needs = out
                .mappings
                .iter()
                .any(|m| self.sources.get(&m.source_handle).is_some_and(|s| s.dirty));
            if !needs {
                continue;
            }
            out.target.wait_for_frame_slot();
            out.target.clear(ctx);
            for m in &out.mappings {
                let Some(src) = self.sources.get(&m.source_handle).and_then(|s| s.texture.as_ref()) else {
                    continue;
                };
                let (w, h) = (src.width as i32, src.height as i32);
                let layout = transform::layout(&m.transform, m.source_rect.clamp_to(w, h), (w, h), m.target_rect);
                self.pipeline.draw_mirror(ctx, src, &layout, m.transform.filter);
            }
            if let Err(e) = out.target.present() {
                let _ = self.events.send(Event::Error(format!("present failed: {e:#}")));
            }
        }
        for s in self.sources.values_mut() {
            s.dirty = false;
        }
    }

    fn start_selection(&mut self) {
        if self.selection.is_some() {
            return;
        }
        match Selection::start(&self.d3d, &self.attached) {
            Ok(s) => self.selection = Some(s),
            Err(e) => self.emit(Event::Error(format!("could not start region selection: {e:#}"))),
        }
    }

    /// Repaints the overlays and finishes the selection once the user is done.
    fn pump_selection(&mut self) {
        let Some(sel) = self.selection.as_mut() else { return };
        sel.redraw(&self.d3d, &self.pipeline);
        let outcome = sel.poll();
        match outcome {
            None => {}
            Some(Outcome::Cancelled) => {
                self.selection = None;
                self.emit(Event::SelectionCancelled);
            }
            Some(Outcome::Done { monitor_index, rect }) => {
                self.selection = None;
                if let Some(m) = self.attached.get(monitor_index) {
                    self.emit(Event::RegionSelected(RegionSelected {
                        monitor: m.id.clone(),
                        is_primary: m.info.is_primary,
                        rect,
                        monitor_size: (m.info.width, m.info.height),
                    }));
                }
            }
        }
    }
}
