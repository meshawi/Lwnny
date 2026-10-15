//! ASUS Aura USB motherboard controller (USB 0B05:18F3) on the
//! ROG Crosshair VIII Hero. Handles the onboard LEDs plus the 12 V RGB
//! headers (one fixed zone) and the two addressable (Gen 2) headers.
//!
//! Aura runs its effects in firmware and can commit them to flash, so the
//! board keeps lighting with the app closed and even shows a shutdown effect.

use super::{report, Device, DeviceInfo, Render, Rgb, Zone};
use anyhow::{bail, Context, Result};
use hidapi::{HidApi, HidDevice};

pub const VID: u16 = 0x0B05;
pub const PID: u16 = 0x18F3;
pub const ID: &str = "aura_mb";

const RID: u8 = 0xEC;
const REQ_FIRMWARE: u8 = 0x82;
const REQ_CONFIG: u8 = 0xB0;
const CTRL_EFFECT: u8 = 0x35;
const CTRL_EFFECT_COLOR: u8 = 0x36;
const CTRL_COMMIT: u8 = 0x3F;

const MODE_STATIC: u8 = 1;

const CTRL_DIRECT: u8 = 0x40;
const DIRECT_APPLY: u8 = 0x80;
const MODE_DIRECT: u8 = 0xFF;
const LEDS_PER_PACKET: usize = 20;

#[derive(Clone, Copy, Debug)]
struct Channel {
    effect_channel: u8,
    /// Channel id used by direct-mode packets (0x04 mainboard, 0/1 headers).
    direct_channel: u8,
    /// LEDs the firmware counts for this channel (used for color masks).
    fw_leds: u8,
}

pub struct AuraMainboard {
    dev: HidDevice,
    info: DeviceInfo,
    channels: Vec<Channel>,
    config: [u8; 60],
    last: Option<Vec<Rgb>>,
    direct: bool,
}

impl AuraMainboard {
    /// `header_leds`: LED count the user has on addressable header 1 and 2 (0 = unused).
    pub fn open(api: &HidApi, header_leds: [u16; 2]) -> Result<Option<Self>> {
        let Some(di) = api.device_list().find(|d| d.vendor_id() == VID && d.product_id() == PID) else {
            return Ok(None);
        };
        let dev = di.open_device(api).context("open Aura")?;

        let firmware = Self::request(&dev, REQ_FIRMWARE, 0x02)
            .map(|b| String::from_utf8_lossy(&b[2..18]).trim_end_matches('\0').to_string())
            .unwrap_or_default();
        let cfg = Self::request(&dev, REQ_CONFIG, 0x30).context("Aura config table")?;
        let mut config = [0u8; 60];
        config.copy_from_slice(&cfg[4..64]);

        let mb_leds = config[0x1B];
        let rgb_headers = if mb_leds < config[0x1D] { 0 } else { config[0x1D] };
        let argb_headers = config[0x02] as usize;

        let mut zones = Vec::new();
        let mut channels = Vec::new();
        let mut effect_channel = 0u8;
        if mb_leds > 0 {
            zones.push(Zone {
                name: format!("Motherboard ({} LEDs incl. {} RGB headers)", mb_leds, rgb_headers),
                leds: mb_leds as usize,
            });
            channels.push(Channel { effect_channel, direct_channel: 0x04, fw_leds: mb_leds });
            effect_channel += 1;
        }
        for i in 0..argb_headers.min(2) {
            zones.push(Zone { name: format!("Addressable header {}", i + 1), leds: header_leds[i] as usize });
            channels.push(Channel { effect_channel, direct_channel: i as u8, fw_leds: 1 });
            effect_channel += 1;
        }

        // Init packet OpenRGB sends once ("SetGen1").
        let mut b = report(65, RID);
        b[1] = 0x52;
        b[2] = 0x53;
        b[3] = 0x00;
        b[4] = 0x01;
        dev.write(&b).context("Aura init")?;

        Ok(Some(AuraMainboard {
            dev,
            info: DeviceInfo {
                id: ID.into(),
                name: "ASUS Aura (Crosshair VIII Hero)".into(),
                kind: "motherboard".into(),
                zones,
                supports_save: true,
                note: format!("Firmware {firmware}. Effects run in firmware; Save stores them to the board."),
            },
            channels,
            config,
            last: None,
            direct: false,
        }))
    }

