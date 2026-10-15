# Lwnny

One RGB controller for your whole PC. No vendor apps, no fighting over who
owns the lights.

Lwnny talks to each piece of hardware directly over USB HID, the GPU's I2C
bus or the chipset SMBus, shows every zone in one window, and keeps running
in the tray.

## Supported hardware

| Device | Interface | Saves to device |
| --- | --- | --- |
| Lian Li UNI HUB SL-Infinity (SL-Infinity fans) | USB HID `0CF2:A102` | held while powered |
| Lian Li Strimer Plus (24-pin and GPU cables) | USB HID `0CF2:A200` | no |
| ASUS Aura motherboards (X570 generation, e.g. ROG Crosshair VIII Hero) incl. both addressable headers | USB HID `0B05:18F3` | yes |
| HyperX QuadCast S | USB HID `0951:171F` | yes |
| ASUS ROG Strix GeForce RTX 30-series (ENE controller) | NvAPI I2C `0x67` | yes |
| Corsair Vengeance RGB Pro / Dominator / Vengeance RGB DDR5 memory | SMBus via PawnIO, `0x58`-`0x5F` | yes (stored in each stick) |

Not controlled on purpose: NZXT Kraken coolers (leave those to NZXT CAM so
pump and fan curves stay where they are).

### RAM, PawnIO and administrator rights

Memory sticks sit on the chipset's SMBus, which only a kernel driver can
reach. Lwnny uses [PawnIO](https://pawnio.eu) (GPL-2.0-or-later, signed by
Microsoft) and its `SmbusPIIX4` module, bundled in `src-tauri/pawnio/`.
Install PawnIO once; Lwnny loads the module itself. Because the driver
requires administrator rights, Lwnny asks for elevation when it starts. If
you decline, everything except the RAM still works.

The SMBus is shared with monitoring tools, so Lwnny takes the system-wide
SMBus mutex for every transfer and never polls the bus. The sticks store
every write in flash, so Lwnny waits until a color has stopped changing for
0.7 s and then writes once, in the background, skipping sticks that already
show that color. A full write of four sticks takes about 1.2 s, most of it
the sticks' own commit time. The colors survive reboots without Lwnny
running. Intel chipsets (`SmbusI801` module) are not wired up yet.

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
- Tray icon, close-to-tray, optional start with Windows (a scheduled task,
  because an elevated program cannot start from the registry Run key).
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
cargo run --bin probe -- rambench             # SMBus speed per wait mode (admin)
cargo test                                    # unit tests (no hardware needed)
```

The RAM only shows up when the probe runs as administrator;
`tools\probe-ram.cmd` and `tools\test-ram.cmd` do that for you.

Replace the icon: put a square PNG at `assets/logo.png` and run `npm run icon`.

## Layout

```
src/                 front end (plain HTML, CSS, JS; no framework)
src-tauri/src/
  lib.rs             Tauri commands, tray, window
  elevate.rs         administrator check, UAC relaunch, scheduled-task autostart
  engine.rs          scene, settings, profiles, effect ticker
  devices/           one driver per device, all implementing `Device`
  devices/pawnio.rs  PawnIO driver client and PIIX4 SMBus transfers
  bin/probe.rs       command-line diagnostics
src-tauri/pawnio/    signed PawnIO SMBus module (LGPL-2.1) and its license
assets/logo.png      source icon (npm run icon regenerates the sizes)
```

`hardware.json` (the machine this was built on, with serial numbers) is
git-ignored on purpose.

Settings, the last scene and profiles live in `%APPDATA%\Lwnny`.

## Credits

Device protocols were learned from the [OpenRGB](https://openrgb.org) project
(GPL-2.0-or-later) and re-implemented in Rust. SMBus access goes through
[PawnIO](https://pawnio.eu) by namazso (driver GPL-2.0-or-later with an IOCTL
exception, modules LGPL-2.1). Lwnny is released under GPL-2.0-or-later.
