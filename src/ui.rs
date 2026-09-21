use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;

use crate::autostart;
use crate::config::{self, Config, Mapping, MonitorRef, Resolved};
use crate::transform::{Filter, Fit};
use crate::engine::{Command, EngineHandle, Event, RegionSelected};
use crate::identity::AttachedMonitor;

pub struct App {
    config: Config,
    path: PathBuf,
    engine: Option<EngineHandle>,
    events: Receiver<Event>,
    attached: Vec<AttachedMonitor>,
    selected: Option<usize>,
    error: Option<String>,
    notice: Option<String>,
    dirty: bool,
    /// Text of the profile name box (new / rename).
    profile_name: String,
    /// Hotkey text being edited; applied when the box loses focus.
    hotkey_text: String,
    /// Mirrors the registry Run entry; read once at start.
    autostart: bool,
    /// Set by the tray's Quit: the next close request really closes.
    quitting: bool,
    /// Pending viewport requests raised from `logic()` (which may run hidden).
    show_requested: bool,
    /// `--minimized` / start_minimized: hide the window on the first pass.
    hide_requested: bool,
}

impl App {
    pub fn new(
        config: Config,
        path: PathBuf,
        engine: EngineHandle,
        events: Receiver<Event>,
        notice: Option<String>,
        start_hidden: bool,
    ) -> Self {
        let profile_name = config.active_profile.clone();
        let hotkey_text = config.select_region_hotkey.clone();
        App {
            config,
            path,
            engine: Some(engine),
            events,
            attached: Vec::new(),
            selected: None,
            error: None,
            notice,
            dirty: false,
            profile_name,
            hotkey_text,
            autostart: autostart::is_enabled(),
            quitting: false,
            show_requested: false,
            hide_requested: start_hidden,
        }
    }

