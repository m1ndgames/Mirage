# Mirage – Project Plan

## Motivation

I'm a sim enthusiast (flight and racing sims) and use a small secondary screen in my cockpit
(https://eu.winctrl.com/view/goods-details.html?id=422). It is a **Winwing DisplayLink USB display**
("WINWING USB 3.0 Display1", DisplayLink VID `17E9`), native 1024×768 @ 60 Hz, rotated to 768×1024
portrait in Windows. From an application's point of view it is a regular monitor in the desktop –
windows can be moved onto it like on any other display. See "DisplayLink" under Technical decisions
for what that means for rendering.

Very few games can natively render a viewport onto such a screen – DCS is the only one that comes to mind.
And it's not just sims: other games have UI elements worth mirroring, e.g. the mini-map in WarDogs.

Goal: a small, native Windows tool that lets me draw a rectangular region on one monitor with the mouse
and shows that region full-screen on another monitor, optionally mirrored / rotated / cropped / stretched.

Why not OBS: its "Fullscreen Projector" can approximate this, but OBS is heavyweight, adds latency, and it
is a workaround. Mirage is meant to be a purpose-built, lightweight, native app that does exactly this one thing.

## Core principles

1. **Read-only consumer of the desktop compositor – display only.** Mirage uses the same official capture
   mechanism as OBS, Discord and the Xbox Game Bar. It never injects into, hooks, or reads memory of a game
   process, and it never generates input of any kind (keyboard, mouse, touch – not even at OS level).
   Nothing about this tool should ever look like cheating: we capture a screen region and display it
   unmodified (apart from geometric transforms). Pixels in, pixels out, nothing else.
2. **No kernel-mode code, no custom drivers.** A custom display driver (IddCx) would be the *worst* option
   with regard to anti-cheat (kernel code is exactly what Vanguard/EAC/Elytra scrutinise), would need
   EV code signing plus Microsoft attestation signing, and is simply unnecessary.
3. **Respect capture opt-outs.** If a game excludes itself from capture
   (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`), Mirage shows black. Circumventing that is
   explicitly out of scope.
4. **GPU-only frame path.** Captured texture → crop/transform → present. No CPU round-trip.
5. **Never steal focus from the game.**

## Target games (test matrix)

| Game | Anti-cheat | Use case |
|------|-----------|----------|
| DCS World | none | MFDs, instruments (has native viewport export – Mirage should be simpler) |
| MSFS | none | instruments, G1000 panels |
| iRacing / ACC | iRacing: own, user-mode; ACC: none | relative/timing boxes |
| **WarDogs** | **Elytra, kernel-level** (was EAC during beta) | mini-map |

WarDogs is the primary anti-cheat test case. Kernel-level anti-cheats are the reason for principles 1–3;
they do not block user-mode compositor capture (OBS works), but any process interaction would be fatal.

## Technical decisions

- **Language: Rust.** `windows` crate for Win32, Direct3D 11 and WinRT (Windows.Graphics.Capture).
- **Capture: Windows.Graphics.Capture (WGC), primary.**
  - Monitor capture via `IGraphicsCaptureItemInterop::CreateForMonitor`.
  - Disable the yellow capture border: `GraphicsCaptureAccess::RequestAccessAsync(Borderless)` then
    `session.IsBorderRequired = false` (Windows 11).
  - Exclude the cursor: `session.IsCursorCaptureEnabled = false`.
  - Evaluate the `windows-capture` crate vs. calling WinRT directly.
  - **Fallback candidate: DXGI Desktop Duplication.** Never draws a border, older and simpler API, but
    needs re-initialisation on desktop switches (UAC, lock screen) and has driver-specific quirks.
    Decide after M0 measurements.
- **Rendering: Direct3D 11.** One full-screen quad per mapping; a transform matrix in the vertex shader
  handles crop, mirror, rotation and fit mode in a single pass. Flip-model swap chain
  (`DXGI_SWAP_EFFECT_FLIP_DISCARD`), vsync to the target monitor. Borderless window – **not** exclusive
  full-screen.
- **Output window:** raw Win32 via `windows` crate. Borderless, sized to the target monitor's bounds,
  `WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` (always on top, never takes focus, hidden from Alt-Tab).
- **DisplayLink target.** The cockpit screen sits behind an Indirect Display Driver: there is no GPU behind
  it. DWM composes on the real GPU, the DisplayLink driver reads frames back, compresses them and pushes
  them over USB 3.0. Consequences:
  - Create the D3D11 device on the **hardware GPU** (the adapter that owns the source monitor), never on the
    DisplayLink adapter that DXGI lists alongside it. A windowed flip-model swap chain on the hardware device
    for a window positioned on the DisplayLink monitor is the normal case; DWM handles the hand-over.
  - Exclusive full-screen is impossible there anyway – another reason for the borderless approach.
  - DisplayLink adds its own latency (read-back + compression + USB), outside our control. Mirage's own
    pipeline budget is 1–2 frames; total latency gets measured in M0.
  - Its refresh rate is 60 Hz regardless of the source monitor.
- **UI: egui / eframe** for the settings window *and* the region-selection overlay.
  - Pure Rust, actively maintained, fastest to iterate; the settings UI is small (profiles, mappings,
    monitor pickers, transform toggles) and doesn't justify anything heavier.
  - The overlay is a second egui viewport: borderless, always-on-top, positioned on the source monitor,
    showing one frozen frame as an egui texture (a single CPU read-back – not latency relevant) with a
    rubber-band rectangle on top. Convert egui points → physical pixels via `pixels_per_point`.
  - If egui's multi-viewport positioning proves unreliable on mixed-DPI setups, the overlay falls back to
    a raw Win32 window; the settings window stays egui.
  - Considered and rejected: raw Win32 controls (laborious), `native-windows-gui` (unmaintained),
    Slint / iced (heavier, no benefit at this size), WinUI 3 from Rust (painful), Tauri (webview).
- **Config:** TOML file in `%APPDATA%\Mirage\`.
- **Platform:** Windows 11 (primary). Windows 10 22H2 best effort (the WGC border cannot be disabled there).

## Architecture

```
capture   WGC session per source monitor → ID3D11Texture2D per frame (FrameArrived callback thread)
output    one Win32 window + D3D11 swap chain per target monitor; draws all mappings for that monitor
overlay   egui viewport: frozen frame + drag-select → returns a source rect in physical pixels
config    profiles / mappings, TOML load & save, monitor identity by device path
app       eframe settings window, tray icon, global hotkeys, foreground-process watcher for profile switching
```

Capture and output share one D3D11 device (same GPU). Config changes flow from the UI to the render side
via a channel; the render loop never blocks on the UI.

## Data model

A **Profile** is a list of **Mappings** plus an optional process name for auto-activation.

Mapping:
- `source`: monitor id + rect (physical pixels)
- `target`: monitor id + rect (default: whole monitor). A sub-rect allows several mappings to be tiled on
  one small screen (e.g. left and right MFD side by side).
- `transform`: `flip_h`, `flip_v`, `rotation` (0/90/180/270), `fit` (`stretch` / `fit` = letterbox /
  `fill` = crop to aspect), `filter` (`linear` / `nearest`)

One output window per target monitor; every mapping that targets it is drawn as a quad into that window.
Rects are stored relative to the monitor's own top-left corner in physical pixels – **never** in virtual
desktop coordinates, because monitor positions shift every time the topology changes. Each rect also
records the monitor resolution it was drawn at; if the monitor later runs at a different resolution, the
rect is scaled proportionally and the UI flags it.

## Monitor identity

The set of connected monitors changes constantly (three identical HP 27xq, a TV and the cockpit screen,
some of them shared with a second PC). Windows' own names (`\\.\DISPLAY8`) and the order of enumeration
are meaningless across sessions. Mirage therefore never stores a display number or index.

**Identification chain**, first match wins:
1. **EDID manufacturer + product code + serial number.** Verified on the dev machine: the three HP 27xq
   report distinct serials (`CNK04611FZ`, `CNK8360L1B`, `CNK04611NG`), the Samsung too.
2. **Parent device serial**, if the EDID serial is empty or a placeholder such as `0`. The Winwing screen
   reports EDID serial `0`, but its DisplayLink USB device carries a real serial (`WWIN2932…`) in its
   instance path, one `CM_Get_Parent` step up from the monitor device.
3. **Connector device path** as a last resort (`DISPLAY\…\UIDnnnn`) – identifies the port, not the panel.
   The UI warns that this monitor will not be recognised if it is plugged in elsewhere.

**Symbolic references.** A mapping's `source` may be `primary` instead of a concrete id – "whatever the
main Windows screen currently is". That is where the game runs, and which physical monitor plays that role
changes with the setup. `target` is normally a concrete id (the cockpit screen).

**User labels.** Every monitor Mirage has ever seen gets a user-editable label ("Winwing", "Main",
"Left HP") stored with its id, because three entries called "HP 27xq" are useless in a picker. Default
label: name + serial suffix.

**Runtime resolution.** On start and on every `WM_DISPLAYCHANGE` / `WM_DEVICECHANGE`, re-enumerate via
`QueryDisplayConfig` (target device path → EDID / parent device; source name → `\\.\DISPLAYn` →
`HMONITOR`) and re-resolve all mappings:
- monitor present → output window (re)positioned on its current bounds, mapping active
- monitor absent → mapping dormant, greyed out in the UI, **not an error**; resumes automatically when the
  monitor reappears
- Nothing about a missing or moved monitor may ever crash Mirage or invalidate a profile.

## Features

### MVP
- Select the region by dragging with the mouse, screenshot-tool style
- Show the region full-screen on a chosen monitor
- Mirror / rotate / stretch / fit / fill
- Settings window

### Beyond MVP
- Profiles: several mappings per profile, one profile per game
- Auto-activate a profile when a given process (e.g. `DCS.exe`, `WarDogs.exe`) is in the foreground
- Global hotkeys: re-select region, toggle output, switch profile
- Tray icon, start minimised, autostart with Windows
- Numeric entry of region coordinates (for games where the overlay cannot be shown, see below)

### Ideas for later
- Per-mapping frame-rate limit to reduce GPU load

## Things to handle (easy to forget)

- **Region selection while the game is running:** freeze one captured frame, show it in a topmost overlay
  on the source monitor and drag-select on that. Exclusive-full-screen games may minimise when the overlay
  appears → select once while the game is in borderless mode and save the profile; numeric entry as a
  fallback. Most modern games use borderless / flip-model presentation anyway.
- **Upscaling small regions:** a 250×250 px mini-map blown up to a whole screen looks very different with
  bilinear vs. nearest-neighbour filtering. Hence `filter` on the mapping; default `linear`.
- **DPI:** Per-Monitor-DPI-v2 aware; work in physical pixels everywhere. Mixed-DPI setups are the norm here.
- **Mixed refresh rates:** capture at the source rate, present at the target rate; unsynchronised is fine
  (latest frame wins).
- **HDR source monitor:** WGC delivers FP16 for HDR outputs; the small screen is SDR → tone-map or force an
  SDR capture format.
- **Multiple DXGI adapters:** the DisplayLink IDD shows up as its own adapter, and so does any virtual
  display driver (e.g. the "Virtual Desktop Monitor" driver is installed on the dev machine). Adapter
  selection must be explicit: pick the hardware GPU that owns the source monitor. If source and target
  ever end up on two *real* GPUs, the texture would need a cross-adapter copy (= CPU staging) – document
  "put both on the same GPU" rather than supporting that.
- **Monitor topology changes** (hot-plug, resolution change, sleep/wake, monitors moved to the other PC):
  re-enumerate, re-resolve mappings as described under *Monitor identity*, re-create capture session and
  swap chains. This is the normal operating condition on the dev machine, not an edge case.
- **Desktop switch** (UAC prompt, lock screen): capture stops; recover automatically.
- **Latency budget:** 1–2 frames for Mirage's own pipeline; DisplayLink adds an unknown amount on top.
  Measure the total in M0 (e.g. a millisecond counter captured from the source monitor, photographed
  next to the cockpit screen).
- **GPU cost on the game:** one copy plus one quad per frame – negligible, but measure.
- **Code signing the executable:** not required, nice-to-have to avoid SmartScreen warnings.

## Dev machine (as of 2026-09-21)

- GPU: NVIDIA GeForce RTX 5070 Ti (driver 32.0.16.1088)
- Monitors in rotation: 3× HP 27xq 2560×1440 (EDID serials `CNK04611FZ`, `CNK8360L1B`, `CNK04611NG`;
  some are shared with a second PC and move back and forth), a Samsung QCQ90S TV, the Winwing cockpit
  screen (EDID serial `0`, DisplayLink USB serial `WWIN2932…`, DisplayLink driver 11.5.6380.0), plus a
  "Virtual Desktop Monitor" virtual display driver. Which of these form the active desktop changes often;
  at the time of writing it was the three HPs side by side with the cockpit screen below the primary.

## Milestones

- **M0 – Spike (validates all the risk):** Rust + WGC capture of monitor A, hard-coded crop rect,
  full-screen present on monitor B. No UI, no config. Any second monitor works for development; the
  WinCtrl screen is only needed for the final check.
  Success criteria: runs alongside WarDogs (Elytra) and DCS/MSFS without anti-cheat complaints, latency
  feels fine, GPU overhead is negligible, works on the WinCtrl screen.
- **M1 – Region selection:** egui settings window + overlay on a frozen frame, choose source and target
  monitor. Includes monitor enumeration with stable identity and hot-plug handling from the start – the
  picker must survive unplugging the cockpit screen while Mirage is running.
- **M2 – Transforms:** mirror, rotate, fit modes, filter.
- **M3 – Profiles:** config file, several mappings per profile, tiling on one target.
- **M4 – Polish:** tray icon, hotkeys, autostart, process-based profile switching.

## Open questions

- **WGC vs. Desktop Duplication:** decide after M0 (latency, border, robustness).
- **egui overlay positioning on mixed-DPI setups:** verify in M1; fall back to raw Win32 if needed.

## Non-goals

- Any interaction with the game process (injection, hooks, memory reads)
- **Any form of input generation** – no touch passthrough, no `SendInput`, no macros. Mirage displays
  pixels, it never acts on the user's behalf.
- Custom display drivers
- Bypassing capture protection a game has opted into
- Game-specific viewport export (e.g. DCS `MonitorSetup` lua) – Mirage is game-agnostic
- Cross-platform support
