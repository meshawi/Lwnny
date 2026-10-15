//! Corsair Vengeance RGB Pro / Dominator / Vengeance RGB DDR5 memory,
//! reached over the chipset SMBus (through PawnIO). One device, one zone
//! per stick.
//!
//! Each stick is a tiny controller at 0x58..0x5F. Lighting is configured by
//! streaming a 20-byte "effect" record into its buffer, checking the CRC the
//! stick computed against ours, and committing it. The record is stored in
//! the stick, so the color survives reboots without any software.

use super::pawnio::Smbus;
use super::{Device, DeviceInfo, Render, Rgb, Zone};
use anyhow::{bail, Context, Result};
use std::{thread, time::Duration};

pub const ID: &str = "ram";

const REG_RESET_BUFFER: u8 = 0x0B;
const REG_SET_BINARY_DATA: u8 = 0x20;
const REG_BINARY_START: u8 = 0x21;
const REG_STATUS: u8 = 0x30;
const REG_GET_BINARY_DATA: u8 = 0x40;
const REG_GET_CHECKSUM: u8 = 0x42;
const REG_GET_DEVICE_INFO: u8 = 0x61;
const REG_WRITE_CONFIGURATION: u8 = 0x82;

const CONFIG_EFFECT: u8 = 1;
const CONFIG_COLORS: u8 = 2;
const MODE_STATIC: u8 = 0x10;
const SPEED_SLOW: u8 = 0x00;
const CUSTOM_COLORS: u8 = 0x01;

struct Stick {
    addr: u8,
    firmware: String,
    pid: u16,
    leds: usize,
    /// Static mode already selected this session.
    mode_set: bool,
    /// Last color written, so unchanged colors are not rewritten (the
    /// stick stores every write in its flash).
    last: Option<Rgb>,
}

pub struct CorsairDram {
    bus: Smbus,
    sticks: Vec<Stick>,
    info: DeviceInfo,
}

/// CRC-8 (poly 0x07, init 0), as the sticks compute it.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

/// The 20-byte effect record for static mode (same layout as the other
/// effects; the two color slots are unused in static mode).
fn static_record() -> [u8; 20] {
    let mut rec = [0u8; 20];
    rec[0] = MODE_STATIC;
    rec[1] = SPEED_SLOW;
    rec[2] = CUSTOM_COLORS;
    rec[3] = 0; // direction
    rec[7] = 255; // brightness, color 1
    rec[11] = 255; // brightness, color 2
    rec
}

/// Per-LED color buffer: R, G, B, 0xFF for each LED.
fn color_block(leds: usize, c: Rgb) -> Vec<u8> {
    (0..leds).flat_map(|_| [c.r, c.g, c.b, 0xFF]).collect()
}

fn model_name(pid: u16) -> (&'static str, usize) {
    match pid {
        0x0100 | 0x0101 => ("Vengeance RGB Pro DDR4", 10),
        0x0200 | 0x0201 => ("Dominator Platinum RGB DDR4", 12),
        0x0300 | 0x0301 => ("Vengeance RGB Pro SL DDR4", 10),
        0x0400 | 0x0401 => ("Vengeance RGB RS DDR4", 6),
        0x0600 | 0x0601 => ("Dominator Platinum RGB DDR5", 12),
        0x0800 | 0x0801 | 0x0810 | 0x0811 => ("Dominator Titanium RGB DDR5", 12),
        0x0700 | 0x0701 | 0x0900 | 0x0901 | 0x0910 | 0x0911 => ("Vengeance RGB DDR5", 10),
        0x0A00 | 0x0A01 | 0x0A10 | 0x0A11 => ("Vengeance Shugo DDR5", 10),
        0x0B00 | 0x0B01 => ("Vengeance RGB RS DDR5", 6),
        _ => ("Corsair RGB DRAM", 10),
    }
}