    fn send(&self, command: Command) {
        if let Some(e) = &self.engine {
            e.send(command);
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::MonitorsChanged(list) => {
                    if self.config.note_attached(&list) {
                        self.dirty = true;
                    }
                    self.attached = list;
                }
                Event::RegionSelected(r) => self.apply_selection(r),
                Event::SelectionCancelled => {}
                Event::ShowSettings => self.show_requested = true,
                Event::Quit => self.quitting = true,
                Event::Error(e) => self.error = Some(e),
            }
        }
    }

    /// The selection goes to the highlighted mapping, or a new one.
    fn apply_selection(&mut self, r: RegionSelected) {
        let default_target = self.default_target(&r.monitor);
        let profile = self.config.active_profile_mut();
        let index = match self.selected {
            Some(i) if i < profile.mappings.len() => i,
            _ => {
                profile.mappings.push(Mapping::default());
                profile.mappings.len() - 1
            }
        };
        let target = {
            let m = &mut profile.mappings[index];
            m.source = if r.is_primary { MonitorRef::Primary } else { MonitorRef::Id(r.monitor) };
            m.source_rect = [r.rect.x, r.rect.y, r.rect.w, r.rect.h];
            m.source_size = [r.monitor_size.0, r.monitor_size.1];
            if m.target.is_empty() {
                m.target = default_target;
            }
            m.target.clone()
        };
        self.config.last_target = Some(target);
        self.selected = Some(index);
        self.dirty = true;
    }

    /// Last used target if attached, else the smallest attached monitor that
    /// is not `source`, else empty (the row shows "no target").
    fn default_target(&self, source: &str) -> String {
        if let Some(t) = &self.config.last_target {
            if self.attached.iter().any(|m| &m.id == t) {
                return t.clone();
            }
        }
        self.attached
            .iter()
            .filter(|m| m.id != source)
            .min_by_key(|m| m.info.width as i64 * m.info.height as i64)
            .map(|m| m.id.clone())
            .unwrap_or_default()
    }

    fn label_for(&self, id: &str) -> String {
        self.config.known(id).map(|k| k.label.clone()).unwrap_or_else(|| id.to_string())
    }

    fn monitors_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Monitors");
        egui::Grid::new("monitors").num_columns(4).striped(true).show(ui, |ui| {
            ui.strong("Label");
            ui.strong("Name");
            ui.strong("Size");
            ui.strong("Status");
            ui.end_row();
            let attached = &self.attached;
            for m in &mut self.config.monitors {
                let present = attached.iter().find(|a| a.id == m.id);
                if ui.text_edit_singleline(&mut m.label).changed() {
                    self.dirty = true;
                }
                ui.label(&m.last_name);
                ui.label(format!("{}x{}", m.last_size[0], m.last_size[1]));
                match present {
                    Some(a) if a.port_bound => ui.colored_label(egui::Color32::YELLOW, "attached ⚠ port-bound"),
                    Some(a) if a.info.is_primary => ui.label("attached, primary"),
                    Some(_) => ui.label("attached"),
                    None => ui.weak("absent"),
                };
                ui.end_row();
            }
        });
    }

    fn general_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("General");
        ui.horizontal(|ui| {
            ui.label("Select-region hotkey");
            let response = ui.add(egui::TextEdit::singleline(&mut self.hotkey_text).desired_width(120.0));
            let commit = response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if commit && self.hotkey_text.trim() != self.config.select_region_hotkey {
                match crate::hotkey::parse(&self.hotkey_text) {
                    Some(_) => {
                        self.config.select_region_hotkey = self.hotkey_text.trim().to_string();
                        self.dirty = true;
                    }
                    None => {
                        self.error = Some(format!("'{}' is not a valid hotkey – e.g. Ctrl+Shift+R or F9", self.hotkey_text));
                        self.hotkey_text = self.config.select_region_hotkey.clone();
                    }
                }
            }
            ui.add_space(12.0);
            if ui.checkbox(&mut self.config.start_minimized, "Start minimized to tray").changed() {
                self.dirty = true;
            }
            let mut autostart = self.autostart;
            if ui.checkbox(&mut autostart, "Start with Windows").changed() {
                match autostart::set(autostart) {
                    Ok(()) => self.autostart = autostart,
                    Err(e) => self.error = Some(format!("autostart: {e:#}")),
                }
            }
        });
        ui.weak("Closing the window keeps Mirage running in the tray; use the tray menu to quit.");
    }

    fn profiles_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Profile");
        let names: Vec<String> = self.config.profiles.iter().map(|p| p.name.clone()).collect();
        let mut active = self.config.active_profile.clone();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("profile").selected_text(active.clone()).show_ui(ui, |ui| {
                for n in &names {
                    ui.selectable_value(&mut active, n.clone(), n);
                }
            });
            if active != self.config.active_profile {
                self.config.active_profile = active.clone();
                self.profile_name = active;
                self.selected = None;
                self.dirty = true;
            }
            ui.add_space(12.0);
            ui.add(egui::TextEdit::singleline(&mut self.profile_name).desired_width(140.0));
            if ui.button("New").clicked() {
                let name = self.config.add_profile(&self.profile_name);
                self.profile_name = name;
                self.selected = None;
                self.dirty = true;
            }
            if ui.button("Rename").clicked() {
                match self.config.rename_active_profile(&self.profile_name) {
                    Ok(()) => self.dirty = true,
                    Err(e) => self.error = Some(e),
                }
            }
            if ui.button("Delete").clicked() {
                match self.config.remove_active_profile() {
                    Ok(()) => {
                        self.profile_name = self.config.active_profile.clone();
                        self.selected = None;
                        self.dirty = true;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        });
    }

    /// Target-area control: presets for tiling plus a custom entry in percent.
    fn area_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let m = &mut self.config.active_profile_mut().mappings[i];
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.label("area");
            let current = AREA_PRESETS.iter().find(|(_, a)| area_eq(a, &m.target_area)).map(|(n, _)| *n).unwrap_or("custom");
            let mut choice = current;
            egui::ComboBox::from_id_salt(("area", i)).selected_text(current).show_ui(ui, |ui| {
                for (name, _) in AREA_PRESETS {
                    ui.selectable_value(&mut choice, name, *name);
                }
                ui.selectable_value(&mut choice, "custom", "custom");
            });
            if choice != current {
                m.target_area = match AREA_PRESETS.iter().find(|(n, _)| *n == choice) {
                    Some((_, a)) => a.to_vec(),
                    None => vec![0.0, 0.0, 1.0, 1.0],
                };
                changed = true;
            }
            if choice == "custom" {
                if m.target_area.len() != 4 {
                    m.target_area = vec![0.0, 0.0, 1.0, 1.0];
                    changed = true;
                }
                for (k, label) in ["x", "y", "w", "h"].iter().enumerate() {
                    ui.label(*label);
                    let mut pct = m.target_area[k] * 100.0;
                    if ui.add(egui::DragValue::new(&mut pct).range(0.0..=100.0).suffix("%").speed(1.0)).changed() {
                        m.target_area[k] = (pct / 100.0).clamp(0.0, 1.0);
                        changed = true;
                    }
                }
            }
        });
        if changed {
            self.dirty = true;
        }
    }

    fn mappings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Mappings");
        let statuses = config::resolve(self.config.active_profile(), &self.attached);
        let known: Vec<(String, String)> = self.config.monitors.iter().map(|m| (m.id.clone(), m.label.clone())).collect();
        let attached_ids: Vec<String> = self.attached.iter().map(|m| m.id.clone()).collect();
        let mut remove = None;
        let mut select_for = None;
        let count = self.config.active_profile().mappings.len();
        for i in 0..count {
            let status = match &statuses[i] {
                Resolved::Active(_) => "active".to_string(),
                Resolved::Dormant { reason, .. } => format!("dormant: {reason}"),
            };
            let (source_text, rect, follows_primary, target) = {
                let m = &self.config.active_profile().mappings[i];
                let source_text = match &m.source {
                    MonitorRef::Primary => "primary".to_string(),
                    MonitorRef::Id(id) => self.label_for(id),
                };
                (source_text, m.source_rect, matches!(m.source, MonitorRef::Primary), m.target.clone())
            };
            let highlighted = self.selected == Some(i);
            ui.horizontal(|ui| {
                if ui.selectable_label(highlighted, format!("#{}", i + 1)).clicked() {
                    self.selected = Some(i);
                }
                ui.label(format!("{source_text}  [{}, {}, {}×{}]", rect[0], rect[1], rect[2], rect[3]));
                ui.label("→");
                let target_label = known
                    .iter()
                    .find(|(id, _)| *id == target)
                    .map(|(_, l)| l.clone())
                    .unwrap_or_else(|| "no target".into());
                let mut new_target = target.clone();
                egui::ComboBox::from_id_salt(("target", i)).selected_text(target_label).show_ui(ui, |ui| {
                    for (id, label) in &known {
                        let text = if attached_ids.contains(id) { label.clone() } else { format!("{label} (absent)") };
                        ui.selectable_value(&mut new_target, id.clone(), text);
                    }
                });
                if new_target != target {
                    self.config.active_profile_mut().mappings[i].target = new_target.clone();
                    self.config.last_target = Some(new_target);
                    self.dirty = true;
                }
                let mut follow = follows_primary;
                if ui.checkbox(&mut follow, "follow primary").changed() {
                    let source = if follow {
                        MonitorRef::Primary
                    } else {
                        // Bind to whatever is primary right now.
                        match self.attached.iter().find(|a| a.info.is_primary) {
                            Some(a) => MonitorRef::Id(a.id.clone()),
                            None => MonitorRef::Primary,
                        }
                    };
                    self.config.active_profile_mut().mappings[i].source = source;
                    self.dirty = true;
                }
                if status == "active" {
                    ui.label(&status)
                } else {
                    ui.weak(&status)
                };
                if ui.button("Select region…").clicked() {
                    select_for = Some(i);
                }
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
            self.transform_row(ui, i);
            self.area_row(ui, i);
            ui.add_space(4.0);
        }
        if let Some(i) = remove {
            self.config.active_profile_mut().mappings.remove(i);
            self.selected = None;
            self.dirty = true;
        }
        if let Some(i) = select_for {
            self.selected = Some(i);
            self.send(Command::SelectRegion);
        }
        ui.add_space(6.0);
        if ui.button("Add mapping").clicked() {
            self.selected = None;
            self.send(Command::SelectRegion);
        }
        ui.weak(format!(
            "{} selects a region for the highlighted mapping (or creates one).",
            self.config.select_region_hotkey
        ));
    }
}

