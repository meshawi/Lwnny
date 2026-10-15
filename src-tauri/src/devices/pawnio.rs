//! Minimal client for the PawnIO kernel driver (https://pawnio.eu) and an
//! SMBus bus on top of its `SmbusPIIX4` module (AMD chipsets).
//!
//! PawnIO is a signed driver that runs small sandboxed modules; user mode
//! talks to it with two IOCTLs: load a module blob, execute a named function
//! with an array of u64 arguments. Opening the device needs administrator
//! rights.
//!
//! Safety rules for the shared SMBus:
//! - every transaction is wrapped in the system-wide mutex that monitoring
//!   tools (HWiNFO, AIDA64, OpenRGB, ...) also honour;
//! - the bus is only touched when the user changes a color, never polled;
//! - the module spins (in the kernel, well under a millisecond per byte)
//!   rather than sleeping: a timer sleep per byte costs 1-15 ms and made a
//!   4-stick write take seconds; the bus is only used on color changes.

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// Compiled module, unmodified, from the PawnIO.Modules project (LGPL-2.1).
pub const SMBUS_PIIX4_MODULE: &[u8] = include_bytes!("../../pawnio/SmbusPIIX4.bin");

const DEVICE_TYPE: u32 = 41394;
const fn ctl_code(function: u32) -> u32 {
    // CTL_CODE(type, function, METHOD_BUFFERED, FILE_ANY_ACCESS)
    (DEVICE_TYPE << 16) | (function << 2)
}
const IOCTL_LOAD_BINARY: u32 = ctl_code(0x821);
const IOCTL_EXECUTE_FN: u32 = ctl_code(0x841);
const FN_NAME_LEN: usize = 32;

/// The driver's NT device path, reachable without ntdll through GLOBALROOT.
const DEVICE_PATH: &str = r"\\?\GLOBALROOT\Device\PawnIO";

/// Name of the mutex every well-behaved SMBus user on Windows takes.
const SMBUS_MUTEX: &[u8] = b"Global\\Access_SMBUS.HTP.Method\0";

pub struct PawnIo {
    handle: HANDLE,
}

unsafe impl Send for PawnIo {}

impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

impl PawnIo {
    /// Open the driver. Fails with "access denied" when not elevated and
    /// "not found" when PawnIO is not installed.
    pub fn open() -> Result<PawnIo> {
        let path: Vec<u16> = DEVICE_PATH.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let code = unsafe { GetLastError() };
            return Err(match code {
                5 => anyhow!("PawnIO: access denied (Lwnny must run as administrator)"),
                2 | 3 => anyhow!("PawnIO driver is not installed (https://pawnio.eu)"),
                c => anyhow!("PawnIO: open failed (error {c})"),
            });
        }
        Ok(PawnIo { handle })
    }

    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize> {
        let mut written: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                code,
                input.as_ptr() as *const c_void,
                input.len() as u32,
                output.as_mut_ptr() as *mut c_void,
                output.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            bail!("PawnIO ioctl {code:#x} failed (error {})", unsafe { GetLastError() });
        }
        Ok(written as usize)
    }

    pub fn load(&self, blob: &[u8]) -> Result<()> {
        self.ioctl(IOCTL_LOAD_BINARY, blob, &mut []).context("PawnIO load module")?;
        Ok(())
    }

    /// Call a module function. `out_len` is the number of u64 output cells.
    pub fn execute(&self, name: &str, input: &[u64], out_len: usize) -> Result<Vec<u64>> {
        if name.len() >= FN_NAME_LEN {
            bail!("PawnIO function name too long");
        }
        let mut buf = vec![0u8; FN_NAME_LEN + input.len() * 8];
        buf[..name.len()].copy_from_slice(name.as_bytes());
        for (i, v) in input.iter().enumerate() {
            buf[FN_NAME_LEN + i * 8..FN_NAME_LEN + i * 8 + 8].copy_from_slice(&v.to_le_bytes());
        }
        let mut out = vec![0u8; out_len * 8];
        let written = self.ioctl(IOCTL_EXECUTE_FN, &buf, &mut out).with_context(|| format!("PawnIO {name}"))?;
        Ok(out[..written.min(out.len())].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect())
    }
}

/// Holds the global SMBus mutex for the lifetime of the value.
struct BusLock(HANDLE);

