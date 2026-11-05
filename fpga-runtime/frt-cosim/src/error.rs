#[derive(Debug, thiserror::Error)]
pub enum CosimError {
    #[error("metadata parse error: {0}")]
    Metadata(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("simulator exited with status {0}")]
    SimFailed(std::process::ExitStatus),
    #[error("simulator tool `{0}` not found")]
    ToolNotFound(String),
}

impl CosimError {
    /// Wrap a source error's message as a metadata parse error.
    pub(crate) fn metadata(source: impl std::fmt::Display) -> Self {
        Self::Metadata(source.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CosimError>;
