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

/// Kernel-argument category as it crosses the C ABI. The discriminants
/// are the wire values; cbindgen exports this enum into `c_api.h`, making
/// this type the single source of the integer contract.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeArgCategory {
    Scalar = 0,
    Mmap = 1,
    Stream = 2,
    Streams = 3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeArgInfo {
    pub index: u32,
    pub name: String,
    pub type_name: String,
    pub category: RuntimeArgCategory,
}

/// Collect per-runtime `RuntimeArgInfo` entries into the wire shape shared by
/// both runtimes: a single vector sorted by argument index.
pub(crate) fn sorted_args_info(
    infos: impl IntoIterator<Item = RuntimeArgInfo>,
) -> Vec<RuntimeArgInfo> {
    let mut args: Vec<_> = infos.into_iter().collect();
    args.sort_by_key(|a| a.index);
    args
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
    /// Whether the launched compute has completed.
    ///
    /// Contract shared by every backend: `false` until [`Self::exec`]
    /// has launched something (a fresh instance is not "finished"),
    /// `true` once the launched compute completed or the instance was
    /// killed. `frt_instance_close` relies on this to decide between
    /// a clean drop and a [`Self::kill`].
    fn is_finished(&mut self) -> Result<bool>;
    fn args_info(&self) -> Vec<RuntimeArgInfo>;
    fn load_ns(&self) -> u64;
    fn compute_ns(&self) -> u64;
    fn store_ns(&self) -> u64;
}
