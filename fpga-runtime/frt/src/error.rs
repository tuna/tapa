use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum FrtError {
    #[error("no device found for {path}")]
    NoDevice { path: PathBuf },
    #[error("xclbin/xo metadata parse error: {0}")]
    MetadataParse(String),
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
