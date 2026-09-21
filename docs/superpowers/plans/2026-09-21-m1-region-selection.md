# Mirage M1 – Region Selection, Settings Window, Monitor Identity – Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the M0 spike into an application: hotkey → all monitors freeze → drag a rectangle → it is mirrored on the cockpit screen; the configuration persists and survives monitor hot-plugging.

**Architecture:** Two threads. The main thread runs an egui/eframe settings window and owns the `Config`. A render thread owns every Win32 window, the D3D11 device, WGC capture sessions and the selection overlay; it receives `Command`s (Apply / SelectRegion / Shutdown) and emits `Event`s (MonitorsChanged / RegionSelected / Error). Monitors are identified by EDID or USB serial, never by index or `\\.\DISPLAYn` name.

**Tech Stack:** Rust 1.98, `windows 0.62`, `eframe 0.36`, `serde 1`, `toml 1`, `anyhow 1`.

**Spec:** `docs/superpowers/specs/2026-09-21-m1-region-selection-design.md` (read it first; `PLAN.md` holds the project-wide principles and the M0 results this builds on).

## Global Constraints

- Display only: no hooks, no injection, no input generation, no process interaction (PLAN.md principle 1 & non-goals).
- GPU-only frame path; the only CPU-side pixel data is the zero-initialised fallback overlay texture.
- Output windows: `WS_POPUP` + `WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`. Overlay windows are the only ones that may take focus, and only during selection.
- The render loop never waits on `Present()` for pacing (M0 result); it sleeps on events.
- All coordinates are physical pixels. Process is Per-Monitor-DPI-v2 aware before anything enumerates monitors.
- Never key anything on `\\.\DISPLAYn` except as a join key within one enumeration.
- Absent monitors make mappings dormant; nothing about topology changes may panic.
- English for code, comments, UI strings and commit messages. Commits end with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- API signatures below follow `windows 0.62` (verified in the crate source for the DisplayConfig, CfgMgr32, Registry, hotkey and timer functions). If the compiler disagrees, https://microsoft.github.io/windows-docs-rs/doc/windows/ wins.
- Work on branch `m1-region-selection`, merge to `main` when the acceptance checklist passes.

---

## File structure

| File | Status | Responsibility |
|---|---|---|
| `Cargo.toml` | modify | add `eframe`, `serde`, `toml`; add `windows` features `Win32_Devices_Display`, `Win32_Devices_DeviceAndDriverInstallation`, `Win32_System_Registry`, `Win32_UI_Input_KeyboardAndMouse` |
| `src/config.rs` | new | `Config`, `Profile`, `Mapping`, `MonitorRef`, `KnownMonitor`, load/save, `resolve`, `note_attached` (pure, tested) |
| `src/identity.rs` | new | `AttachedMonitor`; `parse_edid`, `instance_id_from_device_path`, `choose_id` (pure, tested); `attached()` (Win32) |
| `src/renderer.rs` | rewrite | `Pipeline` (shaders, sampler, constant buffers), `SwapChainTarget`, `SourceTexture` |
| `src/shaders.hlsl` | modify | add `ps_overlay` |
| `src/capture.rs` | modify | add `grab_one_frame()` |
| `src/window.rs` | modify | `create_output_window`, `create_overlay_window`, `create_message_window`, `WindowSignals`, mouse helpers |
| `src/overlay.rs` | new | `Selection` – N overlay windows, drag state, outcome |
| `src/engine.rs` | new | render thread: commands/events, resolve, sources/outputs, topology rebuild, hotkey, selection |
| `src/ui.rs` | new | eframe `App` |
| `src/main.rs` | rewrite | `--list` diagnostic, startup order, eframe |
| `src/monitors.rs`, `src/geometry.rs`, `src/args.rs`, `src/d3d.rs` | keep | unchanged (args loses `--source/--target/--rect` at the end) |

---

### Task 1: Configuration data model (pure, TDD)

**Files:**
- Modify: `Cargo.toml`
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;` only)

**Interfaces:**
- Produces: `config::{MonitorRef, KnownMonitor, Mapping, Profile, Config, Loaded, ActiveMapping, Resolved}`, `Config::{default_path, load, save, active_profile, active_profile_mut, known, note_attached}`, `Mapping::effective_rect`, `config::resolve`, `config::default_label`.
- `config` depends on `identity::{AttachedMonitor, MonitorId}`, so this task creates a minimal `identity.rs` with just those; Task 2 fills in the rest.

- [ ] **Step 1: Add dependencies and features**

In `Cargo.toml` replace `[dependencies]` … up to `[profile.release]` with:

```toml
[dependencies]
anyhow = "1"
eframe = "0.36"
serde = { version = "1", features = ["derive"] }
toml = "1"

[dependencies.windows]
version = "0.62"
features = [
    "Foundation",
    "Graphics_Capture",
    "Graphics_DirectX",
    "Graphics_DirectX_Direct3D11",
    "Security_Authorization_AppCapabilityAccess",
    "Win32_Devices_DeviceAndDriverInstallation",
    "Win32_Devices_Display",
    "Win32_Foundation",
    "Win32_Graphics_Direct3D",
    "Win32_Graphics_Direct3D_Fxc",
    "Win32_Graphics_Direct3D11",
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common",
    "Win32_Graphics_Gdi",
    "Win32_Security",
    "Win32_System_LibraryLoader",
    "Win32_System_Registry",
    "Win32_System_Threading",
    "Win32_System_WinRT",
    "Win32_System_WinRT_Direct3D11",
    "Win32_System_WinRT_Graphics_Capture",
    "Win32_UI_HiDpi",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_UI_WindowsAndMessaging",
]
```

Run `cargo build` once so the new crates download and compile (eframe takes a few minutes the first time).

- [ ] **Step 2: Create the `AttachedMonitor` stub**

`src/identity.rs`:

```rust
use crate::monitors::MonitorInfo;

pub type MonitorId = String;

