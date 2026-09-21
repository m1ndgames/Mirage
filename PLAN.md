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
  - WinRT is called directly through the `windows` crate (decided in M0 – the capture code is ~100
    lines and needs to share Mirage's own D3D11 device, which wrapper crates don't allow).
  - Desktop Duplication is **not** needed: WGC delivered the border-free, cursor-free, 120 Hz capture
    M0 asked for (see *M0 results*). Revisit only if a specific game breaks WGC.
- **Rendering: Direct3D 11.** One full-screen quad per mapping; a transform matrix in the vertex shader
  handles crop, mirror, rotation and fit mode in a single pass. Flip-model swap chain
  (`DXGI_SWAP_EFFECT_FLIP_DISCARD`) on a borderless window – **not** exclusive full-screen.
- **Pacing is event driven, never vsync driven.** The render thread sleeps on the WGC `FrameArrived`
  event (`MsgWaitForMultipleObjectsEx` together with the window's message queue) and presents exactly
  once per newest captured frame. M0 showed that neither `Present(1)` nor the frame-latency waitable
  block on the dev machine – in any window placement, on any monitor – so a loop that relies on them
  spins at ~10 000 iterations/s. Present rate therefore equals the source's change rate, and an idle
  desktop costs nothing.
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
    pipeline is one copy and one draw per captured frame; the M0 spike showed no perceptible end-to-end
    latency on the cockpit screen.
  - Its refresh rate is 60 Hz regardless of the source monitor.
- **UI: egui / eframe** for the settings window only.
  - Pure Rust, actively maintained, fastest to iterate; the settings UI is small (profiles, mappings,
    monitor pickers, transform toggles) and doesn't justify anything heavier.
  - Considered and rejected: raw Win32 controls (laborious), `native-windows-gui` (unmaintained),
    Slint / iced (heavier, no benefit at this size), WinUI 3 from Rust (painful), Tauri (webview).
- **Selection overlay: Win32 + D3D11 on the render thread** (decided in M1, see
  `docs/superpowers/specs/2026-09-21-m1-region-selection-design.md`). One topmost window per monitor
  shows that monitor's frozen frame straight from a capture texture – no CPU read-back – with the
  dim/border effect as a pixel shader. Mouse input arrives in physical pixels, so mixed DPI is a
  non-issue. These are the only Mirage windows that ever take focus, and only while selecting.
- **Config:** TOML file in `%APPDATA%\Mirage\`.
- **Platform:** Windows 11 (primary). Windows 10 22H2 best effort (the WGC border cannot be disabled there).

## Architecture

```
capture   WGC session per source monitor → ID3D11Texture2D per frame (FrameArrived callback thread)
output    one Win32 window + D3D11 swap chain per target monitor; draws all mappings for that monitor
overlay   Win32 windows on the render thread, one per monitor, frozen D3D textures + ps_overlay
identity  EDID / USB-serial / connector-path ids joined to HMONITORs via DisplayConfig
config    profiles / mappings, TOML load & save, resolve() against the attached monitors
engine    the render thread: commands in, events out, sources/outputs, topology rebuild, hotkey
ui        eframe settings window; tray icon and menu live on the render thread's message window
```

Capture and output share one D3D11 device (same GPU). Config changes flow from the UI to the render side
via a channel; the render loop never blocks on the UI.

## Data model

A **Profile** is a named list of **Mappings**; the active one is switched by hand.

Mapping:
- `source`: monitor id + rect (physical pixels)
- `target`: monitor id + `target_area` as fractions `[x, y, w, h]` of the monitor (empty = whole). A
  sub-area allows several mappings to be tiled on one small screen (e.g. left and right MFD side by side).
- `flip_h`, `flip_v`, `rotation` (0/90/180/270, clockwise), `fit` (`stretch` / `fit` = letterbox /
  `fill` = crop to aspect, default `fit`), `filter` (`linear` / `nearest`, default `linear`)

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
- Global hotkeys: re-select region, toggle output, switch profile
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
  feels fine, GPU overhead is negligible, works on the WinCtrl screen. **Done 2026-09-21, see below.**
- **M1 – Region selection:** settings window + overlay on a frozen frame, choose source and target
  monitor. Includes monitor enumeration with stable identity and hot-plug handling from the start – the
  picker must survive unplugging the cockpit screen while Mirage is running. **Done 2026-09-21, see below.**
- **M2 – Transforms:** mirror, rotate, fit modes, filter. **Done 2026-09-21.** One 2×3 UV matrix per
  mapping (crop, flips, rotation and fill-crop folded in, `transform::layout`, 11 unit tests); `fit`
  shrinks the viewport, `fill` crops the source; two sampler states for linear/nearest. Default is
  `fit` (letterbox) – the user's choice. UI: a second row per mapping.
- **M3 – Profiles:** config file, several mappings per profile, tiling on one target. **Done 2026-09-21.**
  Profile bar (switch / new / rename / delete), `target_area` as fractions of the target monitor with
  tiling presets (halves, quarters) and a custom percent entry. Fractions instead of pixels so a tile
  survives a resolution change of the target.
- **M4 – Polish:** tray icon, hotkeys, autostart. **Done 2026-09-21.** Close-to-tray with a tray menu
  (Settings / Select region / Quit), `--minimized` + "Start minimized" option, hotkey editable in the
  window, "Start with Windows" via the HKCU Run key, no console window (`windows_subsystem`; `--list`
  attaches to the parent console), fatal errors as a message box. **Process-based profile switching was
  dropped by the user's decision** – even a handle-free foreground-process lookup is more than "display
  only" needs; profiles are switched manually.

### M0 results (2026-09-21)

Spike lives in `src/` (`cargo run -- --help` style usage: `--list`, `--source N`, `--target N`,
`--rect X,Y,W,H`). Release binary 276 KB, no dependencies beyond Windows.

- **Anti-cheat: WarDogs (Elytra, kernel-level) – pass.** 10+ minutes on an official server in both
  orders (Mirage before the game, Mirage started mid-session with `--rect 0,1040,400,400` on the
  mini-map). No warning, no kick, no launch refusal; the game kept keyboard and mouse throughout.
  DCS/MSFS skipped by decision – they are far less restrictive than Elytra.
- **Latency:** not measurable by eye on the cockpit screen; no photo measurement was needed.
- **GPU cost:** no observable difference in Task Manager with the game running.
- **Capture:** WGC monitor capture at the source's full 120 Hz, no cursor, and
  `GraphicsCaptureAccess::RequestAccessAsync(Borderless)` returned `Allowed` – no yellow border on
  Windows 11 for an unpackaged app. Source resolution changes (2560×1440 → 1920×1080 → back) are
  handled by recreating the frame pool; verified live.
- **DXGI adapters seen:** the RTX 5070 Ti appears **twice** – adapter 0 with 4 outputs (all three HPs
  *and* the DisplayLink screen), adapter 1 with 0 outputs – plus the software "Microsoft Basic Render
  Driver". There is no separately named DisplayLink adapter; the IDD attaches its monitor as an output
  of the real GPU. "Owner of the source monitor" selection picked adapter 0 correctly. The adapter
  enumeration must keep skipping zero-output duplicates.
- **Present does not pace.** `Present(1)` returned `S_OK` instantly at ~12 000 calls/s, the
  frame-latency waitable was permanently signalled, and a 1-px-short window (forcing DWM composition)
  behaved the same, on the DisplayLink screen *and* on an HP monitor. Root cause not pinned down (VRR
  primary at 120 Hz and/or driver vsync override are the suspects) – irrelevant, because the fix is
  the event-driven loop described under *Technical decisions*, which is the better design anyway.
- **`\\.\DISPLAYn` names are not stable even within a session:** enabling two monitors while Mirage
  was running swapped the GDI names of the primary and a secondary HP. Confirms the *Monitor identity*
  plan; nothing may ever key on those names.
- **Crop verified pixel-exact** by screenshotting the source quarter and the cockpit screen and
  comparing them. The 16:9 → 3:4 squash makes a correct crop *look* shifted – fit modes (M2) will
  fix the perception.

### M1 results (2026-09-21)

Acceptance (release build, fresh config, user at the keyboard): first start → `Ctrl+Shift+R` → drag →
mirrored, row with `primary`; restart restores it; unplug/re-plug of the cockpit screen → row goes
`dormant: target monitor absent` and back to `active`, Monitors table flips `absent`/`attached`;
switching the Windows primary to another HP → the mapping follows; in WarDogs, `Ctrl+Shift+R` over the
running game, drag the mini-map, the game keeps focus afterwards. All passed.

- **Identity works as designed:** the three HP 27xq get `edid:HPN:3582:<serial>` ids, the Winwing screen
  `usb:WWIN29320251005160820` (its EDID serial string is `"1"` – a placeholder; the "weak serial" rule
  catches short or single-character-repeated strings, not just `0`). Nothing is port-bound.
- **Message-only windows do not receive `WM_DISPLAYCHANGE`.** The first hot-plug test only *looked*
  successful because Windows 11 restores window positions per monitor configuration; the log showed no
  rebuild. The topology window is now a hidden top-level window and the rebuild is visible in the log.
- **`Ctrl+Alt+M` and `Ctrl+Alt+R` are taken** by another program on the dev machine (RegisterHotKey →
  `ERROR_HOTKEY_ALREADY_REGISTERED`). Default is `Ctrl+Shift+R`; the hotkey string is parsed from
  `select_region_hotkey` in config.toml (M4 adds UI for it) and re-registered on change.
- **DXGI adapter list** unchanged from M0 (RTX listed twice, DisplayLink screen is an output of the real
  GPU); the "owner of the primary monitor" rule keeps picking the right one.
- **Focus after selection:** destroying the overlay windows hands focus back to the previously active
  window (the game) without any extra work.
- Release binary is ~15 MB because eframe ships its own OpenGL renderer; the M0 spike was 276 KB.

## Open questions

- None at the moment. M2 questions (fit-mode default, filter default) get asked when M2 starts.

## Non-goals

- Any interaction with the game process (injection, hooks, memory reads)
- **Any form of input generation** – no touch passthrough, no `SendInput`, no macros. Mirage displays
  pixels, it never acts on the user's behalf.
- Custom display drivers
- Bypassing capture protection a game has opted into
- Game-specific viewport export (e.g. DCS `MonitorSetup` lua) – Mirage is game-agnostic
- Cross-platform support
