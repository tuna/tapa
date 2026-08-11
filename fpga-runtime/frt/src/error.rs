use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FrtError {
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
        "no OpenCL runtime found: {0}. Install XRT (which pulls in an \
             OpenCL ICD loader) to run a design on hardware or in hardware \
             emulation; software simulation and fast cosim do not need it"
    )]
    NoOpenClRuntime(String),
    #[error("resume-from-post-sim stream binding error: {0}")]
    ResumeStreamBinding(String),
    #[error("OpenCL error {code}: {msg}")]
    OpenCl { code: i32, msg: String },
    #[error("simulator exited with status {0}")]
    SimFailed(std::process::ExitStatus),
    #[error("shm error: {0}")]
    Shm(#[from] std::io::Error),
    #[error("cosim error: {0}")]
    Cosim(#[from] frt_cosim::error::CosimError),
}

pub type Result<T> = std::result::Result<T, FrtError>;
