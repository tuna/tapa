#[derive(Debug, thiserror::Error)]
pub enum CosimError {
    #[error("metadata parse error: {0}")]
    Metadata(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("simulator exited with status {0}")]
    SimFailed(std::process::ExitStatus),
    #[error("tool not found: {0}")]
    ToolNotFound(String),
}

pub type Result<T> = std::result::Result<T, CosimError>;
