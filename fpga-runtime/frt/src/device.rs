use crate::error::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferAccess {
    PlaceHolder,
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl BufferAccess {
    pub fn loads_from_host(self) -> bool {
        matches!(self, Self::ReadOnly | Self::ReadWrite)
    }

    pub fn stores_to_host(self) -> bool {
        matches!(self, Self::WriteOnly | Self::ReadWrite)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeArgCategory {
    Scalar,
    Mmap,
    Stream,
    Streams,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeArgInfo {
    pub index: u32,
    pub name: String,
    pub type_name: String,
    pub category: RuntimeArgCategory,
}

pub trait Device: Send {
    fn set_scalar_arg(&mut self, index: u32, value: &[u8]) -> Result<()>;
    fn set_buffer_arg(
        &mut self,
        index: u32,
        ptr: *mut u8,
        bytes: usize,
        access: BufferAccess,
    ) -> Result<()>;
    fn set_stream_arg(&mut self, index: u32, shm_path: &str) -> Result<()>;
    fn suspend_buffer(&mut self, index: u32) -> usize;
    fn write_to_device(&mut self) -> Result<()>;
    fn read_from_device(&mut self) -> Result<()>;
    fn exec(&mut self) -> Result<()>;
    fn pause(&mut self) -> Result<()> {
        Ok(())
    }
    fn resume(&mut self) -> Result<()> {
        Ok(())
    }
    fn finish(&mut self) -> Result<()>;
    fn kill(&mut self) -> Result<()>;
    fn is_finished(&mut self) -> Result<bool>;
    fn args_info(&self) -> Vec<RuntimeArgInfo>;
    fn load_ns(&self) -> u64;
    fn compute_ns(&self) -> u64;
    fn store_ns(&self) -> u64;
}
