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

pub type Result<T> = std::result::Result<T, XilinxError>;

#[cfg(test)]
pub(crate) fn variant_tag(e: &XilinxError) -> &'static str {
    // Exhaustive match — adding a new variant without extending this
    // arm fails the compile under the workspace's deny(wildcard) lint.
    match e {
        XilinxError::Config { .. } => "Config",
        XilinxError::ToolFailure { .. } => "ToolFailure",
        XilinxError::ToolTimeout { .. } => "ToolTimeout",
        XilinxError::ToolSignaled { .. } => "ToolSignaled",
        XilinxError::SshConnect { .. } => "SshConnect",
        XilinxError::SshMuxLost { .. } => "SshMuxLost",
        XilinxError::RemoteTransfer(_) => "RemoteTransfer",
        XilinxError::DeviceConfig { .. } => "DeviceConfig",
        XilinxError::PlatformNotFound(_) => "PlatformNotFound",
        XilinxError::HlsReportParse(_) => "HlsReportParse",
        XilinxError::HlsRetryExhausted { .. } => "HlsRetryExhausted",
        XilinxError::InvalidFrequency(_) => "InvalidFrequency",
        XilinxError::VitisLink(_) => "VitisLink",
        XilinxError::TimingSummaryParse(_) => "TimingSummaryParse",
        XilinxError::KernelXml(_) => "KernelXml",
        XilinxError::XoRedaction(_) => "XoRedaction",
        XilinxError::Template(_) => "Template",
        XilinxError::Io(_) => "Io",
        XilinxError::Zip(_) => "Zip",
        XilinxError::Xml(_) => "Xml",
        XilinxError::Json(_) => "Json",
    }
}

#[cfg(test)]
mod tests {
    //! Every variant must have a non-empty `Display`. `variant_tag`
    //! is an exhaustive match, so a new variant that is not added to
    //! the table below fails the compile there first.

    use super::*;

    fn all_variants() -> Vec<XilinxError> {
        vec![
            XilinxError::Config {
                path: Utf8PathBuf::from("p"),
                source: serde_yaml::from_str::<serde_yaml::Value>(": \\").unwrap_err(),
            },
            XilinxError::ToolFailure {
                program: "p".into(),
                code: 1,
                stderr: "e".into(),
            },
            XilinxError::ToolTimeout {
                program: "p".into(),
                timeout_secs: 1,
            },
            XilinxError::ToolSignaled {
                program: "p".into(),
            },
            XilinxError::SshConnect {
                host: "h".into(),
                detail: "d".into(),
            },
            XilinxError::SshMuxLost { detail: "d".into() },
            XilinxError::RemoteTransfer("e".into()),
            XilinxError::DeviceConfig {
                path: Utf8PathBuf::from("p"),
                detail: "d".into(),
            },
            XilinxError::PlatformNotFound(Utf8PathBuf::from("p")),
            XilinxError::HlsReportParse("e".into()),
            XilinxError::HlsRetryExhausted { attempts: 1 },
            XilinxError::InvalidFrequency("e".into()),
            XilinxError::VitisLink("e".into()),
            XilinxError::TimingSummaryParse("e".into()),
            XilinxError::KernelXml("e".into()),
            XilinxError::XoRedaction("e".into()),
            XilinxError::Template("e".into()),
            XilinxError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "e")),
            XilinxError::Zip(zip::result::ZipError::InvalidArchive("e")),
            XilinxError::Xml(quick_xml::Reader::from_str("<a").read_event().unwrap_err()),
            XilinxError::Json(serde_json::from_str::<serde_json::Value>("x").unwrap_err()),
        ]
    }

    #[test]
    fn every_variant_has_nonempty_display_and_a_tag() {
        for e in all_variants() {
            assert!(!e.to_string().is_empty(), "empty Display for {e:?}");
            assert!(!variant_tag(&e).is_empty());
        }
    }
}
