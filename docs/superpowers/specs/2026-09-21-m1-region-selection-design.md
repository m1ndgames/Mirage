# M1 – Region selection, settings window, monitor identity

Design for milestone M1 of Mirage. Builds on the M0 spike (`src/`, see `PLAN.md` → *M0 results*) and
turns it into an application: the user draws a region with the mouse, it appears on the cockpit screen,
it survives restarts and monitor hot-plugging. Everything here follows the principles in `PLAN.md`
(display only, no process interaction, GPU-only frame path, never steal focus).

## Goal

- Press a hotkey (or click a button) → every monitor freezes → drag a rectangle anywhere → the region is
  mirrored on the cockpit screen. One gesture, no monitor indices, no typing coordinates.
- Configuration is saved automatically and restored on start.
- Monitors are identified by hardware identity, so unplugging, re-plugging and re-arranging never breaks a
  mapping; an absent monitor makes its mapping dormant, never an error.

## Architecture

Two threads, one channel each way.

```
main thread                          render thread
──────────────────────────           ──────────────────────────────────────────────────────
eframe / egui settings window        owns ALL Win32 + D3D11: device, capture sessions,
owns Config, saves it to disk        output windows, overlay windows, hidden message window
                                     (WM_DISPLAYCHANGE / WM_DEVICECHANGE / WM_HOTKEY / WM_TIMER)
        ── Command ─────────────────▶  Apply(Config) | SelectRegion | Shutdown
        ◀───────────────── Event ──── MonitorsChanged(Vec<AttachedMonitor>) | RegionSelected{..}
                                       | SelectionCancelled | Error(String)
```

- Commands travel over an `mpsc` channel plus a Win32 auto-reset event that wakes the render thread's
  `MsgWaitForMultipleObjectsEx` (which already waits on capture frame events and the message queue).
- Events travel over an `mpsc` channel; the render thread holds an `egui::Context` clone and calls
  `request_repaint()` after each send so the UI wakes up.
- The render thread never blocks on the UI. The UI never touches Win32 or D3D.

### Modules

| File | Responsibility | Depends on |
|---|---|---|
| `main.rs` | startup order (DPI → WinRT → spawn render thread → run eframe), shutdown join | all |
| `args.rs`, `geometry.rs` | unchanged from M0 (`--list` stays as a diagnostic) | – |
| `monitors.rs` | `MonitorInfo` (M0) + `enumerate()` | Win32 GDI |
| `identity.rs` | **new** – hardware identity per attached monitor, `AttachedMonitor` | DisplayConfig, Registry, CfgMgr32 |
| `config.rs` | **new** – `Config`, `Profile`, `Mapping`, `MonitorId`, load/save, resolve | serde, toml |
| `d3d.rs` | unchanged | DXGI, D3D11 |
| `capture.rs` | `MonitorCapture` (M0) + `grab_one_frame()` helper for freezing | WGC |
| `renderer.rs` | split into `Pipeline` (shaders, sampler, constant buffers – shared) and `SwapChainTarget` (window-sized swap chain + RTV) | D3D11 |
| `window.rs` | `create_output_window()`, `create_overlay_window()`, `create_message_window()`, message pump | Win32 |
| `overlay.rs` | **new** – selection state machine over N overlay windows | window, renderer |
| `engine.rs` | **new** – the render thread: resolves config against attached monitors, owns sources/outputs, main loop | everything above |
| `ui.rs` | **new** – eframe app: monitors panel, mappings panel, sends commands, applies events | eframe, config |
| `shaders.hlsl` | mirror VS/PS (M0) + `ps_overlay` (dim outside selection, 2 px border) | – |

## Monitor identity (`identity.rs`)

`AttachedMonitor { id: MonitorId, gdi_name: String, friendly_name: String, info: MonitorInfo }` – one
per active monitor, produced by joining three sources on the GDI name (`\\.\DISPLAYn`), which is the
only key `HMONITOR` and DisplayConfig have in common:

1. `EnumDisplayMonitors` / `GetMonitorInfoW` → `HMONITOR`, bounds, primary flag, GDI name (M0 code).
2. `QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS)` + `DisplayConfigGetDeviceInfo`:
   `DISPLAYCONFIG_SOURCE_DEVICE_NAME` → GDI name; `DISPLAYCONFIG_TARGET_DEVICE_NAME` → monitor device
   path (`\\?\DISPLAY#HPN3582#5&2f036cc&1&UID41219#{guid}`), friendly name, EDID manufacturer/product.