/// One monitor of the current desktop with its hardware identity attached.
/// Plain data so config logic is testable without Win32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedMonitor {
    pub id: MonitorId,
    /// `\\.\DISPLAYn` – only valid until the next topology change.
    pub gdi_name: String,
    pub friendly_name: String,
    pub info: MonitorInfo,
    /// True when the id is a `path:` fallback bound to the connector.
    pub port_bound: bool,
}
```

Add `mod identity;` and `mod config;` to `src/main.rs`.

- [ ] **Step 3: Write the failing config tests**

`src/config.rs`:

```rust
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::identity::{AttachedMonitor, MonitorId};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitors::MonitorInfo;

    fn attached(id: &str, name: &str, w: i32, h: i32, primary: bool, handle: isize) -> AttachedMonitor {
        AttachedMonitor {
            id: id.to_string(),
            gdi_name: format!("\\\\.\\DISPLAY{handle}"),
            friendly_name: name.to_string(),
            info: MonitorInfo {
                handle,
                device_name: format!("\\\\.\\DISPLAY{handle}"),
                x: 0,
                y: 0,
                width: w,
                height: h,
                is_primary: primary,
            },
            port_bound: false,
        }
    }

    fn desk() -> Vec<AttachedMonitor> {
        vec![
            attached("edid:HPN:3582:CNK04611FZ", "HP 27xq", 2560, 1440, true, 1),
            attached("edid:HPN:3582:CNK8360L1B", "HP 27xq", 2560, 1440, false, 2),
            attached("usb:WWIN29320251005160820", "USB_Monitor", 768, 1024, false, 3),
        ]
    }

    fn mapping(source: MonitorRef, target: &str) -> Mapping {
        Mapping {
            source,
            source_rect: [0, 1040, 400, 400],
            source_size: [2560, 1440],
            target: target.to_string(),
            target_rect: vec![],
        }
    }

    #[test]
    fn default_config_has_one_empty_profile() {
        let c = Config::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.active_profile, "default");
        assert_eq!(c.profiles.len(), 1);
        assert!(c.profiles[0].mappings.is_empty());
        assert_eq!(c.select_region_hotkey, "Ctrl+Alt+M");
    }

    #[test]
    fn toml_round_trip_preserves_everything() {
        let mut c = Config::default();
        c.last_target = Some("usb:WWIN29320251005160820".into());
        c.monitors.push(KnownMonitor {
            id: "edid:HPN:3582:CNK04611FZ".into(),
            label: "Main".into(),
            last_name: "HP 27xq".into(),
            last_size: [2560, 1440],
        });
        c.active_profile_mut().mappings.push(mapping(MonitorRef::Primary, "usb:WWIN29320251005160820"));
        let text = toml::to_string(&c).unwrap();
        assert!(text.contains("source = \"primary\""), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn unknown_fields_and_missing_fields_are_tolerated() {
        let text = "version = 1\nfuture_field = true\n[[profiles]]\nname = \"default\"\n[[profiles.mappings]]\nsource = \"edid:X:0001:S\"\n";
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.profiles[0].mappings[0].source, MonitorRef::Id("edid:X:0001:S".into()));
        assert_eq!(c.profiles[0].mappings[0].target, "");
    }

    #[test]
    fn load_missing_file_gives_default_without_notice() {
        let dir = std::env::temp_dir().join(format!("mirage-test-{}", std::process::id()));
        let path = dir.join("nope").join("config.toml");
        let loaded = Config::load(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notice.is_none());
    }

    #[test]
    fn save_then_load_round_trips_and_corrupt_file_is_renamed() {
        let dir = std::env::temp_dir().join(format!("mirage-test-{}-{}", std::process::id(), line!()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.last_target = Some("x".into());
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).config, c);

        std::fs::write(&path, "this is = not [valid").unwrap();
        let loaded = Config::load(&path);
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.notice.is_some());
        assert!(dir.join("config.toml.bad").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn effective_rect_scales_when_resolution_changed() {
        let m = mapping(MonitorRef::Primary, "t");
        assert_eq!(m.effective_rect((2560, 1440)), Rect { x: 0, y: 1040, w: 400, h: 400 });
        assert_eq!(m.effective_rect((1920, 1080)), Rect { x: 0, y: 780, w: 300, h: 300 });
    }

    #[test]
    fn effective_rect_without_recorded_size_is_just_clamped() {
        let mut m = mapping(MonitorRef::Primary, "t");
        m.source_size = [0, 0];
        m.source_rect = [2500, 0, 200, 200];
        assert_eq!(m.effective_rect((2560, 1440)), Rect { x: 2500, y: 0, w: 60, h: 200 });
    }

    #[test]
    fn resolve_active_dormant_and_primary() {
        let mut p = Profile::default();
        p.mappings.push(mapping(MonitorRef::Primary, "usb:WWIN29320251005160820"));
        p.mappings.push(mapping(MonitorRef::Id("edid:HPN:3582:CNK04611NG".into()), "usb:WWIN29320251005160820"));
        p.mappings.push(mapping(MonitorRef::Primary, "edid:GONE:0000:X"));
        let r = resolve(&p, &desk());
        match &r[0] {
            Resolved::Active(a) => {
                assert_eq!(a.source_handle, 1);
                assert_eq!(a.target_handle, 3);
                assert_eq!(a.target_rect, Rect { x: 0, y: 0, w: 768, h: 1024 });
                assert_eq!(a.source_rect, Rect { x: 0, y: 1040, w: 400, h: 400 });
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&r[1], Resolved::Dormant { reason, .. } if reason.contains("source")));
        assert!(matches!(&r[2], Resolved::Dormant { reason, .. } if reason.contains("target")));
    }

    #[test]
    fn resolve_uses_explicit_target_rect() {
        let mut p = Profile::default();
        let mut m = mapping(MonitorRef::Primary, "usb:WWIN29320251005160820");
        m.target_rect = vec![0, 0, 768, 512];
        p.mappings.push(m);
        match &resolve(&p, &desk())[0] {
            Resolved::Active(a) => assert_eq!(a.target_rect, Rect { x: 0, y: 0, w: 768, h: 512 }),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn note_attached_adds_and_updates_known_monitors() {
        let mut c = Config::default();
        assert!(c.note_attached(&desk()));
        assert_eq!(c.monitors.len(), 3);
        assert_eq!(c.monitors[0].label, "HP 27xq (1FZ)");
        assert_eq!(c.monitors[2].label, "USB_Monitor (820)");
        assert!(!c.note_attached(&desk()), "second call changes nothing");
        c.monitors[0].label = "Main".into();
        let mut moved = desk();
        moved[0].info.width = 1920;
        assert!(c.note_attached(&moved));
        assert_eq!(c.monitors[0].label, "Main", "labels are never overwritten");
        assert_eq!(c.monitors[0].last_size, [1920, 1440]);
    }

    #[test]
    fn default_label_uses_the_last_three_chars_of_the_serial() {
        assert_eq!(default_label("HP 27xq", "edid:HPN:3582:CNK04611FZ"), "HP 27xq (1FZ)");
        assert_eq!(default_label("USB_Monitor", "usb:WWIN29320251005160820"), "USB_Monitor (820)");
        assert_eq!(default_label("Generic", "path:DISPLAY\\X\\1&2"), "Generic");
    }
}
```

- [ ] **Step 4: Run to verify the tests fail**

Run: `cargo test config`
Expected: compile errors – types and functions not found.

- [ ] **Step 5: Implement the config module**

Insert above the tests in `src/config.rs`:

```rust
/// A monitor reference in a mapping: the current primary, or a concrete id.
/// Serialised as the string `"primary"` or the id itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum MonitorRef {
    Primary,
    Id(MonitorId),
}

impl From<String> for MonitorRef {
    fn from(s: String) -> Self {
        if s == "primary" {
            MonitorRef::Primary
        } else {
            MonitorRef::Id(s)
        }
    }
}

impl From<MonitorRef> for String {
    fn from(r: MonitorRef) -> Self {
        match r {
            MonitorRef::Primary => "primary".to_string(),
            MonitorRef::Id(id) => id,
        }
    }
}

/// Every monitor Mirage has ever seen, so absent ones can still be labelled and picked.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KnownMonitor {
    pub id: MonitorId,
    pub label: String,
    pub last_name: String,
    pub last_size: [i32; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mapping {
    pub source: MonitorRef,
    /// Physical pixels relative to the source monitor's top-left corner.
    pub source_rect: [i32; 4],
    /// Source monitor size when the rect was drawn; `[0, 0]` = unknown.
    pub source_size: [i32; 2],
    pub target: MonitorId,
    /// Empty = the whole target monitor. Otherwise `[x, y, w, h]` relative to it.
    pub target_rect: Vec<i32>,
}

impl Default for Mapping {
    fn default() -> Self {
        Mapping {
            source: MonitorRef::Primary,
            source_rect: [0, 0, 0, 0],
            source_size: [0, 0],
            target: String::new(),
            target_rect: Vec::new(),
        }
    }
}

impl Mapping {
    /// The source rect for the monitor's current size: scaled proportionally if
    /// the monitor changed resolution since the rect was drawn, then clamped.
    pub fn effective_rect(&self, current: (i32, i32)) -> Rect {
        let [x, y, w, h] = self.source_rect;
        let [rw, rh] = self.source_size;
        let rect = if rw > 0 && rh > 0 && (rw, rh) != current {
            let sx = current.0 as f64 / rw as f64;
            let sy = current.1 as f64 / rh as f64;
            Rect {
                x: (x as f64 * sx).round() as i32,
                y: (y as f64 * sy).round() as i32,
                w: (w as f64 * sx).round() as i32,
                h: (h as f64 * sy).round() as i32,
            }
        } else {
            Rect { x, y, w, h }
        };
        rect.clamp_to(current.0, current.1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// M4: executable name that auto-activates this profile. Empty = never.
    pub process: String,
    pub mappings: Vec<Mapping>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { name: "default".into(), process: String::new(), mappings: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub active_profile: String,
    pub last_target: Option<MonitorId>,
    pub select_region_hotkey: String,
    pub monitors: Vec<KnownMonitor>,
    pub profiles: Vec<Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: 1,
            active_profile: "default".into(),
            last_target: None,
            select_region_hotkey: "Ctrl+Alt+M".into(),
            monitors: Vec::new(),
            profiles: vec![Profile::default()],
        }
    }
}

pub struct Loaded {
    pub config: Config,
    /// Something the user should know about (e.g. a corrupt file was set aside).
    pub notice: Option<String>,
}

impl Config {
    /// `%APPDATA%\Mirage\config.toml`, or `./config.toml` if APPDATA is unset.
    pub fn default_path() -> PathBuf {
        match std::env::var_os("APPDATA") {
            Some(appdata) => PathBuf::from(appdata).join("Mirage").join("config.toml"),
            None => PathBuf::from("config.toml"),
        }
    }

    /// Missing file → defaults. Unreadable file → renamed to `<name>.bad`, defaults, notice.
    pub fn load(path: &Path) -> Loaded {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Loaded { config: Config::default(), notice: None },
        };
        match toml::from_str::<Config>(&text) {
            Ok(config) => Loaded { config, notice: None },
            Err(e) => {
                let bad = path.with_extension("toml.bad");
                let _ = std::fs::rename(path, &bad);
                Loaded {
                    config: Config::default(),
                    notice: Some(format!("config.toml could not be read ({e}); it was moved to {}", bad.display())),
                }
            }
        }
    }

    /// Atomic: writes `<path>.tmp`, then renames over the target.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn active_profile(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|p| p.name == self.active_profile)
            .unwrap_or_else(|| &self.profiles[0])
    }

    pub fn active_profile_mut(&mut self) -> &mut Profile {
        if !self.profiles.iter().any(|p| p.name == self.active_profile) {
            self.profiles.push(Profile { name: self.active_profile.clone(), ..Default::default() });
        }
        let name = self.active_profile.clone();
        self.profiles.iter_mut().find(|p| p.name == name).expect("just ensured")
    }

    pub fn known(&self, id: &str) -> Option<&KnownMonitor> {
        self.monitors.iter().find(|m| m.id == id)
    }

    /// Records newly seen monitors and refreshes name/size of known ones.
    /// Labels are never touched. Returns true if anything changed.
    pub fn note_attached(&mut self, attached: &[AttachedMonitor]) -> bool {
        let mut changed = false;
        for a in attached {
            let size = [a.info.width, a.info.height];
            match self.monitors.iter_mut().find(|m| m.id == a.id) {
                Some(m) => {
                    if m.last_name != a.friendly_name || m.last_size != size {
                        m.last_name = a.friendly_name.clone();
                        m.last_size = size;
                        changed = true;
                    }
                }
                None => {
                    self.monitors.push(KnownMonitor {
                        id: a.id.clone(),
                        label: default_label(&a.friendly_name, &a.id),
                        last_name: a.friendly_name.clone(),
                        last_size: size,
                    });
                    changed = true;
                }
            }
        }
        changed
    }
}

/// "HP 27xq (1FZ)" – friendly name plus the serial's last three characters,
/// so three identical monitors stay distinguishable.
pub fn default_label(friendly_name: &str, id: &str) -> String {
    let serial = match id.split_once(':') {
        Some(("edid", rest)) => rest.rsplit(':').next(),
        Some(("usb", serial)) => Some(serial),
        _ => None,
    };
    match serial.filter(|s| s.len() >= 3) {
        Some(s) => format!("{friendly_name} ({})", &s[s.len() - 3..]),
        None => friendly_name.to_string(),
    }
}

/// A mapping bound to the monitors currently attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveMapping {
    pub index: usize,
    pub source_handle: isize,
    pub source_size: (i32, i32),
    pub source_rect: Rect,
    pub target_handle: isize,
    /// Relative to the target monitor's top-left corner.
    pub target_rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Active(ActiveMapping),
    Dormant { index: usize, reason: String },
}

fn find<'a>(attached: &'a [AttachedMonitor], r: &MonitorRef) -> Option<&'a AttachedMonitor> {
    match r {
        MonitorRef::Primary => attached.iter().find(|m| m.info.is_primary),
        MonitorRef::Id(id) => attached.iter().find(|m| &m.id == id),
    }
}