impl BusLock {
    fn take() -> Result<BusLock> {
        let h = unsafe { CreateMutexA(std::ptr::null(), 0, SMBUS_MUTEX.as_ptr()) };
        if h.is_null() {
            bail!("could not create SMBus mutex");
        }
        // Never wait forever: a hung sensor tool must not freeze us.
        let w = unsafe { WaitForSingleObject(h, 5000) };
        if w != WAIT_OBJECT_0 && w != WAIT_ABANDONED {
            unsafe { CloseHandle(h) };
            bail!("SMBus busy (another program holds the bus)");
        }
        Ok(BusLock(h))
    }
}

impl Drop for BusLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

const SMBUS_READ: u64 = 1;
const SMBUS_WRITE: u64 = 0;
const BYTE_DATA: u64 = 2;
const WORD_DATA: u64 = 3;
const BLOCK_DATA: u64 = 5;
/// How the kernel module waits for a transfer: 0 = spin (a byte takes
/// ~0.3 ms), 1 = sleep for the expected time then spin, 2 = always sleep.
/// Windows timer sleeps cost 1-15 ms each, so 1 and 2 make a 4-stick write
/// take seconds; 0 spins for well under a millisecond per byte.
pub const SLEEPMODE_BUSY: u64 = 0;
pub const SLEEPMODE_SHORT_BUSY: u64 = 1;
pub const SLEEPMODE_SLEEP: u64 = 2;

/// SMBus master on the AMD FCH, port 0 (where the DIMMs live).
pub struct Smbus {
    io: PawnIo,
    pub name: String,
}

impl Smbus {
    pub fn open_piix4() -> Result<Smbus> {
        let io = PawnIo::open()?;
        io.load(SMBUS_PIIX4_MODULE).context("load SmbusPIIX4 module (unsupported chipset?)")?;
        let _ = io.execute("ioctl_set_sleep_mode", &[SLEEPMODE_BUSY], 0);
        let id = io.execute("ioctl_identity", &[0], 3).context("SMBus identity")?;
        let name = if id.len() >= 3 {
            let tag: String = id[0].to_le_bytes().iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
            format!("{tag} @ 0x{:X}, PCI {:04X}:{:04X}", id[1], id[2] & 0xFFFF, (id[2] >> 16) & 0xFFFF)
        } else {
            "PIIX4".into()
        };
        let bus = Smbus { io, name };
        bus.select_port0()?;
        Ok(bus)
    }

    /// Change how the module waits for transfers (see `SLEEPMODE_*`).
    pub fn set_sleep_mode(&self, mode: u64) -> Result<()> {
        self.io.execute("ioctl_set_sleep_mode", &[mode], 0).context("set SMBus sleep mode")?;
        Ok(())
    }

    /// Make sure the chipset is on SMBus port 0 (other tools may switch it).
    pub fn select_port0(&self) -> Result<()> {
        let _lock = BusLock::take()?;
        self.io.execute("ioctl_piix4_port_sel", &[0], 1).context("select SMBus port 0")?;
        Ok(())
    }

    pub fn read_byte_data(&self, addr: u8, command: u8) -> Result<u8> {
        let _lock = BusLock::take()?;
        let out = self.io.execute("ioctl_smbus_xfer", &[addr as u64, SMBUS_READ, command as u64, BYTE_DATA], 1)?;
        Ok(out.first().copied().unwrap_or(0) as u8)
    }

    pub fn write_byte_data(&self, addr: u8, command: u8, value: u8) -> Result<()> {
        let _lock = BusLock::take()?;
        self.io.execute("ioctl_smbus_xfer", &[addr as u64, SMBUS_WRITE, command as u64, BYTE_DATA, value as u64], 0)?;
        Ok(())
    }

    pub fn write_word_data(&self, addr: u8, command: u8, value: u16) -> Result<()> {
        let _lock = BusLock::take()?;
        self.io.execute("ioctl_smbus_xfer", &[addr as u64, SMBUS_WRITE, command as u64, WORD_DATA, value as u64], 0)?;
        Ok(())
    }

    /// SMBus block write (1..=32 bytes).
    pub fn write_block_data(&self, addr: u8, command: u8, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > 32 {
            bail!("block size {} out of range", data.len());
        }
        // 40 bytes packed little-endian into 5 cells: [len, data...]
        let mut packed = [0u8; 40];
        packed[0] = data.len() as u8;
        packed[1..1 + data.len()].copy_from_slice(data);
        let mut input = vec![addr as u64, SMBUS_WRITE, command as u64, BLOCK_DATA];
        for c in packed.chunks_exact(8) {
            input.push(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let _lock = BusLock::take()?;
        self.io.execute("ioctl_smbus_xfer", &input, 0)?;
        Ok(())
    }
}
