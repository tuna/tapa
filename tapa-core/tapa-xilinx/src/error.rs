use camino::Utf8PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum XilinxError {
    #[error("malformed .taparc config at {path}: {source}")]
    Config {
        path: Utf8PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("tool `{program}` exited with code {code}:\n{stderr}")]
    ToolFailure {
        program: String,
        code: i32,
        stderr: String,
    },

    #[error("tool `{program}` timed out after {timeout_secs}s")]
    ToolTimeout { program: String, timeout_secs: u64 },

    #[error("tool `{program}` was killed by signal")]
    ToolSignaled { program: String },

    #[error("SSH connection to {host} failed: {detail}")]
    SshConnect { host: String, detail: String },

    #[error("SSH control master lost: {detail}")]
    SshMuxLost { detail: String },

    #[error("remote file transfer failed: {0}")]
    RemoteTransfer(String),

    #[error("device config parse error at {path}: {detail}")]
    DeviceConfig { path: Utf8PathBuf, detail: String },

    #[error("platform file not found: {0}")]
    PlatformNotFound(Utf8PathBuf),

    #[error("HLS report parse error: {0}")]
    HlsReportParse(String),

    #[error("HLS synthesis failed after {attempts} attempts")]
    HlsRetryExhausted { attempts: u32 },

    #[error("invalid implementation frequency: {0}")]
    InvalidFrequency(String),

    #[error("Vitis link error: {0}")]
    VitisLink(String),

    #[error("timing summary parse error: {0}")]
    TimingSummaryParse(String),

    #[error("kernel.xml generation failed: {0}")]
    KernelXml(String),

    #[error(".xo redaction failed: {0}")]
    XoRedaction(String),

    #[error("template render error: {0}")]
    Template(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),

    #[error(transparent)]
    Xml(#[from] quick_xml::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl XilinxError {
    /// The exit status of the vendor tool, when this error is one reporting
    /// that a tool ran and failed.
    ///
    /// Callers propagate a child's status as their own, and deciding which
    /// variants represent "a tool ran and exited non-zero" belongs to this
    /// enum rather than to whoever is converting it: a caller that pattern
    /// matches `ToolFailure` from outside silently stops preserving the
    /// status the day a variant is added or renamed.
    ///
    /// `ToolTimeout` and `ToolSignaled` deliberately return `None` — the tool
    /// never reached an exit status in either case, so there is nothing to
    /// forward.
    #[must_use]
    pub fn tool_exit_code(&self) -> Option<i32> {
        match self {
            Self::ToolFailure { code, .. } => Some(*code),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, XilinxError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_failure_carries_the_child_status() {
        let err = XilinxError::ToolFailure {
            program: "vivado".to_owned(),
            code: 3,
            stderr: String::new(),
        };
        assert_eq!(err.tool_exit_code(), Some(3));
    }

    #[test]
    fn errors_without_a_child_status_report_none() {
        // A tool that timed out or was signaled never produced an exit
        // status, so there is nothing for a caller to forward.
        assert_eq!(
            XilinxError::ToolTimeout {
                program: "vivado".to_owned(),
                timeout_secs: 60,
            }
            .tool_exit_code(),
            None
        );
        assert_eq!(
            XilinxError::ToolSignaled {
                program: "vivado".to_owned(),
            }
            .tool_exit_code(),
            None
        );
        assert_eq!(
            XilinxError::Template("boom".to_owned()).tool_exit_code(),
            None
        );
    }
}
