mod aie_top;
mod api;
pub mod device;
pub mod metadata;
mod pl_control;
mod rings;

use std::path::Path;

use crate::device::Device;
use crate::error::Result;

/// Opens an xclbin: a PL kernel, or a top on the AI Engine, which records
/// its arguments in the xclbin instead of a kernel.
pub fn open(xclbin_path: &Path) -> Result<Box<dyn Device>> {
    let bytes = std::fs::read(xclbin_path)?;
    match metadata::parse_aie_top_arguments(&bytes)? {
        Some(arguments) => Ok(Box::new(aie_top::XrtAieTopDevice::open(
            xclbin_path,
            &bytes,
            arguments,
        )?)),
        None => Ok(Box::new(device::XrtDevice::open(xclbin_path)?)),
    }
}
