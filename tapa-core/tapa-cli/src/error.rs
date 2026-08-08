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

    #[error("archive error: {0}")]
    Archive(String),

    #[error("codegen error: {0}")]
    Codegen(String),

    #[error("floorplan error: {0}")]
    Floorplan(String),

    #[error("report error: {0}")]
    Report(String),

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
}

impl CliError {
    /// Process exit status that preserves child-tool failures and uses the
    /// conventional status 2 for command-line usage errors.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        // A vendor error that carries a child status forwards it. Which of
        // its variants those are is `XilinxError`'s to say, not ours.
        if let Self::Xilinx(error) = self {
            if let Some(code) = error.tool_exit_code() {
                return normalize_child_exit_code(code);
            }
        }
        match self {
            Self::TapaccFailed { code, .. } => normalize_child_exit_code(*code),
            Self::ClapParse { .. }
            | Self::WorkDir(..)
            | Self::MissingState { .. }
            | Self::StaleWorkState { .. }
            | Self::InvalidArg(..)
            | Self::RemoteConfigParse { .. }
            | Self::TapaccNotFound { .. }
            | Self::TapaccNotExecutable { .. } => 2,
            Self::Io(..)
            | Self::Json(..)
            | Self::Schema(..)
            | Self::Xilinx(..)
            | Self::Archive(..)
            | Self::Codegen(..)
            | Self::Floorplan(..)
            | Self::Report(..) => 1,
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
        assert_eq!(CliError::InvalidArg("bad value".to_owned()).exit_code(), 2);
    }

    #[test]
    fn operational_errors_exit_one() {
        assert_eq!(CliError::Archive("zip failed".to_owned()).exit_code(), 1);
        assert_eq!(CliError::Codegen("rtl parse".to_owned()).exit_code(), 1);
        assert_eq!(CliError::Report("serialize".to_owned()).exit_code(), 1);
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

    #[test]
    fn vendor_errors_without_a_child_status_exit_one() {
        // The status comes from `XilinxError::tool_exit_code`, so a vendor
        // error that never reached an exit status must not invent one.
        assert_eq!(
            CliError::Xilinx(tapa_xilinx::XilinxError::ToolTimeout {
                program: "vivado".to_owned(),
                timeout_secs: 60,
            })
            .exit_code(),
            1
        );
        assert_eq!(
            CliError::Xilinx(tapa_xilinx::XilinxError::Template("boom".to_owned())).exit_code(),
            1
        );
    }
}
