//! Minimal NvAPI binding: enumerate GPUs and talk I2C to the RGB controller
//! that sits on the graphics card. Loads nvapi64.dll at runtime, so the app
//! still starts on a machine without the NVIDIA driver.

use anyhow::{bail, Context, Result};
use libloading::Library;
use std::ffi::c_void;

pub type NvHandle = *mut c_void;
type QueryInterface = unsafe extern "C" fn(u32) -> *const c_void;
type FnInitialize = unsafe extern "C" fn() -> i32;
type FnEnumGpus = unsafe extern "C" fn(*mut NvHandle, *mut i32) -> i32;
type FnPciIds = unsafe extern "C" fn(NvHandle, *mut u32, *mut u32, *mut u32, *mut u32) -> i32;
type FnI2c = unsafe extern "C" fn(NvHandle, *mut NvI2cInfoV3, *mut u32) -> i32;

const ID_INITIALIZE: u32 = 0x0150E828;
const ID_ENUM_PHYSICAL_GPUS: u32 = 0xE5AC921F;
const ID_GET_PCI_IDENTIFIERS: u32 = 0x2DDFB66E;
const ID_I2C_WRITE_EX: u32 = 0x283AC65A;
const ID_I2C_READ_EX: u32 = 0x4D7B0709;

#[repr(C)]
pub struct NvI2cInfoV3 {
    version: u32,
    display_mask: u32,
    is_ddc_port: u8,
    i2c_dev_address: u8,
    i2c_reg_address: *mut u8,
    reg_addr_size: u32,
    data: *mut u8,
    size: u32,
    i2c_speed: u32,
    i2c_speed_khz: u32,
    port_id: u8,
    is_port_id_set: u32,
}

pub struct NvApi {
    _lib: Library,
    enum_gpus: FnEnumGpus,
    pci_ids: FnPciIds,
    i2c_read: FnI2c,
    i2c_write: FnI2c,
}

#[derive(Clone, Copy, Debug)]
pub struct Gpu {
    pub handle: NvHandle,
    pub vendor: u16,
    pub device: u16,
    pub sub_vendor: u16,
    pub sub_device: u16,
}

unsafe impl Send for NvApi {}
unsafe impl Send for Gpu {}

impl NvApi {
    pub fn load() -> Result<NvApi> {
        let lib = unsafe { Library::new("nvapi64.dll") }.context("load nvapi64.dll")?;
        let query: QueryInterface = unsafe { *lib.get(b"nvapi_QueryInterface\0").context("nvapi_QueryInterface")? };
        let get = |id: u32| -> Result<*const c_void> {
            let p = unsafe { query(id) };
            if p.is_null() {
                bail!("NvAPI interface {id:08X} missing");
            }
            Ok(p)
        };
        let init: FnInitialize = unsafe { std::mem::transmute(get(ID_INITIALIZE)?) };
        let enum_gpus: FnEnumGpus = unsafe { std::mem::transmute(get(ID_ENUM_PHYSICAL_GPUS)?) };
        let pci_ids: FnPciIds = unsafe { std::mem::transmute(get(ID_GET_PCI_IDENTIFIERS)?) };
        let i2c_write: FnI2c = unsafe { std::mem::transmute(get(ID_I2C_WRITE_EX)?) };
        let i2c_read: FnI2c = unsafe { std::mem::transmute(get(ID_I2C_READ_EX)?) };
        let st = unsafe { init() };
        if st != 0 {
            bail!("NvAPI_Initialize failed: {st}");
        }
        Ok(NvApi { _lib: lib, enum_gpus, pci_ids, i2c_read, i2c_write })
    }

    pub fn gpus(&self) -> Result<Vec<Gpu>> {
        let mut handles: [NvHandle; 64] = [std::ptr::null_mut(); 64];
        let mut count = 0i32;
        let st = unsafe { (self.enum_gpus)(handles.as_mut_ptr(), &mut count) };
        if st != 0 {
            bail!("NvAPI_EnumPhysicalGPUs failed: {st}");
        }
        let mut out = Vec::new();
        for &h in handles.iter().take(count.max(0) as usize) {
            let (mut dev, mut sub, mut rev, mut ext) = (0u32, 0u32, 0u32, 0u32);
            let st = unsafe { (self.pci_ids)(h, &mut dev, &mut sub, &mut rev, &mut ext) };
            if st != 0 {
                continue;
            }
            out.push(Gpu {
                handle: h,
                vendor: (dev & 0xFFFF) as u16,
                device: (dev >> 16) as u16,
                sub_vendor: (sub & 0xFFFF) as u16,
                sub_device: (sub >> 16) as u16,
            });
        }
        Ok(out)
    }

    fn info(addr: u8, reg: &mut [u8], data: &mut [u8]) -> NvI2cInfoV3 {
        NvI2cInfoV3 {
            version: (3 << 16) | std::mem::size_of::<NvI2cInfoV3>() as u32,
            display_mask: 0,
            is_ddc_port: 0,
            i2c_dev_address: addr << 1,
            i2c_reg_address: if reg.is_empty() { std::ptr::null_mut() } else { reg.as_mut_ptr() },
            reg_addr_size: reg.len() as u32,
            data: data.as_mut_ptr(),
            size: data.len() as u32,
            i2c_speed: 0xFFFF,
            i2c_speed_khz: 0, // default
            port_id: 1,       // RGB controllers hang off port 1
            is_port_id_set: 1,
        }
    }

    /// I2C write: `reg` (0 or 1 byte) followed by `data`.
    pub fn i2c_write(&self, gpu: NvHandle, addr: u8, reg: &[u8], data: &[u8]) -> Result<()> {
        let mut reg = reg.to_vec();
        let mut data = data.to_vec();
        let mut info = Self::info(addr, &mut reg, &mut data);
        let mut unknown = 0u32;
        let st = unsafe { (self.i2c_write)(gpu, &mut info, &mut unknown) };
        if st != 0 {
            bail!("NvAPI_I2CWriteEx failed: {st}");
        }
        Ok(())
    }

    /// I2C read of `len` bytes after sending `reg`.
    pub fn i2c_read(&self, gpu: NvHandle, addr: u8, reg: &[u8], len: usize) -> Result<Vec<u8>> {
        let mut reg = reg.to_vec();
        let mut data = vec![0u8; len];
        let mut info = Self::info(addr, &mut reg, &mut data);
        let mut unknown = 0u32;
        let st = unsafe { (self.i2c_read)(gpu, &mut info, &mut unknown) };
        if st != 0 {
            bail!("NvAPI_I2CReadEx failed: {st}");
        }
        data.truncate(info.size as usize);
        Ok(data)
    }
}
