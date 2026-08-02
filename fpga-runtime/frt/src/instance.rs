use crate::cosim::CosimDevice;
use crate::device::{BufferAccess, Device, RuntimeArgInfo};
use crate::error::{FrtError, Result};
use crate::xrt::device::XrtDevice;
use std::path::Path;

#[derive(Clone, Debug)]
pub enum Simulator {
    Verilator,
    Xsim { legacy: bool },
}

pub struct Instance {
    device: Box<dyn Device>,
}

impl Instance {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        match path.extension().and_then(|e| e.to_str()) {
            Some("xclbin") => Ok(Self {
                device: Box::new(XrtDevice::open(path)?),
            }),
            _ => Err(FrtError::NoDevice {
                path: path.to_owned(),
            }),
        }
    }

    pub fn open_cosim(path: impl AsRef<Path>, sim: &Simulator) -> Result<Self> {
        Ok(Self {
            device: Box::new(CosimDevice::open(path.as_ref(), sim)?),
        })
    }

    pub fn set_scalar_arg_bytes(&mut self, index: u32, value: &[u8]) -> Result<()> {
        self.device.set_scalar_arg(index, value)
    }

    pub fn set_buffer_arg_raw(&mut self, index: u32, ptr: *mut u8, bytes: usize) -> Result<()> {
        self.device
            .set_buffer_arg(index, ptr, bytes, BufferAccess::ReadWrite)
    }

    pub fn set_buffer_arg_raw_with_access(
        &mut self,
        index: u32,
        ptr: *mut u8,
        bytes: usize,
        access: BufferAccess,
    ) -> Result<()> {
        self.device.set_buffer_arg(index, ptr, bytes, access)
    }

    pub fn set_stream_arg_raw(&mut self, index: u32, shm_path: &str) -> Result<()> {
        self.device.set_stream_arg(index, shm_path)
    }

    pub fn suspend_buffer(&mut self, index: u32) -> usize {
        self.device.suspend_buffer(index)
    }

    pub fn write_to_device(&mut self) -> Result<()> {
        self.device.write_to_device()
    }

    pub fn read_from_device(&mut self) -> Result<()> {
        self.device.read_from_device()
    }

    pub fn exec(&mut self) -> Result<()> {
        self.device.exec()
    }

    pub fn pause(&mut self) -> Result<()> {
        self.device.pause()
    }

    pub fn resume(&mut self) -> Result<()> {
        self.device.resume()
    }

    pub fn finish(&mut self) -> Result<()> {
        self.device.finish()
    }

    pub fn kill(&mut self) -> Result<()> {
        self.device.kill()
    }

    pub fn is_finished(&mut self) -> Result<bool> {
        self.device.is_finished()
    }

    pub fn args_info(&self) -> Vec<RuntimeArgInfo> {
        self.device.args_info()
    }

    pub fn load_ns(&self) -> u64 {
        self.device.load_ns()
    }

    pub fn compute_ns(&self) -> u64 {
        self.device.compute_ns()
    }

    pub fn store_ns(&self) -> u64 {
        self.device.store_ns()
    }
}
