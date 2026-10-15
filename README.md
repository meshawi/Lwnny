# Lwnny

One RGB controller for your whole PC. No vendor apps, no kernel drivers, no
fighting over who owns the lights.

Lwnny talks to each piece of hardware directly over USB HID or the GPU's I2C
bus, shows every zone in one window, and keeps running in the tray.

## Supported hardware

| Device | Interface | Saves to device |
| --- | --- | --- |
| Lian Li UNI HUB SL-Infinity (SL-Infinity fans) | USB HID `0CF2:A102` | held while powered |
| Lian Li Strimer Plus (24-pin and GPU cables) | USB HID `0CF2:A200` | no |
| ASUS Aura motherboards (X570 generation, e.g. ROG Crosshair VIII Hero) incl. both addressable headers | USB HID `0B05:18F3` | yes |
| HyperX QuadCast S | USB HID `0951:171F` | yes |
| ASUS ROG Strix GeForce RTX 30-series (ENE controller) | NvAPI I2C `0x67` | yes |

Not controlled on purpose: NZXT Kraken coolers (leave those to NZXT CAM so
pump and fan curves stay where they are) and DRAM RGB (needs a kernel
driver).

## Features

- Per-zone colors, whole-device color, "Everything" color with quick swatches.
- Static lighting (animated effects are planned once every device is covered).
- Brightness.
- Rename any zone and flash it with **Find** to see where it is in the case.
- Profiles: snapshot everything and reload it with one click.
- **Save to hardware**: write the current colors into the devices' own memory
  so they come up right at boot, before Lwnny starts.
- **Own color** per device: that device ignores "Everything" and keeps its
  own colors.
- **Color correction / Calibrate**: per-device channel order and RGB gains with
  a guided wizard, for hardware whose LEDs are off from the factory.
- Tray icon, close-to-tray, optional start with Windows.
- Last lighting is restored automatically on launch.

## Build

Requirements: Rust (stable, MSVC toolchain), Node.js, Microsoft Visual C++
Build Tools, WebView2 (ships with Windows 11).

```powershell
npm install
npm run dev          # run with hot reload
npm run build        # produce an installer in src-tauri/target/release/bundle
```

Diagnostics without the UI:

```powershell
cd src-tauri
cargo run --bin probe -- list                 # detect devices and print zones
cargo run --bin probe -- set all ff00ff       # everything purple
cargo run --bin probe -- hubmap               # a different color on each hub channel
cargo run --bin probe -- bench                # time one frame per device
```

Replace the icon: put a square PNG at `assets/logo.png` and run `npm run icon`.

## Layout

```
src/                 front end (plain HTML, CSS, JS; no framework)
src-tauri/src/
  lib.rs             Tauri commands, tray, window
  engine.rs          scene, settings, profiles, effect ticker
  devices/           one driver per device, all implementing `Device`
  bin/probe.rs       command-line diagnostics
assets/logo.png      source icon (npm run icon regenerates the sizes)
```

`hardware.json` (the machine this was built on, with serial numbers) is
git-ignored on purpose.

Settings, the last scene and profiles live in `%APPDATA%\Lwnny`.

## Credits

Device protocols were learned from the [OpenRGB](https://openrgb.org) project
(GPL-2.0-or-later) and re-implemented in Rust. Lwnny is released under the
same license, GPL-2.0-or-later.
