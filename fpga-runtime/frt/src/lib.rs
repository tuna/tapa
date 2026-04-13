pub mod cosim;
pub mod device;
pub mod error;
pub mod ffi;
pub mod instance;
mod shm_ffi;
pub mod xrt;

pub use error::{FrtError, Result};
pub use instance::{Instance, ReadOnlyBuffer, ReadWriteBuffer, Simulator, WriteOnlyBuffer};

pub(crate) use frt_shm::env_bool;
