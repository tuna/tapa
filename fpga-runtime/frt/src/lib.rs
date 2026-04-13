pub mod cosim;
pub mod device;
pub mod error;
pub mod ffi;
pub mod instance;
mod shm_ffi;
pub mod xrt;

pub use error::{FrtError, Result};
pub use instance::{Instance, ReadOnlyBuffer, ReadWriteBuffer, Simulator, WriteOnlyBuffer};

pub(crate) fn env_bool(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}
