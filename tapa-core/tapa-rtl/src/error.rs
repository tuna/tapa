//! Error types for Verilog parsing.

use thiserror::Error;

/// Errors from parsing Verilog module headers.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("cannot parse Verilog: the input is empty")]
    EmptyInput,

    #[error("no module declaration found in the input")]
    NoModuleFound,

    #[error("parse error in module `{module}`: {message}")]
    ParseFailed { module: String, message: String },
}

/// Errors from builder validation.
#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("a module, instance, port, or signal name must not be empty")]
    EmptyName,

    #[error("module instance `{instance}` of `{module}` has no port connections")]
    NoPortConnections { module: String, instance: String },

    #[error("duplicate port name: `{0}`")]
    DuplicatePort(String),

    #[error("duplicate signal name: `{0}`")]
    DuplicateSignal(String),
}
