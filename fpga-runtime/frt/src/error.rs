use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FrtError {
    #[error("VRT error: {0}")]
    Vrt(String),
    #[error("no device found for {path}")]
    NoDevice { path: PathBuf },
    #[error("xclbin/xo metadata parse error: {0}")]
    MetadataParse(String),
    #[error(
        "cosim runtime library `{name}` not found; searched next to the host \
             executable, the linked library path, $TAPA_HOME, and LD_LIBRARY_PATH. \
             Set TAPA_HOME to the TAPA installation prefix (for example \
             `/opt/tapa`) and retry"
    )]
    DpiLibraryNotFound { name: String },
    #[error(
        "no XRT runtime found: {0}. Install XRT (and source its setup.sh) to \
             run a design on hardware or in hardware emulation; software \
             simulation and fast cosim do not need it"
    )]
    NoXrtRuntime(String),
    #[error("resume-from-post-sim stream binding error: {0}")]
    ResumeStreamBinding(String),
    #[error("invalid cosim runtime option: {0}")]
    CosimOption(String),
    #[error(
        "cosimulation exceeded the configured {timeout_secs}s wall-clock timeout; \
         increase -cosim_timeout_seconds or set it to 0 to disable the timeout"
    )]
    CosimTimeout { timeout_secs: u64 },
    #[error("XRT error: {0}")]
    Xrt(String),
    #[error("simulator exited with status {0}")]
    SimFailed(std::process::ExitStatus),
    #[error("shm error: {0}")]
    Shm(#[from] std::io::Error),
    #[error("cosim error: {0}")]
    Cosim(#[from] frt_cosim::error::CosimError),
}

pub type Result<T> = std::result::Result<T, FrtError>;
