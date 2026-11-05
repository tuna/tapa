mod cosim;
mod device;
mod dl;
mod error;
mod ffi;
mod instance;
mod shm_ffi;
pub mod vrt;
mod xrt;

pub use device::BufferAccess;
pub use error::{FrtError, Result};
pub use instance::{Instance, Simulator};

pub(crate) use frt_shm::env_bool;

/// Nanoseconds elapsed since `started`, saturating at `u64::MAX`.
pub(crate) fn elapsed_ns(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
