use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::geometry::Rect;
use crate::identity::{AttachedMonitor, MonitorId};
use crate::transform::{Filter, Fit, Transform};

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
    pub flip_h: bool,
    pub flip_v: bool,
    /// Clockwise degrees: 0, 90, 180 or 270.
    pub rotation: u16,
    pub fit: Fit,
    pub filter: Filter,
}

impl Default for Mapping {
    fn default() -> Self {
        Mapping {
            source: MonitorRef::Primary,
            source_rect: [0, 0, 0, 0],
            source_size: [0, 0],
            target: String::new(),
            target_rect: Vec::new(),
            flip_h: false,
            flip_v: false,
            rotation: 0,
            fit: Fit::default(),
            filter: Filter::default(),
        }
    }
}

impl Mapping {
    pub fn transform(&self) -> Transform {
        Transform { flip_h: self.flip_h, flip_v: self.flip_v, rotation: self.rotation, fit: self.fit, filter: self.filter }
    }

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
            select_region_hotkey: crate::hotkey::DEFAULT.into(),
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
    pub transform: Transform,
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
                [x, y, w, h] if *w > 0 && *h > 0 => {
                    Rect { x: *x, y: *y, w: *w, h: *h }.clamp_to(target.info.width, target.info.height)
                }
                _ => Rect { x: 0, y: 0, w: target.info.width, h: target.info.height },
            };
            Resolved::Active(ActiveMapping {
                index,
                source_handle: source.info.handle,
                source_size,
                source_rect: m.effective_rect(source_size),
                target_handle: target.info.handle,
                target_rect,
                transform: m.transform(),
            })
        })
        .collect()
}

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
            ..Default::default()
        }
    }

    #[test]
    fn default_config_has_one_empty_profile() {
        let c = Config::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.active_profile, "default");
        assert_eq!(c.profiles.len(), 1);
        assert!(c.profiles[0].mappings.is_empty());
        assert_eq!(c.select_region_hotkey, "Ctrl+Shift+R");
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
    fn transform_fields_round_trip_and_default_to_fit_linear() {
        let mut m = mapping(MonitorRef::Primary, "t");
        assert_eq!(m.transform(), Transform::default());
        assert_eq!(m.fit, Fit::Fit);
        m.flip_h = true;
        m.rotation = 90;
        m.fit = Fit::Fill;
        m.filter = Filter::Nearest;
        let mut c = Config::default();
        c.active_profile_mut().mappings.push(m.clone());
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.active_profile().mappings[0], m);
    }

    #[test]
    fn default_label_uses_the_last_three_chars_of_the_serial() {
        assert_eq!(default_label("HP 27xq", "edid:HPN:3582:CNK04611FZ"), "HP 27xq (1FZ)");
        assert_eq!(default_label("USB_Monitor", "usb:WWIN29320251005160820"), "USB_Monitor (820)");
        assert_eq!(default_label("Generic", "path:DISPLAY\\X\\1&2"), "Generic");
    }
}