/// Binds every mapping of `profile` to the attached monitors. Never fails:
/// a mapping whose source or target is missing comes back dormant.
pub fn resolve(profile: &Profile, attached: &[AttachedMonitor]) -> Vec<Resolved> {
    profile
        .mappings
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let Some(source) = find(attached, &m.source) else {
                return Resolved::Dormant { index, reason: "source monitor absent".into() };
            };
            let Some(target) = find(attached, &MonitorRef::Id(m.target.clone())) else {
                return Resolved::Dormant { index, reason: "target monitor absent".into() };
            };
            let source_size = (source.info.width, source.info.height);
            let target_rect = match m.target_rect.as_slice() {
                [x, y, w, h] if *w > 0 && *h > 0 => Rect { x: *x, y: *y, w: *w, h: *h }.clamp_to(target.info.width, target.info.height),
                _ => Rect { x: 0, y: 0, w: target.info.width, h: target.info.height },
            };
            Resolved::Active(ActiveMapping {
                index,
                source_handle: source.info.handle,
                source_size,
                source_rect: m.effective_rect(source_size),
                target_handle: target.info.handle,
                target_rect,
            })
        })
        .collect()
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test config`
Expected: 11 passed. (Dead-code warnings are fine until later tasks use everything.)

- [ ] **Step 7: Commit**

```powershell
git add Cargo.toml Cargo.lock src/config.rs src/identity.rs src/main.rs
git commit -m @'
feat: configuration data model with TOML persistence and mapping resolution

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
```

---

### Task 2: Monitor identity (EDID / USB serial / connector path)

**Files:**
- Modify: `src/identity.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `monitors::enumerate()`, `monitors::MonitorInfo`.
- Produces: `identity::{Edid, parse_edid, instance_id_from_device_path, is_weak_serial, usb_serial, choose_id, attached}`.

- [ ] **Step 1: Write the failing tests**

Append to `src/identity.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // First 128 EDID bytes of the HP 27xq with serial CNK04611FZ (dev machine).
    const HP: [u8; 128] = [
        0x00,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0x00,0x22,0x0E,0x82,0x35,0x00,0x00,0x00,0x00,0x2E,0x1E,0x01,0x04,0xA5,0x3C,0x22,0x78,0x3B,0x8C,0xE5,0xA5,0x58,0x50,0xA0,0x23,
        0x0B,0x50,0x54,0xA5,0x4B,0x00,0xD1,0xC0,0xA9,0xC0,0x81,0xC0,0xD1,0x00,0xB3,0x00,0x95,0x00,0x81,0x00,0xA9,0x40,0x56,0x5E,0x00,0xA0,0xA0,0xA0,0x29,0x50,0x30,0x20,
        0x35,0x00,0x55,0x50,0x21,0x00,0x00,0x1A,0x00,0x00,0x00,0xFD,0x00,0x30,0x90,0xDF,0xDF,0x3C,0x01,0x0A,0x20,0x20,0x20,0x20,0x20,0x20,0x00,0x00,0x00,0xFC,0x00,0x48,
        0x50,0x20,0x32,0x37,0x78,0x71,0x0A,0x20,0x20,0x20,0x20,0x20,0x00,0x00,0x00,0xFF,0x00,0x43,0x4E,0x4B,0x30,0x34,0x36,0x31,0x31,0x46,0x5A,0x0A,0x20,0x20,0x01,0xD4,
    ];
    // The Winwing DisplayLink screen: serial descriptor holds just "1".
    const WINWING: [u8; 128] = [
        0x00,0xFF,0xFF,0xFF,0xFF,0xFF,0xFF,0x00,0x48,0xA7,0x19,0x03,0x00,0x00,0x00,0x00,0x01,0x1E,0x01,0x03,0xA5,0x1C,0x10,0x78,0x02,0xE1,0xE7,0xA1,0x4F,0x4C,0x89,0x23,
        0x27,0x4A,0x54,0x00,0x00,0x00,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x01,0x64,0x19,0x00,0x40,0x41,0x00,0x26,0x30,0x00,0x40,
        0x06,0x12,0x00,0x00,0x00,0x00,0x00,0x1A,0x00,0x00,0x00,0xFD,0x00,0x0F,0xF0,0x0F,0x7F,0x11,0x00,0x0A,0x20,0x20,0x20,0x20,0x20,0x20,0x00,0x00,0x00,0xFC,0x00,0x55,
        0x53,0x42,0x5F,0x4D,0x6F,0x6E,0x69,0x74,0x6F,0x72,0x0A,0x20,0x00,0x00,0x00,0xFF,0x00,0x31,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x0A,0x00,0xDE,
    ];

    #[test]
    fn parses_hp_edid() {
        let e = parse_edid(&HP).unwrap();
        assert_eq!(e.manufacturer, "HPN");
        assert_eq!(e.product, 0x3582);
        assert_eq!(e.serial_u32, 0);
        assert_eq!(e.serial_str.as_deref(), Some("CNK04611FZ"));
    }

    #[test]
    fn parses_winwing_edid_with_placeholder_serial() {
        let e = parse_edid(&WINWING).unwrap();
        assert_eq!(e.manufacturer, "REG");
        assert_eq!(e.product, 0x0319);
        assert_eq!(e.serial_str.as_deref(), Some("1"));
    }

    #[test]
    fn rejects_short_or_unsigned_blobs() {
        assert!(parse_edid(&HP[..100]).is_none());
        let mut bad = HP;
        bad[1] = 0x00;
        assert!(parse_edid(&bad).is_none());
    }

    #[test]
    fn weak_serials() {
        assert!(is_weak_serial("1"));
        assert!(is_weak_serial("0"));
        assert!(is_weak_serial(""));
        assert!(is_weak_serial("0000000"));
        assert!(is_weak_serial("AAAA"));
        assert!(!is_weak_serial("CNK04611FZ"));
        assert!(!is_weak_serial("16780800"));
    }

    #[test]
    fn instance_id_from_device_path_strips_prefix_and_guid() {
        assert_eq!(
            instance_id_from_device_path("\\\\?\\DISPLAY#HPN3582#5&2f036cc&1&UID41219#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}").as_deref(),
            Some("DISPLAY\\HPN3582\\5&2f036cc&1&UID41219")
        );
        assert!(instance_id_from_device_path("garbage").is_none());
    }

    #[test]
    fn usb_serial_walks_to_the_first_real_usb_serial() {
        let ancestors = vec![
            "USB\\VID_17E9&PID_FF00&MI_00\\B&2D19B3BE&1&0000".to_string(),
            "USB\\VID_17E9&PID_FF00\\WWIN29320251005160820".to_string(),
            "USB\\ROOT_HUB30\\5&1234&0&0".to_string(),
        ];
        assert_eq!(usb_serial(&ancestors).as_deref(), Some("WWIN29320251005160820"));
        let gpu_only = vec!["PCI\\VEN_10DE&DEV_2C05\\76A3AD1E012DB04800".to_string()];
        assert_eq!(usb_serial(&gpu_only), None, "a PCI id is not a USB serial");
    }

    #[test]
    fn choose_id_prefers_edid_then_usb_then_path() {
        let hp = parse_edid(&HP).unwrap();
        let ww = parse_edid(&WINWING).unwrap();
        let ww_parents = vec!["USB\\VID_17E9&PID_FF00\\WWIN29320251005160820".to_string()];
        assert_eq!(choose_id(Some(&hp), &[], "DISPLAY\\HPN3582\\5&x"), ("edid:HPN:3582:CNK04611FZ".to_string(), false));
        assert_eq!(choose_id(Some(&ww), &ww_parents, "DISPLAY\\REG0319\\c&x"), ("usb:WWIN29320251005160820".to_string(), false));
        assert_eq!(choose_id(Some(&ww), &[], "DISPLAY\\REG0319\\c&x"), ("path:DISPLAY\\REG0319\\c&x".to_string(), true));
        assert_eq!(choose_id(None, &[], "DISPLAY\\Z\\1"), ("path:DISPLAY\\Z\\1".to_string(), true));
        let mut numeric = hp.clone();
        numeric.serial_str = None;
        numeric.serial_u32 = 16780800;
        assert_eq!(choose_id(Some(&numeric), &[], "x").0, "edid:HPN:3582:16780800");
    }
}
```

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo test identity`
Expected: compile errors – functions not found.

- [ ] **Step 3: Implement the pure part**

Insert after the `AttachedMonitor` struct in `src/identity.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    pub manufacturer: String,
    pub product: u16,
    pub serial_u32: u32,
    pub serial_str: Option<String>,
}