impl App {
    /// Second line of a mapping row: flips, rotation, fit mode, filter.
    fn transform_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let m = &mut self.config.active_profile_mut().mappings[i];
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            changed |= ui.checkbox(&mut m.flip_h, "flip H").changed();
            changed |= ui.checkbox(&mut m.flip_v, "flip V").changed();
            ui.label("rotate");
            egui::ComboBox::from_id_salt(("rotation", i)).selected_text(format!("{}°", m.rotation)).show_ui(ui, |ui| {
                for r in [0u16, 90, 180, 270] {
                    changed |= ui.selectable_value(&mut m.rotation, r, format!("{r}°")).changed();
                }
            });
            ui.label("fit");
            egui::ComboBox::from_id_salt(("fit", i)).selected_text(fit_name(m.fit)).show_ui(ui, |ui| {
                for f in [Fit::Fit, Fit::Fill, Fit::Stretch] {
                    changed |= ui.selectable_value(&mut m.fit, f, fit_name(f)).changed();
                }
            });
            ui.label("filter");
            egui::ComboBox::from_id_salt(("filter", i)).selected_text(filter_name(m.filter)).show_ui(ui, |ui| {
                for f in [Filter::Linear, Filter::Nearest] {
                    changed |= ui.selectable_value(&mut m.filter, f, filter_name(f)).changed();
                }
            });
        });
        if changed {
            self.dirty = true;
        }
    }
}