3. Device path → PnP instance id (`DISPLAY\HPN3582\5&2f036cc&1&UID41219`: drop prefix and GUID, `#`→`\`)
   → registry `HKLM\SYSTEM\CurrentControlSet\Enum\<instance>\Device Parameters\EDID` (REG_BINARY).

`MonitorId` is a string with a scheme prefix, chosen by the first rule that applies:

| Scheme | Source | Example |
|---|---|---|
| `edid:<mfg>:<product>:<serial>` | EDID descriptor 0xFF serial string; else the 32-bit serial if ≠ 0 | `edid:HPN:3582:CNK04611FZ` |
| `usb:<serial>` | walk `CM_Get_Parent` from the monitor's devnode until an instance id whose last segment contains no `&` (a real serial, not a Windows-generated one); at most 4 levels | `usb:WWIN29320251005160820` |
| `path:<instance>` | last resort, warns in the UI that the monitor is bound to its port | `path:DISPLAY\REG0319\C&2DC29C55&0&UID256` |

EDID parsing (`parse_edid(&[u8]) -> Option<Edid { mfg, product, serial_u32, serial_str }>`) is pure and
unit-tested with real EDID blobs from the dev machine (one HP, the Winwing).

## Configuration (`config.rs`)

`%APPDATA%\Mirage\config.toml`, written atomically (write `config.toml.tmp`, rename). Unreadable file →
renamed to `config.toml.bad`, fresh config, UI shows a notice.

```toml
version = 1
active_profile = "default"
last_target = "usb:WWIN29320251005160820"
select_region_hotkey = "Ctrl+Alt+M"          # stored now, editable in M4

[[monitors]]                                 # every monitor Mirage has ever seen
id = "edid:HPN:3582:CNK04611FZ"
label = "Main"                               # user editable; default = friendly name + serial suffix
last_name = "HP 27xq"
last_size = [2560, 1440]

[[profiles]]
name = "default"
process = ""                                 # M4: auto-activate when this exe is in the foreground

[[profiles.mappings]]
source = "primary"                           # or a MonitorId
source_rect = [0, 1040, 400, 400]            # physical px, relative to the source monitor's top-left
source_size = [2560, 1440]                   # monitor size when the rect was drawn
target = "usb:WWIN29320251005160820"
target_rect = []                             # empty = whole monitor; M3 tiling fills it
# M2 adds: flip_h, flip_v, rotation, fit, filter
```

Rust side: `Config`, `KnownMonitor`, `Profile`, `Mapping`, `MonitorRef { Primary | Id(MonitorId) }`
(serialised as the string `"primary"` or the id). `serde` derives with `#[serde(default)]` everywhere so
older files load after fields are added.

Pure helpers, unit-tested:
- `Config::load(path)` / `save(path)` round-trip; missing file → `Config::default()`.
- `Mapping::effective_rect(current_size) -> Rect` – scales `source_rect` proportionally if the monitor
  runs at a different resolution than `source_size`, then clamps.
- `resolve(profile, attached: &[AttachedMonitor]) -> Vec<ResolvedMapping>` – maps `MonitorRef` to
  `HMONITOR`s; a mapping whose source or target is absent is returned as `Dormant { reason }`, active
  ones carry both handles, the effective source rect and the target rect (whole monitor when empty).
- `Config::note_attached(&mut self, &[AttachedMonitor])` – adds unseen monitors with a default label,
  updates `last_name`/`last_size`; returns whether anything changed (→ save).

## Render thread (`engine.rs`)

State: `D3d`, `Pipeline`, hidden message window, `sources: HashMap<isize /*HMONITOR*/, Source>`,
`outputs: HashMap<isize, Output>`, `attached: Vec<AttachedMonitor>`, `config: Config`,
`selection: Option<overlay::Selection>`.

- `Source { capture: MonitorCapture, texture, srv, size }` – one per distinct source monitor in use.
- `Output { window, swap_chain_target, mappings: Vec<ResolvedMapping> }` – one per distinct target
  monitor; draws each mapping as a quad with viewport = target rect, UV = source rect / source size.

Loop (extends M0):
1. `MsgWaitForMultipleObjectsEx` on `[command_event, source frame events…]`, 1 s timeout.
2. Pump messages (output, overlay and message windows all live on this thread).
3. Drain commands: `Apply(config)` → store, `rebuild()`; `SelectRegion` → `overlay::start()`;
   `Shutdown` → break.
4. For every source whose event fired: drain frames, copy newest into its texture, mark dirty.
5. Every output with a dirty source (or a resize) redraws and presents once.
6. If a selection is running, hand it the pump's result (`overlay` sees its own window messages through
   the window procedure; the loop only polls `selection.poll()` for `Done`/`Cancelled`).

`rebuild()`: `attached = identity::attached()?`; send `MonitorsChanged`; `resolve()`; drop sources /
outputs no longer needed; create missing ones; on any failure send `Error` and keep the rest running.

Topology: the message window handles `WM_DISPLAYCHANGE` and `WM_DEVICECHANGE` by (re)starting a 500 ms
`SetTimer`; `WM_TIMER` → `rebuild()`. The debounce matters: Windows sends several messages per change.
Output windows also get `WM_DISPLAYCHANGE`; they ignore it – the rebuild repositions them.