/// Parses the base EDID block. Returns None for anything that is not a
/// 128-byte block with the standard header.
pub fn parse_edid(bytes: &[u8]) -> Option<Edid> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if bytes.len() < 128 || bytes[..8] != HEADER {
        return None;
    }
    let mfg = u16::from_be_bytes([bytes[8], bytes[9]]);
    let letter = |v: u16| (b'A' + (v as u8).wrapping_sub(1)) as char;
    let manufacturer: String = [(mfg >> 10) & 0x1F, (mfg >> 5) & 0x1F, mfg & 0x1F].iter().map(|&v| letter(v)).collect();
    let product = u16::from_le_bytes([bytes[10], bytes[11]]);
    let serial_u32 = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let serial_str = [54usize, 72, 90, 108].iter().find_map(|&off| {
        let d = &bytes[off..off + 18];
        (d[0] == 0 && d[1] == 0 && d[2] == 0 && d[3] == 0xFF).then(|| {
            let text: String = d[5..18].iter().take_while(|&&b| b != 0x0A && b != 0).map(|&b| b as char).collect();
            text.trim().to_string()
        })
    });
    Some(Edid { manufacturer, product, serial_u32, serial_str })
}

/// Serial strings that manufacturers use as placeholders: too short, or one
/// repeated character ("0", "1", "0000000", "AAAA").
pub fn is_weak_serial(s: &str) -> bool {
    let s = s.trim();
    let mut chars = s.chars();
    match chars.next() {
        None => true,
        Some(first) => s.len() < 4 || chars.all(|c| c == first),
    }
}

/// `\\?\DISPLAY#HPN3582#5&2f036cc&1&UID41219#{guid}` → `DISPLAY\HPN3582\5&2f036cc&1&UID41219`
pub fn instance_id_from_device_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("\\\\?\\")?;
    let without_guid = match rest.rfind("#{") {
        Some(i) => &rest[..i],
        None => rest,
    };
    let id = without_guid.replace('#', "\\");
    (id.matches('\\').count() >= 2).then_some(id)
}

/// The first ancestor that is a USB device whose instance id ends in a real
/// serial (Windows-generated ids contain '&', serials don't).
pub fn usb_serial(ancestors: &[String]) -> Option<String> {
    ancestors.iter().find_map(|id| {
        let last = id.rsplit('\\').next()?;
        (id.starts_with("USB\\") && !last.contains('&') && last.len() >= 4).then(|| last.to_string())
    })
}

/// Identity rules from the spec. Returns the id and whether it is port-bound.
pub fn choose_id(edid: Option<&Edid>, ancestors: &[String], instance_id: &str) -> (MonitorId, bool) {
    if let Some(e) = edid {
        if let Some(s) = e.serial_str.as_deref().filter(|s| !is_weak_serial(s)) {
            return (format!("edid:{}:{:04X}:{}", e.manufacturer, e.product, s), false);
        }
        if e.serial_u32 != 0 {
            return (format!("edid:{}:{:04X}:{}", e.manufacturer, e.product, e.serial_u32), false);
        }
    }
    if let Some(s) = usb_serial(ancestors) {
        return (format!("usb:{s}"), false);
    }
    (format!("path:{instance_id}"), true)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test identity`
Expected: 7 passed.

- [ ] **Step 5: Implement the Win32 part**

Add these imports at the top of `src/identity.rs`:

```rust
use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Device_ID_Size, CM_Get_Parent, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ};

use crate::monitors::{self, MonitorInfo};
```

(remove the earlier `use crate::monitors::MonitorInfo;`). Then append before the tests:

```rust
fn utf16_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct TargetInfo {
    device_path: String,
    friendly_name: String,
}

/// GDI name (`\\.\DISPLAYn`) → target device path + friendly name, via DisplayConfig.
fn display_config_targets() -> anyhow::Result<HashMap<String, TargetInfo>> {
    let mut n_paths = 0u32;
    let mut n_modes = 0u32;
    let err = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes) };
    anyhow::ensure!(err == ERROR_SUCCESS, "GetDisplayConfigBufferSizes: {err:?}");
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
    let err = unsafe {
        QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, paths.as_mut_ptr(), &mut n_modes, modes.as_mut_ptr(), None)
    };
    anyhow::ensure!(err == ERROR_SUCCESS, "QueryDisplayConfig: {err:?}");

    let mut out = HashMap::new();
    for path in &paths[..n_paths as usize] {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };
        let ok_source = unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0;
        let ok_target = unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } == 0;
        if ok_source && ok_target {
            out.insert(
                utf16_to_string(&source.viewGdiDeviceName),
                TargetInfo {
                    device_path: utf16_to_string(&target.monitorDevicePath),
                    friendly_name: utf16_to_string(&target.monitorFriendlyDeviceName),
                },
            );
        }
    }
    Ok(out)
}

/// `HKLM\SYSTEM\CurrentControlSet\Enum\<instance>\Device Parameters\EDID`
fn read_edid(instance_id: &str) -> Option<Vec<u8>> {
    let sub = to_wide(&format!("SYSTEM\\CurrentControlSet\\Enum\\{instance_id}\\Device Parameters"));
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(sub.as_ptr()), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return None;
    }
    let mut len = 0u32;
    let mut result = None;
    if unsafe { RegQueryValueExW(key, w!("EDID"), None, None, None, Some(&mut len)) } == ERROR_SUCCESS && len >= 128 {
        let mut buf = vec![0u8; len as usize];
        if unsafe { RegQueryValueExW(key, w!("EDID"), None, None, Some(buf.as_mut_ptr()), Some(&mut len)) } == ERROR_SUCCESS {
            result = Some(buf);
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

/// Instance ids of the device's parent, grandparent, … (at most four levels).
fn ancestor_ids(instance_id: &str) -> Vec<String> {
    let id = to_wide(instance_id);
    let mut dev = 0u32;
    if unsafe { CM_Locate_DevNodeW(&mut dev, PCWSTR(id.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) } != CR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for _ in 0..4 {
        let mut parent = 0u32;
        if unsafe { CM_Get_Parent(&mut parent, dev, 0) } != CR_SUCCESS {
            break;
        }
        let mut len = 0u32;
        if unsafe { CM_Get_Device_ID_Size(&mut len, parent, 0) } != CR_SUCCESS {
            break;
        }
        let mut buf = vec![0u16; len as usize + 1];
        if unsafe { CM_Get_Device_IDW(parent, &mut buf, 0) } != CR_SUCCESS {
            break;
        }
        out.push(utf16_to_string(&buf));
        dev = parent;
    }
    out
}

/// Every active monitor with its identity. Monitors DisplayConfig does not
/// know about (should not happen) fall back to a `path:` id on the GDI name.
pub fn attached() -> anyhow::Result<Vec<AttachedMonitor>> {
    let targets = display_config_targets()?;
    Ok(monitors::enumerate()?
        .into_iter()
        .map(|info| {
            let gdi_name = info.device_name.clone();
            let (id, friendly_name, port_bound) = match targets.get(&gdi_name) {
                Some(t) => {
                    let instance = instance_id_from_device_path(&t.device_path).unwrap_or_else(|| t.device_path.clone());
                    let edid = read_edid(&instance).and_then(|b| parse_edid(&b));
                    let (id, port_bound) = choose_id(edid.as_ref(), &ancestor_ids(&instance), &instance);
                    let name = if t.friendly_name.is_empty() { "Generic Monitor".to_string() } else { t.friendly_name.clone() };
                    (id, name, port_bound)
                }
                None => (format!("path:{gdi_name}"), "Unknown monitor".to_string(), true),
            };
            AttachedMonitor { id, gdi_name, friendly_name, info, port_bound }
        })
        .collect())
}
```

- [ ] **Step 6: Make `--list` print identities**

In `src/main.rs`, replace the block from `let monitors = monitors::enumerate()?;` through `if args.list { return Ok(()); }` with:

```rust
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
```

- [ ] **Step 7: Verify against the real machine**

Run: `cargo run -- --list`
Expected: one line per attached monitor. The HPs show `edid:HPN:3582:CNK…` ids, the Winwing screen shows `usb:WWIN29320251005160820` with name `USB_Monitor`, nobody is `port-bound`. Ids must stay the same after re-plugging the Winwing screen or swapping which HP is primary.

Run: `cargo test`
Expected: 38 passed (20 from M0 + 11 config + 7 identity).

- [ ] **Step 8: Commit**

```powershell
git add src/identity.rs src/main.rs
git commit -m @'
feat: hardware monitor identity from EDID, USB serial or connector path

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
```

---

### Task 3: Renderer split, overlay shader, frame grab, window helpers

**Files:**
- Rewrite: `src/renderer.rs`
- Modify: `src/shaders.hlsl`, `src/capture.rs`, `src/window.rs`, `src/main.rs`

**Interfaces:**
- Produces:
  - `renderer::Pipeline::new(&D3d)`, `Pipeline::draw_mirror(&self, ctx, target: &SwapChainTarget, src: &SourceTexture, uv: [f32;4], dst: Rect)`, `Pipeline::draw_overlay(&self, ctx, target, src, selection: Option<Rect>)`.
  - `renderer::SwapChainTarget::new(&D3d, HWND, w: u32, h: u32)`, `wait_for_frame_slot`, `clear(ctx)`, `present`, `width()`, `height()`.
  - `renderer::SourceTexture::new(device, w, h) -> Result<Self>`, `SourceTexture::black(device, w, h)`, `copy_from(&self, ctx, &ID3D11Texture2D)`, fields `width`, `height`.
  - `capture::grab_one_frame(d3d: &D3d, monitor_handle: isize, width: u32, height: u32, timeout_ms: u32) -> anyhow::Result<SourceTexture>`.
  - `window::create_output_window(x, y, w, h)`, `window::create_overlay_window(x, y, w, h, userdata: *mut c_void, proc: WNDPROC) -> Result<HWND>`, `window::create_message_window(userdata) -> Result<HWND>`, `window::WindowSignals { topology_changed: Cell<bool>, hotkey: Cell<bool> }`, `window::mouse_pos(lparam) -> (i32, i32)`, `window::pump_messages()`.
- The M0 command-line mirror keeps working through this task (verification), so `main.rs` is adapted to the new API but not yet restructured.

- [ ] **Step 1: Rewrite `src/renderer.rs`**

```rust
use anyhow::{anyhow, bail};
use windows::core::{s, Interface, PCSTR};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    IDXGISwapChain1, IDXGISwapChain2, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use crate::d3d::D3d;
use crate::geometry::Rect;

const SHADER_SOURCE: &str = include_str!("shaders.hlsl");

/// A GPU-resident copy of a captured frame that shaders can sample.
pub struct SourceTexture {
    pub texture: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    pub width: u32,
    pub height: u32,
}

impl SourceTexture {
    pub fn new(device: &ID3D11Device, width: u32, height: u32) -> anyhow::Result<Self> {
        Self::create(device, width, height, None)
    }

    /// Zero-initialised – the only CPU-side pixel data in Mirage, used when a
    /// monitor yields no frame in time during selection.
    pub fn black(device: &ID3D11Device, width: u32, height: u32) -> anyhow::Result<Self> {
        let zeros = vec![0u8; (width * height * 4) as usize];
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: zeros.as_ptr() as *const _, SysMemPitch: width * 4, SysMemSlicePitch: 0 };
        Self::create(device, width, height, Some(&init as *const _))
    }

    fn create(device: &ID3D11Device, width: u32, height: u32, init: Option<*const D3D11_SUBRESOURCE_DATA>) -> anyhow::Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, init, Some(&mut texture)) }?;
        let texture = texture.ok_or_else(|| anyhow!("no texture"))?;
        let mut srv = None;
        unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut srv)) }?;
        Ok(SourceTexture { texture, srv: srv.ok_or_else(|| anyhow!("no shader resource view"))?, width, height })
    }

    /// GPU copy; `frame` must have the same size and format.
    pub fn copy_from(&self, ctx: &ID3D11DeviceContext, frame: &ID3D11Texture2D) {
        unsafe { ctx.CopyResource(&self.texture, frame) };
    }
}

