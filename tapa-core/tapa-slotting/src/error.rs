//! Error types for tapa-slotting.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SlottingError {
    #[error("empty source input")]
    EmptySource,

    #[error("function '{0}' not found")]
    FunctionNotFound(String),

    #[error("invalid port index in '{0}': must be a numeric index")]
    InvalidPortIndex(String),

    #[error("unknown port category: {0}")]
    UnknownPortCategory(String),

    #[error("tree-sitter error: {0}")]
    TreeSitter(String),
}
