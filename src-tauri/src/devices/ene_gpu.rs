//! ENE RGB controller on the ASUS ROG Strix RTX 3090 (I2C address 0x67 on
//! the GPU's I2C port 1, reached through NvAPI). Second-generation
//! controller "AUMA0-E6K5-0107": effect colors at 0x8160, direct at 0x8100.
//!
//! Register access is SMBus-style: write the 16-bit ENE register to
//! sub-register 0x00 (big-endian), then read 0x81 / write 0x01 / block 0x03.

use super::nvapi::{Gpu, NvApi};
use super::{Device, DeviceInfo, Render, Rgb, Zone};
use anyhow::{bail, Context, Result};

pub const ID: &str = "gpu";
const ADDR: u8 = 0x67;

const REG_DEVICE_NAME: u16 = 0x1000;
const REG_CONFIG_TABLE: u16 = 0x1C00;
const REG_DIRECT: u16 = 0x8020;
const REG_MODE: u16 = 0x8021;
const REG_SPEED: u16 = 0x8022;
const REG_DIRECTION: u16 = 0x8023;
const REG_APPLY: u16 = 0x80A0;
const APPLY: u8 = 0x01;
const SAVE: u8 = 0xAA;

const MODE_STATIC: u8 = 1;
/// Bytes per block write. SMBus allows 32; OpenRGB uses 3 to be safe. We
/// verify a larger block on open and fall back to 3 if the chip rejects it.
const BLOCK_FAST: usize = 30;
const BLOCK_SAFE: usize = 3;

pub struct EneGpu {
    api: NvApi,
    gpu: Gpu,
    info: DeviceInfo,
    config: [u8; 64],
    direct_reg: u16,
    effect_reg: u16,
    /// LED count per zone as exposed to the app.
    zone_leds: Vec<usize>,
    direct: bool,
    block: usize,
}

impl EneGpu {
    pub fn open() -> Result<Option<Self>> {
        let api = match NvApi::load() {
            Ok(a) => a,
            Err(e) => {
                log::info!("NvAPI unavailable: {e}");
                return Ok(None);
            }
        };
        for gpu in api.gpus()? {
            if gpu.vendor != 0x10DE {
                continue;
            }
            match Self::probe(api, gpu) {
                Ok(Some(dev)) => return Ok(Some(dev)),
                Ok(None) => {}
                Err(e) => log::info!("GPU {:04X}:{:04X}: no ENE controller ({e})", gpu.device, gpu.sub_device),
            }
            // probe consumed the api; reload for the next GPU (rare: multi-GPU)
            return Ok(None);
        }
        Ok(None)
    }

    fn probe(api: NvApi, gpu: Gpu) -> Result<Option<Self>> {
        let mut name = [0u8; 16];
        for (i, b) in name.iter_mut().enumerate() {
            *b = Self::reg_read(&api, gpu, REG_DEVICE_NAME + i as u16)?;
        }
        let name_s = String::from_utf8_lossy(&name).trim_end_matches('\0').to_string();
        if !name_s.starts_with("AUMA0") && !name_s.starts_with("AU") {
            bail!("unexpected device string {name_s:?}");
        }
        let mut config = [0u8; 64];
        for (i, b) in config.iter_mut().enumerate() {
            *b = Self::reg_read(&api, gpu, REG_CONFIG_TABLE + i as u16)?;
        }
        // Second generation controllers use the V2 color registers.
        let v2 = name_s.contains("E6K5") || name_s.contains("E8K5");
        let (direct_reg, effect_reg, channel_cfg) = if v2 { (0x8100, 0x8160, 0x1B) } else { (0x8000, 0x8010, 0x13) };

        let mut zones = Vec::new();
        let mut zone_leds = Vec::new();
        for z in 0..8usize {
            let n = config[3 + z] as usize;
            if n == 0 {
                continue;
            }
            let ch = config[channel_cfg + z];
            zones.push(Zone { name: format!("{} ({} LEDs)", channel_name(ch), n), leds: n });
            zone_leds.push(n);
        }
        if zones.is_empty() {
            bail!("ENE config table reports no LEDs");
        }
        let total: usize = zone_leds.iter().sum();

        let mut dev = EneGpu {
            api,
            gpu,
            info: DeviceInfo {
                id: ID.into(),
                name: "ASUS ROG Strix RTX 3090".into(),
                kind: "gpu".into(),
                zones,
                supports_save: true,
                note: format!("ENE {name_s}, {total} LEDs, PCI {:04X}:{:04X}", gpu.sub_vendor, gpu.sub_device),
            },
            config,
            direct_reg,
            effect_reg,
            zone_leds,
            direct: false,
            block: BLOCK_FAST,
        };
        dev.block = if dev.verify_fast_blocks() { BLOCK_FAST } else { BLOCK_SAFE };
        log::info!("ENE GPU block size {}", dev.block);
        Ok(Some(dev))
    }

    /// Write a test pattern to the direct color registers with large blocks
    /// and read it back. Direct colors are harmless to touch: the next apply
    /// overwrites them.
    fn verify_fast_blocks(&self) -> bool {
        let pattern: Vec<u8> = (0..BLOCK_FAST as u8).map(|i| 0x10 + i).collect();
        if self.reg_write_block(self.direct_reg, &pattern).is_err() {
            return false;
        }
        for (i, want) in pattern.iter().enumerate() {
            match Self::reg_read(&self.api, self.gpu, self.direct_reg + i as u16) {
                Ok(v) if v == *want => {}
                _ => return false,
            }
        }
        true
    }