/// Flip-model swap chain plus render target view for one window.
pub struct SwapChainTarget {
    swap_chain: IDXGISwapChain2,
    frame_latency_waitable: HANDLE,
    rtv: ID3D11RenderTargetView,
    width: u32,
    height: u32,
}

impl SwapChainTarget {
    pub fn new(d3d: &D3d, hwnd: HWND, width: u32, height: u32) -> anyhow::Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            ..Default::default()
        };
        let swap_chain: IDXGISwapChain1 = unsafe { d3d.factory.CreateSwapChainForHwnd(&d3d.device, hwnd, &desc, None, None) }?;
        let swap_chain: IDXGISwapChain2 = swap_chain.cast()?;
        unsafe {
            swap_chain.SetMaximumFrameLatency(1)?;
            d3d.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
        }
        let frame_latency_waitable = unsafe { swap_chain.GetFrameLatencyWaitableObject() };
        let back_buffer: ID3D11Texture2D = unsafe { swap_chain.GetBuffer(0) }?;
        let mut rtv = None;
        unsafe { d3d.device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv)) }?;
        Ok(SwapChainTarget { swap_chain, frame_latency_waitable, rtv: rtv.ok_or_else(|| anyhow!("no render target view"))?, width, height })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Blocks until the swap chain can accept another frame (at most ~1 s).
    /// On the dev machine this never blocks (PLAN.md, M0 results) – pacing is
    /// event driven; this is only a safety net.
    pub fn wait_for_frame_slot(&self) {
        unsafe {
            let _ = WaitForSingleObjectEx(self.frame_latency_waitable, 1000, false);
        }
    }

    pub fn clear(&self, ctx: &ID3D11DeviceContext) {
        unsafe {
            ctx.ClearRenderTargetView(&self.rtv, &[0.0, 0.0, 0.0, 1.0]);
            ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
        }
    }

    pub fn present(&self) -> anyhow::Result<()> {
        unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) }.ok()?;
        Ok(())
    }
}

/// Shaders and state shared by every window. One per device.
pub struct Pipeline {
    vs: ID3D11VertexShader,
    ps_mirror: ID3D11PixelShader,
    ps_overlay: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    crop_buffer: ID3D11Buffer,
    overlay_buffer: ID3D11Buffer,
}

impl Pipeline {
    pub fn new(d3d: &D3d) -> anyhow::Result<Self> {
        let vs_blob = compile(s!("vs_main"), s!("vs_5_0"))?;
        let ps_blob = compile(s!("ps_main"), s!("ps_5_0"))?;
        let ov_blob = compile(s!("ps_overlay"), s!("ps_5_0"))?;
        let mut vs = None;
        let mut ps_mirror = None;
        let mut ps_overlay = None;
        unsafe {
            d3d.device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))?;
            d3d.device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps_mirror))?;
            d3d.device.CreatePixelShader(blob_bytes(&ov_blob), None, Some(&mut ps_overlay))?;
        }
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        unsafe { d3d.device.CreateSamplerState(&sampler_desc, Some(&mut sampler)) }?;
        Ok(Pipeline {
            vs: vs.ok_or_else(|| anyhow!("no vertex shader"))?,
            ps_mirror: ps_mirror.ok_or_else(|| anyhow!("no mirror shader"))?,
            ps_overlay: ps_overlay.ok_or_else(|| anyhow!("no overlay shader"))?,
            sampler: sampler.ok_or_else(|| anyhow!("no sampler"))?,
            crop_buffer: constant_buffer(&d3d.device, 16)?,
            overlay_buffer: constant_buffer(&d3d.device, 32)?,
        })
    }

    /// Draws `src` cropped to `uv` (`[u0, v0, du, dv]`) into the window
    /// rectangle `dst` of the current render target.
    pub fn draw_mirror(&self, ctx: &ID3D11DeviceContext, target: &SwapChainTarget, src: &SourceTexture, uv: [f32; 4], dst: Rect) {
        unsafe {
            ctx.UpdateSubresource(&self.crop_buffer, 0, None, uv.as_ptr() as *const _, 0, 0);
            self.bind(ctx, src, &self.ps_mirror, dst, target);
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
    }

    /// Draws the frozen `src` over the whole window, dimmed outside `selection`
    /// (frame pixels) with a border around it; no selection dims everything.
    pub fn draw_overlay(&self, ctx: &ID3D11DeviceContext, target: &SwapChainTarget, src: &SourceTexture, selection: Option<Rect>) {
        let (w, h) = (src.width as f32, src.height as f32);
        let sel = match selection {
            Some(r) => [r.x as f32 / w, r.y as f32 / h, (r.x + r.w) as f32 / w, (r.y + r.h) as f32 / h],
            None => [0.0; 4],
        };
        let params: [f32; 8] = [sel[0], sel[1], sel[2], sel[3], 1.0 / w, 1.0 / h, if selection.is_some() { 1.0 } else { 0.0 }, 0.0];
        let full = Rect { x: 0, y: 0, w: target.width as i32, h: target.height as i32 };
        unsafe {
            ctx.UpdateSubresource(&self.crop_buffer, 0, None, [0.0f32, 0.0, 1.0, 1.0].as_ptr() as *const _, 0, 0);
            ctx.UpdateSubresource(&self.overlay_buffer, 0, None, params.as_ptr() as *const _, 0, 0);
            self.bind(ctx, src, &self.ps_overlay, full, target);
            ctx.PSSetConstantBuffers(1, Some(&[Some(self.overlay_buffer.clone())]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
    }

    unsafe fn bind(&self, ctx: &ID3D11DeviceContext, src: &SourceTexture, ps: &ID3D11PixelShader, dst: Rect, target: &SwapChainTarget) {
        let _ = target;
        unsafe {
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: dst.x as f32,
                TopLeftY: dst.y as f32,
                Width: dst.w as f32,
                Height: dst.h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.IASetInputLayout(None);
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.crop_buffer.clone())]));
            ctx.PSSetShader(ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(src.srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        }
    }
}

fn constant_buffer(device: &ID3D11Device, bytes: u32) -> anyhow::Result<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: bytes,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        ..Default::default()
    };
    let mut buffer = None;
    unsafe { device.CreateBuffer(&desc, None, Some(&mut buffer)) }?;
    buffer.ok_or_else(|| anyhow!("no constant buffer"))
}

