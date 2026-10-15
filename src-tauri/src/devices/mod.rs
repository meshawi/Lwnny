//! Device layer: one small driver per piece of RGB hardware in this PC.
//!
//! Every driver implements [`Device`]. The engine hands it a [`Render`]
//! (one color per zone plus an effect) and the driver translates that into
//! the vendor's USB HID or I2C protocol. Hardware effects are used wherever
//! the device supports them so lighting keeps running without the app.
//!
//! Protocol knowledge comes from the OpenRGB project (GPL-2.0), re-implemented
//! here in Rust for this machine's exact hardware.

pub mod aura;
pub mod corsair_dram;
pub mod ene_gpu;
pub mod lianli_hub;
pub mod nvapi;
pub mod pawnio;
pub mod quadcast;
pub mod strimer;

use serde::{Deserialize, Serialize};

/// Plain 8-bit RGB color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Parse "#rrggbb" or "rrggbb".
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Multiply every channel by `f` (0.0 ..= 1.0).
    pub fn scaled(self, f: f32) -> Rgb {
        let f = f.clamp(0.0, 1.0);
        Rgb::new(
            (self.r as f32 * f).round() as u8,
            (self.g as f32 * f).round() as u8,
            (self.b as f32 * f).round() as u8,
        )
    }

    /// HSV to RGB, hue in degrees (wraps), s and v in 0..=1.
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Rgb {
        let h = h.rem_euclid(360.0) / 60.0;
        let i = h.floor() as i32;
        let f = h - i as f32;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Rgb::new((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }
}

/// Lighting effect. Static only for now; animated effects will return once
/// every device is covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Fixed color per zone.
    Static,
}

#[derive(Clone, Debug, Serialize)]
pub struct Zone {
    pub name: String,
    pub leds: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    /// Stable identifier used in profiles ("lianli_hub", "gpu", ...).
    pub id: String,
    pub name: String,
    /// Short category for the UI: "fans", "cable", "motherboard", "gpu", "mic".
    pub kind: String,
    pub zones: Vec<Zone>,
    /// True when the device can store the current lighting in its own memory.
    pub supports_save: bool,
    /// Free text shown in the UI (firmware, caveats).
    pub note: String,
}

/// What a device should show right now.
pub struct Render<'a> {
    /// One color per zone, same order as `DeviceInfo::zones`.
    pub colors: &'a [Rgb],
    pub effect: Effect,
    /// 0 ..= 100.
    pub brightness: u8,
}

impl<'a> Render<'a> {
    /// Zone colors with brightness already applied.
    pub fn scaled_colors(&self) -> Vec<Rgb> {
        let f = self.brightness as f32 / 100.0;
        self.colors.iter().map(|c| c.scaled(f)).collect()
    }
}

pub trait Device: Send {
    fn info(&self) -> &DeviceInfo;

    /// Push the render to the hardware.
    fn apply(&mut self, render: &Render) -> anyhow::Result<()>;

    /// Store the last applied lighting in device memory so it survives
    /// the app closing or the PC rebooting. No-op when unsupported.
    fn save(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// True when the device has no hardware effects and the engine must
    /// stream frames to it (and keep it alive) instead.
    fn needs_software_effects(&self) -> bool {
        false
    }

    /// Software path: push one already-computed frame (one color per zone).
    fn write_frame(&mut self, _colors: &[Rgb]) -> anyhow::Result<()> {
        Ok(())
    }

    /// Some devices store every write in flash (RAM sticks). For those the
    /// engine waits until the colors stop changing for this long before it
    /// calls `apply`, so dragging a color picker does not wear the flash.
    fn commit_delay(&self) -> Option<std::time::Duration> {
        None
    }

    /// Diagnostic dump of device state for the probe CLI.
    fn debug_dump(&mut self) -> Option<String> {
        None
    }

    /// Diagnostic: light individual LEDs (index order as the device counts
    /// them). Used to map LED positions. Unsupported devices return Ok.
    fn write_leds(&mut self, _leds: &[Rgb]) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Zero-filled HID output buffer with report id at byte 0.
pub(crate) fn report(size: usize, report_id: u8) -> Vec<u8> {
    let mut v = vec![0u8; size];
    v[0] = report_id;
    v
}