/// Tiling presets as fractions of the target monitor: name → [x, y, w, h].
const AREA_PRESETS: &[(&str, &[f32])] = &[
    ("whole", &[]),
    ("top half", &[0.0, 0.0, 1.0, 0.5]),
    ("bottom half", &[0.0, 0.5, 1.0, 0.5]),
    ("left half", &[0.0, 0.0, 0.5, 1.0]),
    ("right half", &[0.5, 0.0, 0.5, 1.0]),
    ("top-left quarter", &[0.0, 0.0, 0.5, 0.5]),
    ("top-right quarter", &[0.5, 0.0, 0.5, 0.5]),
    ("bottom-left quarter", &[0.0, 0.5, 0.5, 0.5]),
    ("bottom-right quarter", &[0.5, 0.5, 0.5, 0.5]),
];

/// Preset match. "whole" is only the empty list; an explicit 0/0/100/100 is a
/// custom area the user may still be editing.
fn area_eq(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

fn fit_name(fit: Fit) -> &'static str {
    match fit {
        Fit::Fit => "fit (letterbox)",
        Fit::Fill => "fill (crop)",
        Fit::Stretch => "stretch",
    }
}

fn filter_name(filter: Filter) -> &'static str {
    match filter {
        Filter::Linear => "linear",
        Filter::Nearest => "nearest",
    }
}

impl eframe::App for App {
    /// Runs before every frame *and* while the window is hidden (whenever the
    /// engine requests a repaint), so events and saves never wait for the UI.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.hide_requested {
            // eframe 0.36 ignores ViewportBuilder::with_visible(false); hide here instead.
            self.hide_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.drain_events();
        if self.show_requested {
            self.show_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if self.quitting {
            // A hidden eframe window ignores ViewportCommand::Close, so end the
            // process ourselves once the render thread has torn everything down.
            if let Some(engine) = self.engine.take() {
                engine.shutdown();
            }
            std::process::exit(0);
        }
        if self.dirty {
            self.dirty = false;
            if let Err(e) = self.config.save(&self.path) {
                self.error = Some(format!("saving config failed: {e:#}"));
            }
            self.send(Command::Apply(self.config.clone()));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Close button → hide to tray. Only the tray's Quit really closes.
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.quitting {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        egui::Panel::top("bar").show(ui, |ui| {
            if let Some(e) = self.error.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                    if ui.small_button("dismiss").clicked() {
                        self.error = None;
                    }
                });
            }
            if let Some(n) = self.notice.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::YELLOW, n);
                    if ui.small_button("ok").clicked() {
                        self.notice = None;
                    }
                });
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.general_panel(ui);
                ui.separator();
                self.profiles_panel(ui);
                ui.separator();
                self.monitors_panel(ui);
                ui.separator();
                self.mappings_panel(ui);
            });
        });
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
    }
}