fn compile(entry: PCSTR, target: PCSTR) -> anyhow::Result<ID3DBlob> {
    let mut blob = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            SHADER_SOURCE.as_ptr() as *const _,
            SHADER_SOURCE.len(),
            PCSTR::null(),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let message = errors.map(|b| String::from_utf8_lossy(blob_bytes(&b)).into_owned()).unwrap_or_default();
        bail!("shader compilation failed: {e}\n{message}");
    }
    blob.ok_or_else(|| anyhow!("D3DCompile returned no bytecode"))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}
```

- [ ] **Step 2: Add the overlay pixel shader**

Append to `src/shaders.hlsl`:

```hlsl
// Selection overlay: frozen frame, dimmed outside the selection, 2 px border.
cbuffer Overlay : register(b1)
{
    float4 sel;    // u0, v0, u1, v1 of the selection
    float4 px;     // 1/w, 1/h, has_selection, unused
};

float4 ps_overlay(VsOut i) : SV_Target
{
    float4 c = source.Sample(linear_clamp, i.uv);
    float4 dim = c * float4(0.4, 0.4, 0.4, 1.0);
    if (px.z < 0.5)
        return dim;
    bool inside = i.uv.x >= sel.x && i.uv.x <= sel.z && i.uv.y >= sel.y && i.uv.y <= sel.w;
    if (inside)
        return c;
    float2 b = px.xy * 2.0;
    bool border = i.uv.x >= sel.x - b.x && i.uv.x <= sel.z + b.x && i.uv.y >= sel.y - b.y && i.uv.y <= sel.w + b.y;
    return border ? float4(1.0, 0.85, 0.2, 1.0) : dim;
}
```

- [ ] **Step 3: Add `grab_one_frame` to `src/capture.rs`**

Append (add `use windows::Win32::System::Threading::WaitForSingleObjectEx;`, `use crate::d3d::D3d;` and `use crate::renderer::SourceTexture;` to the imports):

```rust
/// One frame of `monitor_handle` as a fresh texture, via a temporary capture
/// session. A monitor that yields nothing within `timeout_ms` (e.g. one that
/// is asleep) gives a black texture so selection still works there.
pub fn grab_one_frame(d3d: &D3d, monitor_handle: isize, width: u32, height: u32, timeout_ms: u32) -> anyhow::Result<SourceTexture> {
    let mut capture = MonitorCapture::new(d3d.winrt_device()?, monitor_handle)?;
    unsafe {
        let _ = WaitForSingleObjectEx(capture.frame_event(), timeout_ms, false);
    }
    let mut newest = None;
    while let Some(frame) = capture.try_next_frame()? {
        newest = Some(frame);
    }
    match newest {
        Some(frame) if frame.width as u32 == width && frame.height as u32 == height => {
            let texture = SourceTexture::new(&d3d.device, width, height)?;
            texture.copy_from(&d3d.context, &frame.texture);
            Ok(texture)
        }
        _ => SourceTexture::black(&d3d.device, width, height),
    }
}
```

Also silence the `println!` in `MonitorCapture::new` about the capture item size (it would spam during selection): delete the line `println!("capture item {}x{}", size.Width, size.Height);`.

- [ ] **Step 4: Extend `src/window.rs`**

Replace the file with:

```rust
use std::cell::Cell;
use std::ffi::c_void;

use anyhow::ensure;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetWindowLongPtrW, KillTimer, LoadCursorW, PeekMessageW, PostQuitMessage,
    RegisterClassW, SetTimer, SetWindowLongPtrW, ShowWindow, TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, HWND_MESSAGE, IDC_ARROW,
    IDC_CROSS, MA_NOACTIVATE, MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WM_CREATE, WM_DESTROY, WM_DEVICECHANGE,
    WM_DISPLAYCHANGE, WM_HOTKEY, WM_MOUSEACTIVATE, WM_QUIT, WM_TIMER, WNDCLASSW, WNDPROC, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

pub struct OutputWindow {
    pub hwnd: HWND,
    pub width: i32,
    pub height: i32,
}

fn register_class(name: PCWSTR, proc: WNDPROC, cursor: PCWSTR) -> anyhow::Result<()> {
    let instance = unsafe { GetModuleHandleW(None) }?;
    let class = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: instance.into(),
        lpszClassName: name,
        hCursor: unsafe { LoadCursorW(None, cursor) }?,
        ..Default::default()
    };
    // Registering twice fails with ERROR_CLASS_ALREADY_EXISTS – harmless.
    unsafe { RegisterClassW(&class) };
    Ok(())
}

fn create(ex_style: WINDOW_EX_STYLE, class: PCWSTR, x: i32, y: i32, w: i32, h: i32, parent: Option<HWND>, userdata: *mut c_void) -> anyhow::Result<HWND> {
    let instance = unsafe { GetModuleHandleW(None) }?;
    let hwnd = unsafe {
        CreateWindowExW(ex_style, class, w!("Mirage"), WS_POPUP, x, y, w, h, parent, None, Some(instance.into()), Some(userdata as *const c_void))
    }?;
    ensure!(!hwnd.is_invalid(), "CreateWindowExW returned null");
    Ok(hwnd)
}

/// A borderless window covering exactly the given monitor rectangle. Topmost so
/// nothing on the cockpit screen hides it, NOACTIVATE + TOOLWINDOW so it never
/// takes focus from the game and stays out of Alt-Tab.
pub fn create_output_window(x: i32, y: i32, width: i32, height: i32) -> anyhow::Result<OutputWindow> {
    register_class(w!("MirageOutput"), Some(output_proc), IDC_ARROW)?;
    let hwnd = create(WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, w!("MirageOutput"), x, y, width, height, None, std::ptr::null_mut())?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(OutputWindow { hwnd, width, height })
}

unsafe extern "system" fn output_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // A click on the cockpit screen must not activate us either.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Selection overlay window: topmost, *activatable* (needs keyboard + mouse
/// capture), crosshair cursor. `userdata` is stored in GWLP_USERDATA for `proc`.
pub fn create_overlay_window(x: i32, y: i32, width: i32, height: i32, userdata: *mut c_void, proc: WNDPROC) -> anyhow::Result<HWND> {
    register_class(w!("MirageOverlay"), proc, IDC_CROSS)?;
    create(WS_EX_TOPMOST, w!("MirageOverlay"), x, y, width, height, None, userdata)
}

/// Flags raised by the hidden message window, polled by the render loop.
#[derive(Default)]
pub struct WindowSignals {
    pub topology_changed: Cell<bool>,
    pub hotkey: Cell<bool>,
}

const TOPOLOGY_TIMER: usize = 1;

/// Hidden window that receives WM_DISPLAYCHANGE / WM_DEVICECHANGE (debounced
/// into `topology_changed` 500 ms after the last one) and WM_HOTKEY.
pub fn create_message_window(signals: *const WindowSignals) -> anyhow::Result<HWND> {
    register_class(w!("MirageMessages"), Some(message_proc), IDC_ARROW)?;
    create(WINDOW_EX_STYLE(0), w!("MirageMessages"), 0, 0, 0, 0, Some(HWND_MESSAGE), signals as *mut c_void)
}

