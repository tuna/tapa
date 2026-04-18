//! Error types for Verilog parsing.

use thiserror::Error;

/// Errors from parsing Verilog module headers.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("empty input")]
    EmptyInput,

    #[error("no module declaration found in input")]
    NoModuleFound,

    #[error("parse error in module `{module}`: {message}")]
    ParseFailed {
        module: String,
        message: String,
    },
}

/// Errors from builder validation.
#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("empty name is not allowed")]
    EmptyName,

    #[error("module instance has no port connections")]
    NoPortConnections,

    #[error("duplicate port name: '{0}'")]
    DuplicatePort(String),

    #[error("duplicate signal name: '{0}'")]
    DuplicateSignal(String),
}