Hotkey: `RegisterHotKey(message_hwnd, 1, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'M')`; `WM_HOTKEY` →
start selection (same path as the `SelectRegion` command). Registration failure (key taken) → `Error`
event, everything else keeps working.

## Selection overlay (`overlay.rs`)

`overlay::start(engine_ctx, attached) -> Selection`:
1. For every attached monitor, grab one frame: `capture::grab_one_frame(device, hmonitor, timeout 1 s)`
   (temporary WGC session; waits on its frame event; copies into a fresh texture; closes the session).
   A monitor that yields no frame in time gets a black texture – selection still works there.
2. Create one overlay window per monitor: `WS_POPUP`, `WS_EX_TOPMOST`, covering the monitor, crosshair
   cursor, **activatable** (it needs keyboard for Esc and mouse capture) – the only Mirage window that
   ever takes focus, and only while the user is actively dragging in it.
3. Each overlay owns a `SwapChainTarget` and draws its frozen texture with `ps_overlay`: pixels outside
   the current selection multiplied by 0.4, a 2 px border around it, no selection → whole frame dimmed.

Input (window procedure, physical client pixels):
- `WM_LBUTTONDOWN` → `SetCapture`, anchor point; `WM_MOUSEMOVE` while captured → update rect (normalised,
  clamped to the monitor), redraw; `WM_LBUTTONUP` → `ReleaseCapture`; rect ≥ 8×8 → `Done`, else ignore.
- `WM_KEYDOWN` `VK_ESCAPE` or `WM_RBUTTONDOWN` → `Cancelled`.
- Only the overlay that received the button-down tracks the drag; a drag cannot span monitors.

Result: `RegionSelected { monitor: MonitorId, is_primary: bool, rect: Rect, monitor_size: (i32, i32) }`.
All overlay windows are destroyed on `Done`/`Cancelled`; focus returns to whatever had it (Windows does
this when the active window is destroyed).

## Settings window (`ui.rs`)

eframe app, English UI, ~400×500 px, opens on start (tray/minimised start is M4).

- **Monitors**: one row per known monitor – editable label, friendly name, size, status
  `attached` / `absent` (greyed), and `⚠ port-bound` for `path:` ids.
- **Mappings** (of the active profile): selectable rows – source (`primary` or label), rect, target
  (combo box over all known monitors, absent ones greyed), status `active` / `dormant: <reason>`,
  buttons `Select region…` and `Remove`. Below: `Add mapping` (creates a row and starts selection).
  A per-row toggle `follow primary` switches `source` between `primary` and the concrete id.
- Hint line: `Ctrl+Alt+M selects a region for the highlighted mapping.`
- Errors from the render thread appear in a dismissable bar at the top.

Rules:
- Any change → `config.save()` → `Command::Apply(config.clone())`. No save button.
- `RegionSelected` goes to the highlighted mapping. If none: a new mapping is appended with
  `target = config.last_target` if that monitor is attached, else the smallest attached monitor that is
  not the source. `source = primary` if `is_primary`, else the concrete id. `last_target` is updated.
- `MonitorsChanged` → `config.note_attached()`; save if changed; rows update their status.

## Shutdown

Closing the settings window sends `Shutdown`, joins the render thread (which destroys windows, closes
capture sessions) and exits. M4 will change "close" to "hide to tray".

## Testing

Unit tests (pure, no Win32): `parse_edid` with captured blobs, instance-id derivation from a device
path, `MonitorId` choice rules, config round-trip and defaults, `effective_rect` scaling, `resolve`
against fake attached lists (present / absent / primary moved), `note_attached`.

Manual acceptance (with the user at the keyboard, release build):
1. First start: window opens, no mappings. `Ctrl+Alt+M`, drag on the primary → cockpit screen shows the
   region; a mapping row appears with `source = primary`.
2. Restart Mirage → same region shows again without interaction.
3. Unplug the Winwing screen while running → row says `dormant: target absent`, no crash; plug it back
   → mirror resumes by itself.
4. Change the primary monitor in Windows → the mapping follows the new primary (same rect, scaled if the
   resolution differs).
5. Overlay: Esc cancels and leaves everything as it was; a drag on a non-primary monitor creates a
   mapping with that monitor's concrete id.
6. The game keeps focus while Mirage runs; only the overlay takes focus, and only during selection.

## Out of scope (later milestones)

Transforms (M2: `flip_h`, `flip_v`, `rotation`, `fit`, `filter` – the shader gets a matrix), profile
list and switching plus tiling UI (M3), tray icon, hotkey editing, autostart, process-based profile
switching (M4). The data model already carries their fields where that avoids a later migration.

## Dependencies added

`eframe 0.36`, `serde 1` (derive), `toml 1`. `windows` features added: `Win32_Devices_Display`,
`Win32_Devices_DeviceAndDriverInstallation`, `Win32_System_Registry`, `Win32_UI_Input_KeyboardAndMouse`.