unsafe extern "system" fn message_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_CREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return LRESULT(0);
        }
        let signals = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowSignals;
        match msg {
            WM_DISPLAYCHANGE | WM_DEVICECHANGE => {
                SetTimer(Some(hwnd), TOPOLOGY_TIMER, 500, None);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == TOPOLOGY_TIMER => {
                let _ = KillTimer(Some(hwnd), TOPOLOGY_TIMER);
                if !signals.is_null() {
                    (*signals).topology_changed.set(true);
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                if !signals.is_null() {
                    (*signals).hotkey.set(true);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Reads the userdata pointer a window was created with (see `create_*`).
pub unsafe fn userdata<T>(hwnd: HWND, msg: u32, lparam: LPARAM) -> *const T {
    unsafe {
        if msg == WM_CREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const T
    }
}

/// Client coordinates from a mouse message's LPARAM (physical pixels).
pub fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
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
```

- [ ] **Step 5: Adapt `main.rs` to the new API (M0 behaviour preserved)**

In `src/main.rs`, replace everything from `let output = window::create(` to the end of the loop with:

```rust
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
            pipeline.draw_mirror(&d3d.context, &target, src_tex, rect.clamp_to(w, h).to_uv(w, h), dst);
            target.present()?;
            stats.presented += 1;
        }
        stats.report_if_due();
    }
    Ok(())
```

Remove the now-unused `WAIT_OBJECT_0` import.

- [ ] **Step 6: Build, test, and verify the mirror still works**

Run: `cargo build 2>&1 | grep -E "^error|warning: unused" ; cargo test 2>&1 | grep "test result"`
Expected: builds (dead-code warnings for `draw_overlay`, `grab_one_frame`, overlay/message windows are fine), 38 tests pass.

Run: `cargo run -- --rect 0,0,1280,720` – look at the cockpit screen.
Expected: exactly the M0 behaviour – top-left quarter of the primary mirrored, ~120 fps captured/presented in the console.

- [ ] **Step 7: Commit**

```powershell
git add src/renderer.rs src/shaders.hlsl src/capture.rs src/window.rs src/main.rs
git commit -m @'
refactor: split renderer into pipeline and swap-chain target, add overlay shader and window helpers

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
```

---

### Task 4: Render engine thread

**Files:**
- Create: `src/engine.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `config::{Config, resolve, Resolved, ActiveMapping}`, `identity::attached`, `renderer::*`, `capture::MonitorCapture`, `window::*`, `d3d`.
- Produces: `engine::{Command, Event, RegionSelected, EngineHandle, spawn}`; `EngineHandle::send(Command)`, `EngineHandle::shutdown(self)`.
- Selection is wired in Task 5; this task makes `SelectRegion` and the hotkey emit `Event::Error("selection not implemented yet")` so the plumbing is testable.

- [ ] **Step 1: Write `src/engine.rs`**

```rust
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT};

use crate::capture::MonitorCapture;
use crate::config::{resolve, ActiveMapping, Config, Resolved};
use crate::d3d::{self, D3d};
use crate::geometry::Rect;
use crate::identity::{self, AttachedMonitor, MonitorId};
use crate::renderer::{Pipeline, SourceTexture, SwapChainTarget};
use crate::window::{self, OutputWindow, WindowSignals};

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
    let thread = std::thread::Builder::new().name("mirage-render".into()).spawn(move || {
        if let Err(e) = run(rx, wake, events.clone(), repaint.clone()) {
            let _ = events.send(Event::Error(format!("render thread stopped: {e:#}")));
            repaint();
        }
    })?;
    Ok(EngineHandle { tx, wake, thread: Some(thread) })
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
}

const HOTKEY_ID: i32 = 1;

fn run(rx: Receiver<Command>, wake: isize, events: Sender<Event>, repaint: Arc<dyn Fn() + Send + Sync>) -> anyhow::Result<()> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }?;
    let signals = Box::new(WindowSignals::default());
    let message_hwnd = window::create_message_window(&*signals as *const WindowSignals)?;
    // The device is created for whatever is primary now; a primary on another
    // GPU would need a rebuild of the device – not supported (PLAN.md: one GPU).
    let primary = identity::attached()?.into_iter().find(|m| m.info.is_primary).context("no primary monitor")?;
    let d3d = d3d::create_for_monitor(primary.info.handle)?;
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
    };
    if let Err(e) = unsafe { RegisterHotKey(Some(message_hwnd), HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'M' as u32) } {
        engine.emit(Event::Error(format!("Ctrl+Alt+M could not be registered ({e}); use the button instead")));
    }
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
                    engine.rebuild();
                }
                Ok(Command::SelectRegion) => engine.start_selection(),
                Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                    engine.outputs.clear();
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
        engine.pump_frames();
        engine.present_dirty();
    }
    Ok(())
}

impl Engine {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        (self.repaint)();
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
                    self.sources.insert(handle, Source { capture, texture: None, dirty: false });
                }
                Err(e) => self.emit(Event::Error(format!("capture of a source monitor failed: {e:#}"))),
            }
        }

        let target_handles: HashSet<isize> = self.active.iter().map(|m| m.target_handle).collect();
        for handle in target_handles {
            let Some(monitor) = self.attached.iter().find(|m| m.info.handle == handle) else { continue };
            let info = &monitor.info;
            let result = window::create_output_window(info.x, info.y, info.width, info.height)
                .and_then(|w| SwapChainTarget::new(&self.d3d, w.hwnd, w.width as u32, w.height as u32).map(|t| (w, t)));
            match result {
                Ok((window, target)) => {
                    let mappings = self.active.iter().filter(|m| m.target_handle == handle).cloned().collect();
                    self.outputs.insert(handle, Output { window, target, mappings });
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
                        let _ = self.events.send(Event::Error(format!("capture error on monitor {handle}: {e:#}")));
                        break;
                    }
                }
            }
            if let Some(frame) = newest {
                let (fw, fh) = (frame.width as u32, frame.height as u32);
                if source.texture.as_ref().map_or(true, |t| t.width != fw || t.height != fh) {
                    match SourceTexture::new(device, fw, fh) {
                        Ok(t) => source.texture = Some(t),
                        Err(e) => {
                            let _ = self.events.send(Event::Error(format!("texture creation failed: {e:#}")));
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
            let needs = out.mappings.iter().any(|m| self.sources.get(&m.source_handle).is_some_and(|s| s.dirty));
            if !needs {
                continue;
            }
            out.target.wait_for_frame_slot();
            out.target.clear(ctx);
            for m in &out.mappings {
                let Some(src) = self.sources.get(&m.source_handle).and_then(|s| s.texture.as_ref()) else { continue };
                let (w, h) = (src.width as i32, src.height as i32);
                let uv = m.source_rect.clamp_to(w, h).to_uv(w, h);
                self.pipeline.draw_mirror(ctx, &out.target, src, uv, m.target_rect);
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
        self.emit(Event::Error("region selection is not implemented yet".into()));
    }
}
```

- [ ] **Step 2: Temporary headless `main.rs` that drives the engine**

Replace `src/main.rs` entirely (the `--source/--target/--rect` flags stop working here; `args.rs` is trimmed in Task 6):

```rust
mod args;
mod capture;
mod config;
mod d3d;
mod engine;
mod geometry;
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
```

- [ ] **Step 3: Build, then verify with a hand-written config**

Run: `cargo build 2>&1 | grep -E "^error"` – fix anything reported.

Write `%APPDATA%\Mirage\config.toml` (replace the target id with the Winwing id from `cargo run -- --list`):

```toml
version = 1
active_profile = "default"

[[profiles]]
name = "default"

[[profiles.mappings]]
source = "primary"
source_rect = [0, 0, 1280, 720]
source_size = [2560, 1440]
target = "usb:WWIN29320251005160820"
```

Run: `cargo run`
Expected: `monitors: [...]` line, then the cockpit screen shows the top-left quarter of the primary, live. No `error:` lines.

Hot-plug test while it runs: unplug the Winwing USB cable → within ~1 s a new `monitors:` line without the usb id; nothing crashes. Plug it back → `monitors:` line with the id, and the mirror comes back by itself.

Ctrl+C ends the process cleanly.

- [ ] **Step 4: Commit**

```powershell
git add src/engine.rs src/main.rs
git commit -m @'
feat: render engine thread with config-driven sources, outputs and hot-plug rebuild

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
```

---

### Task 5: Selection overlay

**Files:**
- Create: `src/overlay.rs`
- Modify: `src/engine.rs`, `src/main.rs`

**Interfaces:**
- Produces: `overlay::Selection::start(d3d, attached) -> anyhow::Result<Selection>`, `Selection::redraw(&mut self, d3d, pipeline)`, `Selection::poll(&self) -> Option<Outcome>`, `overlay::Outcome::{Done { monitor_index, rect }, Cancelled}`.
- Engine: `start_selection` creates a `Selection`; the loop redraws it and on an outcome emits `Event::RegionSelected` / `Event::SelectionCancelled` and destroys the overlays.

- [ ] **Step 1: Write `src/overlay.rs`**

```rust
use std::cell::Cell;
use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, SetForegroundWindow, ShowWindow, SW_SHOW, WM_CREATE, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_RBUTTONDOWN,
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
        let shared = Box::new(Shared { anchor: Cell::new(None), current: Cell::new(None), outcome: Cell::new(None) });
        let mut windows = Vec::new();
        for (index, m) in attached.iter().enumerate() {
            let info = &m.info;
            let frame = capture::grab_one_frame(d3d, info.handle, info.width as u32, info.height as u32, 1000)?;
            let ctx = Box::new(WindowCtx { shared: &*shared as *const Shared, index, width: info.width, height: info.height });
            let hwnd = window::create_overlay_window(
                info.x,
                info.y,
                info.width,
                info.height,
                &*ctx as *const WindowCtx as *mut c_void,
                Some(overlay_proc),
            )?;
            let target = SwapChainTarget::new(d3d, hwnd, info.width as u32, info.height as u32)?;
            windows.push(OverlayWindow { hwnd, target, frame, index, _ctx: ctx, last_drawn: None });
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
        Ok(Selection { shared, windows })
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
    Rect { x: x0, y: y0, w: (x1 - x0).max(1), h: (y1 - y0).max(1) }.clamp_to(width, height)
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
                        shared.current.set(Some((i, normalised(ax, ay, x, y, ctx.width, ctx.height))));
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
```

- [ ] **Step 2: Wire the selection into the engine**

In `src/engine.rs`:
- add `mod`-level import `use crate::overlay::{Outcome, Selection};`
- add the field `selection: Option<Selection>,` to `Engine` and initialise it with `None`.
- replace `start_selection`:

```rust
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
```

- in the loop in `run`, after `engine.present_dirty();` add `engine.pump_selection();`.
- in `rebuild()`, add `self.selection = None;` right after the `MonitorsChanged` emit (a topology change during selection cancels it).
- add `mod overlay;` to `src/main.rs`.

- [ ] **Step 3: Headless main reacts to a selection**

In `src/main.rs`'s event loop, keep a mutable copy of the config and handle the new event:

Replace `engine.send(Command::Apply(loaded.config));` … through the end of the `while` loop with:

```rust
    let mut config = loaded.config;
    engine.send(Command::Apply(config.clone()));
    println!("engine running – Ctrl+Alt+M selects a region, Ctrl+C quits");
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
```

- [ ] **Step 4: Build and verify the overlay**

Run: `cargo run` (the config from Task 4 is still in place, so the mirror comes up first).

Then press `Ctrl+Alt+M`:
Expected: every monitor dims (frozen image of itself), cursor is a crosshair. Drag on the primary → the dragged rectangle is shown undimmed with a yellow border while dragging; release → overlays vanish, console prints `selected Rect {...} on edid:… (primary=true)`, and the cockpit screen shows the new region within a second.

Press `Ctrl+Alt+M`, then `Esc` → overlays vanish, console prints `selection cancelled`, mirror unchanged. Same with a right-click.

Drag a tiny (< 8 px) rectangle → nothing happens, overlay stays up; drag a proper one afterwards → works.

Drag on a *non-primary* HP → `primary=false` and the concrete `edid:` id is printed; the mirror shows that monitor's region.

Restart `cargo run` → the last selected region is restored from `config.toml`.

- [ ] **Step 5: Commit**

```powershell
git add src/overlay.rs src/engine.rs src/main.rs
git commit -m @'
feat: snipping-tool style region selection overlay on all monitors

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
```

---

### Task 6: Settings window (egui) and final startup

**Files:**
- Create: `src/ui.rs`
- Rewrite: `src/main.rs`, `src/args.rs`

**Interfaces:**
- Produces: `ui::App::new(config, path, engine, events, notice) -> App`, `impl eframe::App for App`.
- `args.rs` shrinks to `--list` only.

- [ ] **Step 1: Write `src/ui.rs`**

```rust
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;

use crate::config::{self, Config, Mapping, MonitorRef, Resolved};
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
}

impl App {
    pub fn new(config: Config, path: PathBuf, engine: EngineHandle, events: Receiver<Event>, notice: Option<String>) -> Self {
        App { config, path, engine: Some(engine), events, attached: Vec::new(), selected: None, error: None, notice, dirty: false }
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
                let target_label = known.iter().find(|(id, _)| *id == target).map(|(_, l)| l.clone()).unwrap_or_else(|| "no target".into());
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
                    let m = &mut self.config.active_profile_mut().mappings[i];
                    m.source = if follow {
                        MonitorRef::Primary
                    } else {
                        // Bind to whatever is primary right now.
                        match self.attached.iter().find(|a| a.info.is_primary) {
                            Some(a) => MonitorRef::Id(a.id.clone()),
                            None => MonitorRef::Primary,
                        }
                    };
                    self.dirty = true;
                }
                if status == "active" { ui.label(&status) } else { ui.weak(&status) };
                if ui.button("Select region…").clicked() {
                    select_for = Some(i);
                }
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
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
        ui.weak("Ctrl+Alt+M selects a region for the highlighted mapping (or creates one).");
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
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
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.monitors_panel(ui);
                ui.separator();
                self.mappings_panel(ui);
            });
        });
        if self.dirty {
            self.dirty = false;
            if let Err(e) = self.config.save(&self.path) {
                self.error = Some(format!("saving config failed: {e:#}"));
            }
            self.send(Command::Apply(self.config.clone()));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
    }
}
```

- [ ] **Step 2: Final `src/main.rs`**

```rust
mod args;
mod capture;
mod config;
mod d3d;
mod engine;
mod geometry;
mod identity;
mod monitors;
mod overlay;
mod renderer;
mod ui;
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
    let config = loaded.config;
    let notice = loaded.notice;

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([560.0, 480.0]).with_title("Mirage"),
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
            Ok(Box::new(ui::App::new(config, path, engine, event_rx, notice)))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}
```

- [ ] **Step 3: Trim `src/args.rs`**

Replace `USAGE`, `Args` and `Args::parse` so only `--list` remains; keep the tests that still apply:

```rust
pub const USAGE: &str = "\
usage: mirage [--list]
  --list        print the monitors Windows currently sees, with their identities, and exit";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub list: bool,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(iter: I) -> Result<Args, String> {
        let mut args = Args::default();
        for flag in iter {
            match flag.as_str() {
                "--list" => args.list = true,
                other => return Err(format!("unknown argument '{other}'")),
            }
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        Args::parse(s.split_whitespace().map(String::from))
    }

    #[test]
    fn empty_is_all_defaults() {
        assert_eq!(parse(""), Ok(Args::default()));
    }

    #[test]
    fn parses_list() {
        assert_eq!(parse("--list"), Ok(Args { list: true }));
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse("--bogus").is_err());
        assert!(parse("stray").is_err());
    }
}
```

Delete `src/main.rs`'s `use crate::geometry::Rect;` if it is still there. `monitors::describe`, `choose_source`, `choose_target` are no longer used by the binary: delete them and their tests from `src/monitors.rs` (keep `MonitorInfo` and `enumerate`).

- [ ] **Step 4: Build, test, run**

Run: `cargo build 2>&1 | grep -E "^error|^warning"` → no errors; remaining warnings must be fixed or justified (unused imports: remove).
Run: `cargo test 2>&1 | grep "test result"` → 27 passed (6 geometry + 3 args + 11 config + 7 identity).
Run: `cargo run` → the settings window opens. Expected:
- *Monitors* lists your attached monitors with default labels like `HP 27xq (1FZ)` and `USB_Monitor (820)`; the Winwing row says `attached`; editing a label and restarting keeps the label.
- *Mappings* shows the mapping from Task 5 as `#1 primary [x, y, w×h] → USB_Monitor (820) … active`.
- Pressing `Ctrl+Alt+M` while the game or the desktop is in front, dragging → the row updates and the cockpit screen follows. Note: the settings window does **not** take focus back after selection.
- `Add mapping` → selection starts; after dragging, a second row appears. (Both rows target the same monitor: the later one covers the earlier – tiling is M3; remove the second row again.)
- `Remove` deletes the row and the cockpit screen goes black.
- Closing the window ends the process; the cockpit screen shows the desktop again.

- [ ] **Step 5: Commit**

```powershell
git add src/ui.rs src/main.rs src/args.rs src/monitors.rs
git commit -m @'
feat: egui settings window with monitor labels and mapping list

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
```

---

### Task 7: Acceptance, docs, merge

**Files:**
- Modify: `PLAN.md`, `README.md`

- [ ] **Step 1: Release build and the spec's acceptance list**

Run: `cargo build --release`; use `target\release\mirage.exe`. Work through, with the user:

1. Delete `%APPDATA%\Mirage\config.toml`. Start Mirage: window opens, no mappings. `Ctrl+Alt+M`, drag on the primary → cockpit shows the region, a row with `primary` appears.
2. Close and restart → same region shows without interaction.
3. Unplug the Winwing screen while running → the row says `dormant: target monitor absent`, the Monitors panel shows it `absent`, no crash. Plug it back → mirror resumes, row says `active`.
4. Make another HP the primary in Windows display settings → the mapping follows it (same rect).
5. `Ctrl+Alt+M`, `Esc` → nothing changes. Drag on a non-primary monitor with "follow primary" off → row shows that monitor's label.
6. Start WarDogs; `Ctrl+Alt+M` in-game; drag the mini-map; the game keeps focus afterwards (the overlay took it briefly, the game gets it back because the overlay windows are destroyed).

Record anything that fails as a bug and fix it before continuing.

- [ ] **Step 2: Update `PLAN.md`**

- Under *Milestones*, mark M1 done with the date and add a short *M1 results* section: what was verified, anything surprising (e.g. whether `SetForegroundWindow` on the overlay worked from a game, whether the yellow border stayed away for the temporary selection sessions).
- Under *Technical decisions → UI*, replace the egui-viewport overlay paragraphs with: the overlay is Win32 + D3D11 (the M1 spec explains why); egui is used for the settings window only.
- Under *Architecture*, update the `overlay` and `config` lines to match (`overlay  Win32 windows on the render thread, frozen D3D textures, ps_overlay`).
- *Open questions*: remove the egui-positioning entry (moot).

- [ ] **Step 3: Update `README.md`**

```markdown
# Mirage

Crop any part of your screen and mirror it to another monitor – display only, no game interaction.

## Status

M1: press `Ctrl+Alt+M`, drag a rectangle on any monitor, it shows up full-screen on the target monitor
(by default the smallest one attached). Mappings are saved to `%APPDATA%\Mirage\config.toml` and
survive restarts and monitor hot-plugging.

## Build

Rust stable (MSVC), then `cargo build --release`. The binary is `target\release\mirage.exe`;
`mirage.exe --list` prints the attached monitors with their identities.

See `PLAN.md` for the design and roadmap.
```

- [ ] **Step 4: Commit and merge**

```powershell
git add PLAN.md README.md
git commit -m @'
docs: record M1 results, update README

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
'@
git checkout main
git merge --no-ff m1-region-selection -m "Merge branch m1-region-selection: region selection, settings window, monitor identity"
cargo test
git push origin main
git branch -d m1-region-selection
```

---

## Verification summary

| What | How | Pass criterion |
|---|---|---|
| Config logic | `cargo test config` | 11 green |
| Identity logic | `cargo test identity` | 7 green (real EDID blobs) |
| Identity on hardware | `mirage --list` | `edid:` ids for HPs, `usb:` for Winwing, stable across re-plugs |
| Engine | headless run with hand-written config (Task 4) | mirror up, hot-plug survives |
| Overlay | Task 5 Step 4 | dim/undim/border, Esc, right-click, min size, non-primary |
| UI | Task 6 Step 4 | labels, rows, select/remove, apply-on-change |
| Acceptance | Task 7 Step 1 | all six pass with the user watching |

## Out of scope

Transforms (M2), profiles/tiling UI (M3), tray, hotkey editing, autostart, process watcher (M4). `target_rect` is honoured by the engine already but has no UI until M3.