impl CorsairDram {
    /// Open the SMBus and look for sticks. Returns Ok(None) when none answer.
    pub fn open() -> Result<Option<Self>> {
        let bus = Smbus::open_piix4()?;
        let mut sticks = Vec::new();
        for addr in (0x58u8..=0x5F).chain(0x18u8..=0x1F) {
            if Self::probe(&bus, addr) {
                match Self::read_info(&bus, addr) {
                    Ok((pid, firmware)) => {
                        let leds = model_name(pid).1;
                        sticks.push(Stick { addr, firmware, pid, leds, mode_set: false, last: None })
                    }
                    Err(e) => log::warn!("RAM 0x{addr:02X}: info read failed: {e}"),
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        if sticks.is_empty() {
            return Ok(None);
        }
        let (model, _leds) = model_name(sticks[0].pid);
        let zones = sticks
            .iter()
            .enumerate()
            .map(|(i, s)| Zone { name: format!("Stick {} (0x{:02X})", i + 1, s.addr), leds: s.leds })
            .collect();
        let fw = sticks[0].firmware.clone();
        Ok(Some(CorsairDram {
            info: DeviceInfo {
                id: ID.into(),
                name: format!("Corsair {model}"),
                kind: "ram".into(),
                zones,
                supports_save: true,
                note: format!("{} sticks via SMBus ({}), firmware {fw}. Colors are stored in the sticks.", sticks.len(), bus.name),
            },
            bus,
            sticks,
        }))
    }

    fn probe(bus: &Smbus, addr: u8) -> bool {
        let Ok(a) = bus.read_byte_data(addr, 0x43) else { return false };
        if !matches!(a, 0x1A..=0x1C) {
            return false;
        }
        matches!(bus.read_byte_data(addr, 0x44), Ok(0x01 | 0x03 | 0x04))
    }

    fn read_info(bus: &Smbus, addr: u8) -> Result<(u16, String)> {
        bus.write_byte_data(addr, REG_GET_DEVICE_INFO, 0)?;
        bus.write_byte_data(addr, REG_BINARY_START, 0)?;
        let mut d = [0u8; 32];
        for b in d.iter_mut() {
            *b = bus.read_byte_data(addr, REG_GET_BINARY_DATA)?;
        }
        let dev_crc = bus.read_byte_data(addr, REG_GET_CHECKSUM)?;
        if dev_crc != crc8(&d) {
            bail!("device info CRC mismatch");
        }
        let pid = u16::from_le_bytes([d[2], d[3]]);
        let fw = format!("{}.{}.{}", d[9], d[8], u16::from_le_bytes([d[10], d[11]]));
        Ok((pid, fw))
    }

    /// Wait for the stick to finish storing a configuration (status 0x30,
    /// bit 3 = busy). A slow commit is logged, not treated as a failure:
    /// the data was already checked by CRC before the commit.
    fn wait_ready(&self, addr: u8) -> Result<()> {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(1500) {
            if let Ok(s) = self.bus.read_byte_data(addr, REG_STATUS) {
                if s & 0x08 == 0 {
                    return Ok(());
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        log::warn!("RAM stick 0x{addr:02X} still busy after 1.5 s");
        Ok(())
    }

    /// Stream `data` into the stick's buffer, check the CRC it computed
    /// against ours, and apply it as configuration `id`.
    fn write_config(&self, addr: u8, id: u8, data: &[u8]) -> Result<()> {
        self.bus.write_byte_data(addr, REG_RESET_BUFFER, 0)?;
        self.bus.write_byte_data(addr, REG_BINARY_START, 0)?;
        for &b in data {
            self.bus.write_byte_data(addr, REG_SET_BINARY_DATA, b)?;
        }
        let dev_crc = self.bus.read_byte_data(addr, REG_GET_CHECKSUM)?;
        if dev_crc != crc8(data) {
            bail!("stick 0x{addr:02X}: CRC mismatch, not applied");
        }
        self.bus.write_byte_data(addr, REG_WRITE_CONFIGURATION, id)?;
        self.wait_ready(addr)
    }

    /// Select the static effect. Its colors come from the per-LED color
    /// buffer written by [`Self::set_colors`], not from this record.
    fn set_static_mode(&self, addr: u8) -> Result<()> {
        self.write_config(addr, CONFIG_EFFECT, &static_record())?;
        thread::sleep(Duration::from_millis(15));
        Ok(())
    }

    /// Store one color on every LED of a stick.
    fn set_colors(&self, addr: u8, leds: usize, c: Rgb) -> Result<()> {
        self.write_config(addr, CONFIG_COLORS, &color_block(leds, c))
    }
    /// Store one color per stick, skipping sticks that already show it.
    fn write_colors(&mut self, colors: &[Rgb]) -> Result<()> {
        let started = std::time::Instant::now();
        self.bus.select_port0().context("SMBus port")?;
        let mut written = 0;
        for i in 0..self.sticks.len() {
            let c = colors.get(i).copied().unwrap_or(Rgb::BLACK);
            let (addr, leds) = (self.sticks[i].addr, self.sticks[i].leds);
            if !self.sticks[i].mode_set {
                self.set_static_mode(addr)?;
                self.sticks[i].mode_set = true;
            }
            if self.sticks[i].last != Some(c) {
                self.set_colors(addr, leds, c)?;
                self.sticks[i].last = Some(c);
                written += 1;
            }
        }
        if written > 0 {
            log::info!("RAM write took {} ms ({written} sticks)", started.elapsed().as_millis());
        }
        Ok(())
    }
}

impl Device for CorsairDram {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, r: &Render) -> Result<()> {
        self.write_colors(&r.scaled_colors())
    }

    /// Used by "Find" to flash one stick; the engine re-applies the scene
    /// afterwards.
    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        self.write_colors(colors)
    }

    fn commit_delay(&self) -> Option<Duration> {
        Some(Duration::from_millis(700))
    }

    fn save(&mut self) -> Result<()> {
        // The effect record written by `apply` already lives in the stick.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_matches_the_standard_check_value() {
        // CRC-8 (poly 0x07, init 0) of "123456789" is 0xF4.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn static_record_layout() {
        let r = static_record();
        assert_eq!(r.len(), 20);
        assert_eq!(&r[..4], &[0x10, 0x00, 0x01, 0x00]);
        assert_eq!(r[7], 255);
        assert_eq!(r[11], 255);
        assert!(r[12..].iter().all(|&b| b == 0));
    }

    #[test]
    fn color_block_is_four_bytes_per_led() {
        let b = color_block(10, Rgb::new(1, 2, 3));
        assert_eq!(b.len(), 40);
        assert!(b.chunks(4).all(|c| c == [1, 2, 3, 0xFF]));
    }

    #[test]
    fn model_table_knows_the_vengeance_pro() {
        assert_eq!(model_name(0x0100), ("Vengeance RGB Pro DDR4", 10));
        assert_eq!(model_name(0x0200).1, 12);
        assert_eq!(model_name(0xFFFF).1, 10);
    }
}
