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
    fn write_to_device(&mut self) -> Result<()>;
    fn read_from_device(&mut self) -> Result<()>;
    fn exec(&mut self) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
    fn kill(&mut self) -> Result<()>;
    /// Whether the launched compute has completed.
    ///
    /// Contract shared by every backend: `false` until [`Self::exec`]
    /// has launched something (a fresh instance is not "finished"),
    /// `true` once the launched compute completed or the instance was
    /// killed. `frt_instance_close` relies on this to decide between
    /// a clean drop and a [`Self::kill`].
    fn is_finished(&mut self) -> Result<bool>;
    fn load_ns(&self) -> u64;
    fn compute_ns(&self) -> u64;
    fn store_ns(&self) -> u64;
}
