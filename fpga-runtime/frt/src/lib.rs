mod aiesim;
mod cosim;
mod device;
mod dl;
mod error;
mod ffi;
mod host_frames;
mod instance;
mod ring;
mod shm_ffi;
mod vrt;
mod xrt;

pub use device::{BufferAccess, Device};
pub use error::{FrtError, Result};
pub use instance::{open, Simulator};

pub(crate) use frt_shm::env_bool;

/// Nanoseconds elapsed since `started`, saturating at `u64::MAX`.
pub(crate) fn elapsed_ns(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
