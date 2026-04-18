//! Top-level CLI error enum. Each variant carries enough context that the
//! `Display` impl is the only thing the binary needs to print — no panic,
//! no backtrace by default.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("work directory `{0}` is invalid: {1}")]
    WorkDir(PathBuf, String),

    #[error("missing required state `{name}` in `{path}`")]
    MissingState { name: String, path: PathBuf },

    #[error("invalid CLI argument: {0}")]
    InvalidArg(String),

    #[error("invalid remote config in `{path}`: {message}")]
    RemoteConfigParse { path: PathBuf, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Schema(#[from] tapa_task_graph::ParseError),

    #[error(transparent)]
    Xilinx(#[from] tapa_xilinx::XilinxError),

    #[error("`tapacc` resource `{name}` not found; searched: {searched}")]
    TapaccNotFound { name: String, searched: String },

    #[error("`tapacc` binary `{path}` is not executable: {reason}")]
    TapaccNotExecutable { path: PathBuf, reason: String },

    #[error("`tapacc` exited {code}:\n{stderr}")]
    TapaccFailed { code: i32, stderr: String },

    #[error(
        "step `{step}` (flag group `{flag_name}`) is not yet fully implemented \
         natively — the native port is incomplete; file a follow-up for this \
         branch, the Python CLI was retired in AC-8 and is no longer available"
    )]
    StepUnported { step: String, flag_name: String },

    #[error("clap parse error in `{step}`: {message}")]
    ClapParse { step: String, message: String },

    #[error("unrecognized subcommand token `{token}` at chain position {pos}")]
    UnknownSubcommand { token: String, pos: usize },

    #[error(
        "flag `{flag}` appears before its subcommand at chain position {pos}; \
         per-step flags must follow the subcommand name"
    )]
    OrphanFlag { flag: String, pos: usize },
}

pub type Result<T> = std::result::Result<T, CliError>;