    pub fn config_table(&self) -> &[u8; 64] {
        &self.config
    }

    fn select(api: &NvApi, gpu: Gpu, reg: u16) -> Result<()> {
        api.i2c_write(gpu.handle, ADDR, &[0x00], &[(reg >> 8) as u8, (reg & 0xFF) as u8])
    }

    fn reg_read(api: &NvApi, gpu: Gpu, reg: u16) -> Result<u8> {
        Self::select(api, gpu, reg)?;
        let v = api.i2c_read(gpu.handle, ADDR, &[0x81], 1)?;
        v.first().copied().context("empty I2C read")
    }

    fn reg_write(&self, reg: u16, val: u8) -> Result<()> {
        Self::select(&self.api, self.gpu, reg)?;
        self.api.i2c_write(self.gpu.handle, ADDR, &[0x01], &[val])
    }

    fn reg_write_block(&self, reg: u16, data: &[u8]) -> Result<()> {
        Self::select(&self.api, self.gpu, reg)?;
        let mut payload = vec![data.len() as u8];
        payload.extend_from_slice(data);
        if self.api.i2c_write(self.gpu.handle, ADDR, &[0x03], &payload).is_err() {
            // Fall back to byte writes.
            for b in data {
                self.api.i2c_write(self.gpu.handle, ADDR, &[0x01], &[*b])?;
            }
        }
        Ok(())
    }

    /// Expand zone colors to per-LED bytes in the controller's R, B, G order.
    fn led_bytes(&self, zone_colors: &[Rgb]) -> Vec<u8> {
        let mut out = Vec::new();
        for (z, &n) in self.zone_leds.iter().enumerate() {
            let c = zone_colors.get(z).copied().unwrap_or(Rgb::BLACK);
            for _ in 0..n {
                out.extend_from_slice(&[c.r, c.b, c.g]);
            }
        }
        out
    }

    fn enter_direct(&mut self) -> Result<()> {
        self.reg_write(REG_MODE, MODE_STATIC)?;
        self.reg_write(REG_DIRECT, 1)?;
        self.reg_write(REG_APPLY, APPLY)?;
        self.direct = true;
        Ok(())
    }

    fn write_colors(&self, base: u16, bytes: &[u8]) -> Result<()> {
        let n = self.block.max(1);
        for (i, chunk) in bytes.chunks(n).enumerate() {
            self.reg_write_block(base + (i * n) as u16, chunk)?;
        }
        Ok(())
    }
}

fn channel_name(ch: u8) -> &'static str {
    match ch {
        0x82 | 0x83 => "Center",
        0x84 => "Audio",
        0x85 => "Back I/O",
        0x86 | 0x87 | 0x91 => "RGB header",
        0x88 => "Backplate",
        0x8A | 0x05 | 0x0E => "DRAM",
        0x8B => "PCIe",
        _ => "GPU zone",
    }
}

impl Device for EneGpu {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn apply(&mut self, r: &Render) -> Result<()> {
        let bytes = self.led_bytes(&r.scaled_colors());
        // Effect registers hold what the card shows after a reboot (via Save);
        // direct registers are what it shows now. The firmware's own static
        // effect treats every non-zero channel as fully on, so mixed colors
        // only look right in direct mode, which the chip holds until power-off.
        self.write_colors(self.effect_reg, &bytes)?;
        self.write_colors(self.direct_reg, &bytes)?;
        self.reg_write(REG_SPEED, 0x02)?;
        self.reg_write(REG_DIRECTION, 0)?;
        self.enter_direct()
    }

    fn save(&mut self) -> Result<()> {
        self.reg_write(REG_APPLY, SAVE)
    }

    fn write_frame(&mut self, colors: &[Rgb]) -> Result<()> {
        let bytes = self.led_bytes(colors);
        self.write_colors(self.direct_reg, &bytes)?;
        if !self.direct {
            self.enter_direct()?;
        }
        Ok(())
    }

    fn write_leds(&mut self, leds: &[Rgb]) -> Result<()> {
        let mut bytes = Vec::with_capacity(leds.len() * 3);
        for c in leds {
            bytes.extend_from_slice(&[c.r, c.b, c.g]);
        }
        self.write_colors(self.direct_reg, &bytes)?;
        self.enter_direct()
    }

    fn debug_dump(&mut self) -> Option<String> {
        let rd = |reg: u16| Self::reg_read(&self.api, self.gpu, reg).map(|v| format!("{v:02X}")).unwrap_or_else(|e| format!("err({e})"));
        let mut s = String::new();
        s.push_str(&format!("direct=0x{} mode=0x{} speed=0x{} dir=0x{}\n", rd(REG_DIRECT), rd(REG_MODE), rd(REG_SPEED), rd(REG_DIRECTION)));
        s.push_str("direct colors (R,B,G x4): ");
        for i in 0..12 {
            s.push_str(&rd(self.direct_reg + i));
            s.push(' ');
        }
        s.push_str("\neffect colors (R,B,G x4): ");
        for i in 0..12 {
            s.push_str(&rd(self.effect_reg + i));
            s.push(' ');
        }
        s.push_str("\nconfig: ");
        for b in self.config.iter() {
            s.push_str(&format!("{b:02X} "));
        }
        Some(s)
    }
}
