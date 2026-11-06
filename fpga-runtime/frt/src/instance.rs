use crate::aiesim::AieSimDevice;
use crate::cosim::CosimDevice;
use crate::device::Device;
use crate::error::{FrtError, Result};
use crate::xrt::device::XrtDevice;
use std::path::Path;

#[derive(Clone, Debug)]
pub enum Simulator {
    Verilator,
    Xsim { legacy: bool },
}

/// How a bitstream runs, as decided by what kind of file it is.
///
/// The one place that decision is made: the C++ host used to sniff the
/// same extensions to pick which flags to forward, and the FFI layer
/// sniffed them again to pick a constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionMode {
    /// A SLASH `.vbin` on a V80 through VRT.
    Vrt,
    /// An `.xclbin` on real hardware or an XRT emulator.
    Hardware,
    /// An `.xo`/`.zip` package driven by a simulator.
    Cosim,
    /// A design's AI Engine graph in a simulator, from the package directory
    /// `tapa compile --aie-sim` builds.
    AieSim,
}

impl ExecutionMode {
    pub fn of(path: &Path) -> Result<Self> {
        if AieSimDevice::is_package(path) {
            return Ok(Self::AieSim);
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("vbin") => Ok(Self::Vrt),
            Some("xclbin") => Ok(Self::Hardware),
            Some("xo" | "zip") => Ok(Self::Cosim),
            _ => Err(FrtError::NoDevice {
                path: path.to_owned(),
            }),
        }
    }
}

/// Open `path`, letting its kind decide the backend. `sim` is consulted
/// only for a cosim package; hardware ignores it.
pub fn open(path: impl AsRef<Path>, sim: &Simulator) -> Result<Box<dyn Device>> {
    let path = path.as_ref();
    match ExecutionMode::of(path)? {
        ExecutionMode::Vrt => crate::vrt::open(path),
        ExecutionMode::Hardware => Ok(Box::new(XrtDevice::open(path)?)),
        ExecutionMode::Cosim => Ok(Box::new(CosimDevice::open(path, sim)?)),
        ExecutionMode::AieSim => Ok(Box::new(AieSimDevice::open(path)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionMode;
    use std::path::Path;

    #[test]
    fn the_file_kind_decides_the_backend() {
        assert_eq!(
            ExecutionMode::of(Path::new("k.vbin")).expect("vbin"),
            ExecutionMode::Vrt
        );
        assert_eq!(
            ExecutionMode::of(Path::new("k.xclbin")).expect("xclbin"),
            ExecutionMode::Hardware
        );
        assert_eq!(
            ExecutionMode::of(Path::new("k.xo")).expect("xo"),
            ExecutionMode::Cosim
        );
        assert_eq!(
            ExecutionMode::of(Path::new("k.zip")).expect("zip"),
            ExecutionMode::Cosim
        );
    }

    #[test]
    fn an_unrunnable_path_names_itself() {
        let err = ExecutionMode::of(Path::new("kernel.bit")).expect_err("not runnable");
        assert!(err.to_string().contains("kernel.bit"), "{err}");
    }
}
