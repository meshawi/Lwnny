//! Lian Li UNI HUB SL-Infinity (USB 0CF2:A102).
//!
//! The hub has 4 fan ports. Each port exposes two RGB channels (one for the
//! fan's inner ring, one for the side edge strip), so there are 8 channels.
//! Channel `c` belongs to port `c / 2`. Every fan contributes 16 LEDs to each
//! of its two channels.
//!
//! Only lighting commands are sent. Fan speed (PWM) is left entirely to the
//! motherboard header the hub is connected to.

use super::{report, Device, DeviceInfo, Render, Rgb, Zone};
use anyhow::{Context, Result};
use hidapi::{HidApi, HidDevice};
use std::{thread, time::Duration};

pub const VID: u16 = 0x0CF2;
pub const PID: u16 = 0xA102;
pub const ID: &str = "lianli_hub";

const TID: u8 = 0xE0;
const LEDS_PER_FAN: usize = 16;
const MAX_FANS: usize = 4;

const MODE_STATIC: u8 = 0x01;
/// Speed code "very slow" (irrelevant for static) and brightness "full".
const SPEED_SLOW: u8 = 0x02;
const BRIGHTNESS_FULL: u8 = 0x00;

pub struct LianLiHub {
    dev: HidDevice,
    info: DeviceInfo,
    fans_per_port: [u8; 4],
}

impl LianLiHub {
    /// `fans_per_port`: how many SL-Infinity fans are daisy-chained on each
    /// of the 4 ports (0 disables the port).
    pub fn open(api: &HidApi, fans_per_port: [u8; 4]) -> Result<Option<Self>> {
        let Some(di) = api.device_list().find(|d| {
            d.vendor_id() == VID && d.product_id() == PID && d.usage_page() == 0xFF72 && d.usage() == 0xA1
        }) else {
            return Ok(None);
        };
        let dev = di.open_device(api).context("open Lian Li hub")?;
        let product = dev.get_product_string().ok().flatten().unwrap_or_default();

        let mut zones = Vec::new();
        for (port, &n) in fans_per_port.iter().enumerate().take(4) {
            let fans = (n as usize).min(MAX_FANS);
            zones.push(Zone { name: format!("Port {} inner", port + 1), leds: fans * LEDS_PER_FAN });
            zones.push(Zone { name: format!("Port {} edge", port + 1), leds: fans * LEDS_PER_FAN });
        }

        Ok(Some(LianLiHub {
            dev,
            info: DeviceInfo {
                id: ID.into(),
                name: "Lian Li UNI FAN SL-Infinity".into(),
                kind: "fans".into(),
                zones,
                supports_save: false,
                note: format!("{product}. Colors are held by the hub while the PC is powered."),
            },
            fans_per_port,
        }))
    }

    fn fans(&self, channel: usize) -> usize {
        (self.fans_per_port[channel / 2] as usize).min(MAX_FANS)
    }

    fn write(&self, buf: &[u8]) -> Result<()> {
        self.dev.write(buf).context("hub write")?;
        // The hub needs a short gap between packets. A 353-byte color report
        // already takes ~6 ms on the wire, so 1 ms is plenty.
        thread::sleep(Duration::from_millis(1));
        Ok(())
    }

    fn start(&self, channel: usize, fans: usize) -> Result<()> {
        let mut b = report(65, TID);
        b[1] = 0x10;
        b[2] = 0x60;
        b[3] = 1 + (channel as u8) / 2; // port 1..4
        b[4] = fans.clamp(1, MAX_FANS) as u8;
        self.write(&b)
    }

    fn color_data(&self, channel: usize, data: &[u8]) -> Result<()> {
        let mut b = report(353, TID);
        b[1] = 0x30 + channel as u8;
        let n = data.len().min(b.len() - 2);
        b[2..2 + n].copy_from_slice(&data[..n]);
        self.write(&b)
    }

    fn commit(&self, channel: usize, mode: u8, speed: u8, direction: u8, brightness: u8) -> Result<()> {
        let mut b = report(65, TID);
        b[1] = 0x10 + channel as u8;
        b[2] = mode;
        b[3] = speed;
        b[4] = direction;
        b[5] = brightness;
        self.write(&b)
    }

    /// Manufacturer limit: keep r+g+b <= 460 so full white does not overdrive the LEDs.
    fn limit(c: Rgb) -> Rgb {
        let sum = c.r as u32 + c.g as u32 + c.b as u32;
        if sum > 460 {
            c.scaled(460.0 / sum as f32)
        } else {
            c
        }
    }

    /// The hub wants R, B, G byte order.
    fn push_rbg(buf: &mut Vec<u8>, c: Rgb) {
        buf.push(c.r);
        buf.push(c.b);
        buf.push(c.g);
    }

    /// Static color on one channel: start, per-LED color data, commit.
    fn set_channel(&self, channel: usize, color: Rgb) -> Result<()> {
        let fans = self.fans(channel);
        if fans == 0 {
            return Ok(());
        }
        let color = Self::limit(color);
        let mut data = Vec::with_capacity(fans * LEDS_PER_FAN * 3);
        for _ in 0..fans * LEDS_PER_FAN {
            Self::push_rbg(&mut data, color);
        }
        self.start(channel, fans)?;
        self.color_data(channel, &data)?;
        self.commit(channel, MODE_STATIC, SPEED_SLOW, 0x00, BRIGHTNESS_FULL)
    }

    fn set_all(&self, colors: &[Rgb]) -> Result<()> {
        for channel in 0..8 {
            let color = colors.get(channel).copied().unwrap_or(Rgb::BLACK);
            self.set_channel(channel, color)?;
        }
        Ok(())
    }

}

impl Device for LianLiHub {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, r: &Render) -> Result<()> {
        self.set_all(&r.scaled_colors())
    }

    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        self.set_all(colors)
    }
}
