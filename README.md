# Mirage

Crop any part of your screen and mirror it to another monitor – display only, no game interaction.

## Status

M1: press `Ctrl+Shift+R`, drag a rectangle on any monitor, it shows up full-screen on the target
monitor (by default the smallest one attached). Mappings are saved to `%APPDATA%\Mirage\config.toml`
and survive restarts and monitor hot-plugging. The hotkey can be changed via `select_region_hotkey`
in that file.

## Build

Rust stable (MSVC), then `cargo build --release`. The binary is `target\release\mirage.exe`;
`mirage.exe --list` prints the attached monitors with their identities.

See `PLAN.md` for the design and roadmap.
