//! Lian Li Strimer Plus controller (USB 0CF2:A200), "Strimer L Connect".
//!
//! One controller drives the 24-pin cable (6 strips x 20 LEDs) and the
//! triple 8-pin GPU cable (6 strips x 27 LEDs) = 12 zones.

use super::{report, Device, DeviceInfo, Render, Rgb, Zone};
use anyhow::{Context, Result};
use hidapi::{HidApi, HidDevice};
use std::{thread, time::Duration};

pub const VID: u16 = 0x0CF2;
pub const PID: u16 = 0xA200;
pub const ID: &str = "strimer";

const PACKET: usize = 255;
const REPORT_ID: u8 = 0xE0;
const CMD_MODE: u8 = 0x10;
const CMD_COLOR: u8 = 0x30;

const MODE_DIRECT: u8 = 0x01;
const SPEED_NORMAL: u8 = 0x00;
const BRIGHTNESS_FULL: u8 = 0x00;

pub struct Strimer {
    dev: HidDevice,
    info: DeviceInfo,
    /// All zones are already in direct mode: frames only need color packets.
    direct: bool,
}

impl Strimer {
    pub fn open(api: &HidApi) -> Result<Option<Self>> {
        let Some(di) = api.device_list().find(|d| {
            d.vendor_id() == VID && d.product_id() == PID && d.usage_page() == 0xFF72 && d.usage() == 0xA1
        }) else {
            return Ok(None);
        };
        let dev = di.open_device(api).context("open Strimer")?;
        let product = dev.get_product_string().ok().flatten().unwrap_or_default();

        let mut zones = Vec::new();
        for i in 0..6 {
            zones.push(Zone { name: format!("24-pin strip {}", i + 1), leds: 20 });
        }
        for i in 0..6 {
            zones.push(Zone { name: format!("GPU 8-pin strip {}", i + 1), leds: 27 });
        }

        Ok(Some(Strimer {
            dev,
            info: DeviceInfo {
                id: ID.into(),
                name: "Lian Li Strimer Plus".into(),
                kind: "cable".into(),
                zones,
                supports_save: false,
                note: product,
            },
            direct: false,
        }))
    }

    fn write(&self, buf: &[u8]) -> Result<()> {
        self.dev.write(buf).context("strimer write")?;
        Ok(())
    }

    fn set_leds(&self, zone: usize, colors: &[Rgb]) -> Result<()> {
        let mut b = report(PACKET, REPORT_ID);
        b[1] = CMD_COLOR | zone as u8;
        for (i, c) in colors.iter().enumerate() {
            let o = 2 + 3 * i;
            if o + 2 >= PACKET {
                break;
            }
            b[o] = c.r;
            b[o + 1] = c.b;
            b[o + 2] = c.g;
        }
        self.write(&b)
    }

    fn set_direct_mode(&self, zone: usize) -> Result<()> {
        let mut b = report(PACKET, REPORT_ID);
        b[1] = CMD_MODE | zone as u8;
        b[2] = MODE_DIRECT;
        b[3] = SPEED_NORMAL;
        b[4] = 1; // direction, irrelevant for direct
        b[5] = BRIGHTNESS_FULL;
        self.write(&b)
    }

    fn apply_all(&self) -> Result<()> {
        let mut b = report(PACKET, REPORT_ID);
        b[1] = 0x2C;
        b[2] = 0x0F;
        b[3] = 0xFF;
        self.write(&b)
    }
}

impl Device for Strimer {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, r: &Render) -> Result<()> {
        self.direct = false;
        self.write_frame(&r.scaled_colors())?;
        // The controller stutters if hit faster than ~10 Hz.
        thread::sleep(Duration::from_millis(90));
        Ok(())
    }

    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        // Mode packets cost as much as color packets (255 bytes each), so
        // put every zone into direct mode once and then send colors only.
        if !self.direct {
            for zone in 0..self.info.zones.len() {
                self.set_direct_mode(zone)?;
            }
            self.direct = true;
        }
        for (zone, z) in self.info.zones.iter().enumerate() {
            let color = colors.get(zone).copied().unwrap_or(Rgb::BLACK);
            self.set_leds(zone, &vec![color; z.leds])?;
        }
        self.apply_all()
    }
}
