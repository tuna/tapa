//! Error types for tapa-codegen.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("task '{0}' not found in topology")]
    TaskNotFound(String),

    #[error("module already attached to task '{0}'")]
    ModuleAlreadyAttached(String),

    #[error("cannot create FSM module for lower-level task '{0}'")]
    FsmForLowerTask(String),

    #[error("RTL parse error: {0}")]
    RtlParse(#[from] tapa_rtl::ParseError),

    #[error("RTL builder error: {0}")]
    RtlBuilder(#[from] tapa_rtl::BuilderError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
