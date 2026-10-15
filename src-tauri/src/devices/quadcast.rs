//! HyperX QuadCast S microphone (USB 0951:171F, interface 0).
//!
//! Two LED groups (top, bottom), controlled with HID feature reports. The
//! mic has no host-independent effects and falls back to its own lighting
//! when it stops hearing from the host, so the engine streams frames to it.
//! `save` writes the current colors to the mic's flash.

use super::{Device, DeviceInfo, Render, Rgb, Zone};
use anyhow::{Context, Result};
use hidapi::{HidApi, HidDevice};
use std::{thread, time::Duration};

pub const VID: u16 = 0x0951;
pub const PID: u16 = 0x171F;
pub const ID: &str = "quadcast";

const PACKET: usize = 65;

pub struct QuadCast {
    dev: HidDevice,
    info: DeviceInfo,
    last: [Rgb; 2],
}

impl QuadCast {
    pub fn open(api: &HidApi) -> Result<Option<Self>> {
        let Some(di) = api
            .device_list()
            .find(|d| d.vendor_id() == VID && d.product_id() == PID && d.interface_number() == 0)
        else {
            return Ok(None);
        };
        let dev = di.open_device(api).context("open QuadCast S")?;
        Ok(Some(QuadCast {
            dev,
            info: DeviceInfo {
                id: ID.into(),
                name: "HyperX QuadCast S".into(),
                kind: "mic".into(),
                zones: vec![Zone { name: "Top".into(), leds: 1 }, Zone { name: "Bottom".into(), leds: 1 }],
                supports_save: true,
                note: "Effects are streamed by the app; Save stores the static colors in the mic.".into(),
            },
            last: [Rgb::BLACK; 2],
        }))
    }

    fn feature(&self, buf: &[u8]) -> Result<()> {
        self.dev.send_feature_report(buf).context("QuadCast feature report")?;
        thread::sleep(Duration::from_millis(15));
        Ok(())
    }

    fn register(&self, reg: u8, p1: u8, p2: u8) -> Result<()> {
        let mut b = vec![0u8; PACKET];
        b[1] = 0x04;
        b[2] = reg;
        b[8] = p1;
        b[9] = p2;
        self.feature(&b)
    }

    fn color_packet(top: Rgb, bottom: Rgb) -> Vec<u8> {
        let mut b = vec![0u8; PACKET];
        b[1] = 0x81;
        b[2] = top.r;
        b[3] = top.g;
        b[4] = top.b;
        b[5] = 0x81;
        b[6] = bottom.r;
        b[7] = bottom.g;
        b[8] = bottom.b;
        b
    }

    fn send_direct(&self, top: Rgb, bottom: Rgb) -> Result<()> {
        self.feature(&Self::color_packet(top, bottom))?;
        self.register(0xF2, 0, 1)
    }

    fn save_colors(&self, top: Rgb, bottom: Rgb) -> Result<()> {
        // One frame -> one color packet.
        self.register(0x53, 1, 0)?;
        self.feature(&Self::color_packet(top, bottom))?;
        self.register(0x02, 0, 0)?;
        self.register(0x23, 1, 0)?;
        let mut eot = vec![0u8; PACKET];
        eot[1] = 0x08;
        eot[0x3C] = 0x28;
        eot[0x3D] = 1;
        eot[0x3F] = 0xAA;
        eot[0x40] = 0x55;
        self.feature(&eot)?;
        self.register(0x02, 0, 0)?;
        self.send_direct(top, bottom)
    }
}

impl Device for QuadCast {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, r: &Render) -> Result<()> {
        let c = r.scaled_colors();
        self.last = [c.first().copied().unwrap_or(Rgb::BLACK), c.get(1).copied().unwrap_or(Rgb::BLACK)];
        self.send_direct(self.last[0], self.last[1])
    }

    fn save(&mut self) -> Result<()> {
        self.save_colors(self.last[0], self.last[1])
    }

    fn needs_software_effects(&self) -> bool {
        true
    }

    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        let top = colors.first().copied().unwrap_or(Rgb::BLACK);
        let bottom = colors.get(1).copied().unwrap_or(top);
        self.send_direct(top, bottom)
    }
}
