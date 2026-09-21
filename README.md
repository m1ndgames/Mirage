# Mirage

Crop any part of your screen and mirror it to another monitor – display only, no game interaction.

## What it does

Press `Ctrl+Shift+R`, drag a rectangle on any monitor, and it shows up full-screen on the target
monitor (by default the smallest one attached). Mappings are saved to `%APPDATA%\Mirage\config.toml`
and survive restarts and monitor hot-plugging. Each mapping can be flipped, rotated (90° steps),
fitted (letterbox / fill / stretch) and filtered (linear / nearest) from the settings window, and
placed on a part of the target (halves, quarters or a custom area) so several regions share one
screen. Mappings are grouped into profiles. The hotkey is editable in the settings window.

Closing the window keeps Mirage in the tray (menu: Settings / Select region / Quit). `--minimized`
or the "Start minimized" option starts it hidden; "Start with Windows" adds an autostart entry.

Mirage reads the screen through Windows.Graphics.Capture – the same mechanism OBS and the Xbox Game
Bar use. It never touches a game process and never generates input.

## Download

Prebuilt binaries are on the [Releases](https://github.com/m1ndgames/Mirage/releases) page
(`mirage-vX.Y.Z-windows-x86_64.zip` with a SHA-256 next to it). Windows 11, x86_64.

## Build

Rust stable (MSVC), then `cargo build --release`. The binary is `target\release\mirage.exe`;
`mirage.exe --list` prints the attached monitors with their identities.

Releases are built by GitHub Actions from a `v*` tag whose version matches `Cargo.toml`.

See `PLAN.md` for the design and roadmap.