    /// Direct mode: per-LED colors, 20 LEDs per packet, apply flag on the last.
    fn send_direct(&self, ch: Channel, colors: &[Rgb]) -> Result<()> {
        let total = colors.len();
        let mut sent = 0usize;
        while sent < total {
            let n = (total - sent).min(LEDS_PER_PACKET);
            let last = sent + n >= total;
            let mut b = report(65, RID);
            b[1] = CTRL_DIRECT;
            b[2] = ch.direct_channel | if last { DIRECT_APPLY } else { 0 };
            b[3] = sent as u8;
            b[4] = n as u8;
            for (i, c) in colors[sent..sent + n].iter().enumerate() {
                let o = 5 + 3 * i;
                b[o] = c.r;
                b[o + 1] = c.g;
                b[o + 2] = c.b;
            }
            self.dev.write(&b).context("Aura direct")?;
            sent += n;
        }
        Ok(())
    }

    pub fn config_table(&self) -> &[u8; 60] {
        &self.config
    }

    fn request(dev: &HidDevice, req: u8, expect: u8) -> Result<Vec<u8>> {
        let mut b = report(65, RID);
        b[1] = req;
        dev.write(&b).context("Aura request")?;
        let mut rx = vec![0u8; 65];
        let n = dev.read_timeout(&mut rx, 1000).context("Aura read")?;
        if n == 0 {
            bail!("Aura: no reply to 0x{req:02X}");
        }
        // hidapi on Windows returns the report id in byte 0.
        if rx[1] != expect {
            bail!("Aura: unexpected reply 0x{:02X} to 0x{req:02X}", rx[1]);
        }
        Ok(rx)
    }

    fn send_effect(&self, ch: Channel, mode: u8, shutdown: bool) -> Result<()> {
        let mut b = report(65, RID);
        b[1] = CTRL_EFFECT;
        b[2] = ch.effect_channel;
        b[3] = 0x00;
        b[4] = shutdown as u8;
        b[5] = mode;
        self.dev.write(&b).context("Aura effect")?;
        Ok(())
    }

    fn send_color(&self, start_led: u8, color: Rgb, count: u8, shutdown: bool) -> Result<()> {
        let mask: u16 = (((1u32 << count) - 1) as u16).wrapping_shl(start_led as u32);
        let mut b = report(65, RID);
        b[1] = CTRL_EFFECT_COLOR;
        b[2] = (mask >> 8) as u8;
        b[3] = (mask & 0xFF) as u8;
        b[4] = shutdown as u8;
        for i in 0..count as usize {
            let o = 5 + 3 * (start_led as usize + i);
            if o + 2 >= b.len() {
                break;
            }
            b[o] = color.r;
            b[o + 1] = color.g;
            b[o + 2] = color.b;
        }
        self.dev.write(&b).context("Aura color")?;
        Ok(())
    }

    fn commit(&self) -> Result<()> {
        let mut b = report(65, RID);
        b[1] = CTRL_COMMIT;
        b[2] = 0x55;
        self.dev.write(&b).context("Aura commit")?;
        Ok(())
    }

    /// Firmware static mode with one color per channel. `shutdown` writes the
    /// colors the board shows while the PC is off.
    fn render(&self, colors: &[Rgb], shutdown: bool) -> Result<()> {
        let mut start_led = 0u8;
        for (idx, ch) in self.channels.iter().enumerate() {
            if self.info.zones[idx].leds > 0 {
                self.send_effect(*ch, MODE_STATIC, shutdown)?;
                let color = colors.get(idx).copied().unwrap_or(Rgb::BLACK);
                self.send_color(start_led, color, ch.fw_leds, shutdown)?;
            }
            start_led = start_led.saturating_add(ch.fw_leds);
        }
        Ok(())
    }
}

impl Device for AuraMainboard {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, r: &Render) -> Result<()> {
        let colors = r.scaled_colors();
        self.direct = false;
        self.render(&colors, false)?;
        self.last = Some(colors);
        Ok(())
    }

    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.direct {
            for (idx, ch) in self.channels.iter().enumerate() {
                if self.info.zones[idx].leds > 0 {
                    self.send_effect(*ch, MODE_DIRECT, false)?;
                }
            }
            self.direct = true;
        }
        for (idx, ch) in self.channels.iter().enumerate() {
            let n = self.info.zones[idx].leds;
            if n == 0 {
                continue;
            }
            let c = colors.get(idx).copied().unwrap_or(Rgb::BLACK);
            self.send_direct(*ch, &vec![c; n])?;
        }
        Ok(())
    }

    fn save(&mut self) -> Result<()> {
        let Some(colors) = self.last.clone() else {
            bail!("nothing applied yet");
        };
        self.render(&colors, false)?;
        self.render(&colors, true)?;
        self.commit()
    }
}
