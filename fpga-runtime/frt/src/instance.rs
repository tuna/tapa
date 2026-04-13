use crate::cosim::CosimDevice;
use crate::device::{BufferAccess, Device, RuntimeArgInfo};
use crate::error::{FrtError, Result};
use crate::xrt::device::XrtDevice;
use std::marker::PhantomData;
use std::path::Path;

pub struct ReadOnly;
pub struct WriteOnly;
pub struct ReadWrite;

pub struct Buffer<T, Tag> {
    ptr: *mut T,
    count: usize,
    _tag: PhantomData<Tag>,
}

impl<T, Tag> Buffer<T, Tag> {
    pub fn size_in_bytes(&self) -> usize {
        self.count * std::mem::size_of::<T>()
    }

    pub fn as_ptr(&self) -> *mut T {
        self.ptr
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

pub type ReadOnlyBuffer<T> = Buffer<T, ReadOnly>;
pub type WriteOnlyBuffer<T> = Buffer<T, WriteOnly>;
pub type ReadWriteBuffer<T> = Buffer<T, ReadWrite>;

impl<T> ReadOnlyBuffer<T> {
    pub fn new(slice: &[T]) -> Self {
        Self {
            ptr: slice.as_ptr().cast_mut(),
            count: slice.len(),
            _tag: PhantomData,
        }
    }
}

impl<T> WriteOnlyBuffer<T> {
    pub fn new(slice: &mut [T]) -> Self {
        Self {
            ptr: slice.as_mut_ptr(),
            count: slice.len(),
            _tag: PhantomData,
        }
    }
}

impl<T> ReadWriteBuffer<T> {
    pub fn new(slice: &mut [T]) -> Self {
        Self {
            ptr: slice.as_mut_ptr(),
            count: slice.len(),
            _tag: PhantomData,
        }
    }
}

#[derive(Clone)]
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

    pub fn set_scalar_arg(&mut self, index: u32, value: u64) -> Result<()> {
        self.device.set_scalar_arg(index, &value.to_le_bytes())
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

    pub fn set_read_only_arg<T>(&mut self, index: u32, buf: &ReadOnlyBuffer<T>) -> Result<()> {
        self.device.set_buffer_arg(
            index,
            buf.as_ptr().cast::<u8>(),
            buf.size_in_bytes(),
            BufferAccess::ReadOnly,
        )
    }

    pub fn set_write_only_arg<T>(&mut self, index: u32, buf: &WriteOnlyBuffer<T>) -> Result<()> {
        self.device.set_buffer_arg(
            index,
            buf.as_ptr().cast::<u8>(),
            buf.size_in_bytes(),
            BufferAccess::WriteOnly,
        )
    }

    pub fn set_read_write_arg<T>(&mut self, index: u32, buf: &ReadWriteBuffer<T>) -> Result<()> {
        self.device.set_buffer_arg(
            index,
            buf.as_ptr().cast::<u8>(),
            buf.size_in_bytes(),
            BufferAccess::ReadWrite,
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_size_in_bytes() {
        let data = vec![1u32, 2, 3, 4];
        let buf = ReadOnlyBuffer::new(&data);
        assert_eq!(buf.size_in_bytes(), 16);
    }

    #[test]
    fn simulator_enum_variants() {
        assert!(matches!(Simulator::Verilator, Simulator::Verilator));
        assert!(matches!(
            Simulator::Xsim { legacy: false },
            Simulator::Xsim { .. }
        ));
    }
}
