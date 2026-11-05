pub mod cosim;
pub mod device;
mod dl;
pub mod error;
pub mod ffi;
pub mod instance;
mod shm_ffi;
pub mod vrt;
pub mod xrt;

pub use error::{FrtError, Result};
pub use instance::{Instance, Simulator};

pub(crate) use frt_shm::env_bool;

/// Nanoseconds elapsed since `started`, saturating at `u64::MAX`.
pub(crate) fn elapsed_ns(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
