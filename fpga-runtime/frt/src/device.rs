use crate::error::Result;

/// What the *kernel* does with a buffer, which is what decides the
/// transfers: a buffer the kernel reads has to be loaded from the host
/// first, one it writes has to be stored back.
///
/// The discriminants are the wire values; cbindgen exports this enum into
/// `c_api.h`, so tapa-lib names these same cases rather than declaring a
/// parallel enum in the host's own perspective and inverting twice.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferAccess {
    PlaceHolder = 0,
    ReadOnly = 1,
    WriteOnly = 2,
    ReadWrite = 3,
}

impl BufferAccess {
    /// The wire value, or `None` when the integer names no case.
    pub fn from_wire(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::PlaceHolder),
            1 => Some(Self::ReadOnly),
            2 => Some(Self::WriteOnly),
            3 => Some(Self::ReadWrite),
            _ => None,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::BufferAccess;

    #[test]
    fn wire_values_name_the_kernels_view_of_a_buffer() {
        // tapa-lib sends these through `frt_instance_set_buffer_arg_typed`
        // as plain integers; the pairing is the whole contract.
        assert_eq!(BufferAccess::from_wire(0), Some(BufferAccess::PlaceHolder));
        assert_eq!(BufferAccess::from_wire(1), Some(BufferAccess::ReadOnly));
        assert_eq!(BufferAccess::from_wire(2), Some(BufferAccess::WriteOnly));
        assert_eq!(BufferAccess::from_wire(3), Some(BufferAccess::ReadWrite));
        assert_eq!(BufferAccess::from_wire(4), None);
        assert_eq!(BufferAccess::from_wire(-1), None);
    }

    #[test]
    fn transfers_follow_what_the_kernel_does() {
        // A buffer the kernel reads has to be loaded from the host first;
        // one it writes has to be stored back. Getting this backwards is
        // the failure the old double inversion invited.
        assert!(BufferAccess::ReadOnly.loads_from_host());
        assert!(!BufferAccess::ReadOnly.stores_to_host());
        assert!(BufferAccess::WriteOnly.stores_to_host());
        assert!(!BufferAccess::WriteOnly.loads_from_host());
        assert!(BufferAccess::ReadWrite.loads_from_host());
        assert!(BufferAccess::ReadWrite.stores_to_host());
        assert!(!BufferAccess::PlaceHolder.loads_from_host());
        assert!(!BufferAccess::PlaceHolder.stores_to_host());
    }
}
