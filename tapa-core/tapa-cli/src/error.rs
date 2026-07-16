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

    #[error(
        "`{path}` was written by a different tapa version \
         (state schema {found}, this tapa expects v{expected}); \
         re-run `tapa analyze` to regenerate the work directory"
    )]
    StaleWorkState {
        path: PathBuf,
        found: String,
        expected: u32,
    },

    #[error("invalid CLI argument: {0}")]
    InvalidArg(String),

    #[error("invalid remote config in `{path}`: {message}")]
    RemoteConfigParse { path: PathBuf, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Schema(#[from] tapa_ir::ParseError),

    #[error(transparent)]
    Xilinx(#[from] tapa_xilinx::XilinxError),

    #[error("`tapacc` resource `{name}` not found; searched: {searched}")]
    TapaccNotFound { name: String, searched: String },

    #[error("`tapacc` binary `{path}` is not executable: {reason}")]
    TapaccNotExecutable { path: PathBuf, reason: String },

    #[error("`tapacc` exited {code}:\n{stderr}")]
    TapaccFailed { code: i32, stderr: String },

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

impl CliError {
    /// Process exit status that preserves child-tool failures and uses the
    /// conventional status 2 for command-line usage errors.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::TapaccFailed { code, .. }
            | Self::Xilinx(tapa_xilinx::XilinxError::ToolFailure { code, .. }) => {
                normalize_child_exit_code(*code)
            }
            Self::ClapParse { .. }
            | Self::UnknownSubcommand { .. }
            | Self::OrphanFlag { .. }
            | Self::WorkDir(..)
            | Self::MissingState { .. }
            | Self::StaleWorkState { .. }
            | Self::InvalidArg(..)
            | Self::RemoteConfigParse { .. }
            | Self::TapaccNotFound { .. }
            | Self::TapaccNotExecutable { .. } => 2,
            Self::Io(..) | Self::Json(..) | Self::Schema(..) | Self::Xilinx(..) => 1,
        }
    }
}

fn normalize_child_exit_code(code: i32) -> u8 {
    match u8::try_from(code) {
        Ok(0) | Err(_) => 1,
        Ok(code) => code,
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_errors_exit_two() {
        assert_eq!(
            CliError::UnknownSubcommand {
                token: "bogus".to_owned(),
                pos: 1,
            }
            .exit_code(),
            2
        );
        assert_eq!(CliError::InvalidArg("bad value".to_owned()).exit_code(), 2);
    }

    #[test]
    fn child_tool_exit_status_is_preserved() {
        assert_eq!(
            CliError::TapaccFailed {
                code: 7,
                stderr: "failed".to_owned(),
            }
            .exit_code(),
            7
        );
        assert_eq!(
            CliError::Xilinx(tapa_xilinx::XilinxError::ToolFailure {
                program: "vivado".to_owned(),
                code: 3,
                stderr: "failed".to_owned(),
            })
            .exit_code(),
            3
        );
        assert_eq!(
            CliError::TapaccFailed {
                code: -1,
                stderr: "signal".to_owned(),
            }
            .exit_code(),
            1
        );
    }
}
