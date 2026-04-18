use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum XilinxError {
    #[error("Xilinx tool not found: {0}")]
    ToolNotFound(String),

    #[error("XILINX_HLS not set and no fallback detected")]
    MissingXilinxHls,

    #[error("malformed .taparc config at {path}: {source}")]
    Config {
        path: PathBuf,
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
    ToolTimeout {
        program: String,
        timeout_secs: u64,
    },

    #[error("tool `{program}` was killed by signal")]
    ToolSignaled { program: String },

    #[error("SSH connection to {host} failed: {detail}")]
    SshConnect { host: String, detail: String },

    #[error("SSH control master lost: {detail}")]
    SshMuxLost { detail: String },

    #[error("remote file transfer failed: {0}")]
    RemoteTransfer(String),

    #[error("device config parse error at {path}: {detail}")]
    DeviceConfig { path: PathBuf, detail: String },

    #[error("platform file not found: {0}")]
    PlatformNotFound(PathBuf),

    #[error("HLS report parse error: {0}")]
    HlsReportParse(String),

    #[error("HLS synthesis failed after {attempts} attempts")]
    HlsRetryExhausted { attempts: u32 },

    #[error("kernel.xml generation failed: {0}")]
    KernelXml(String),

    #[error(".xo redaction failed: {0}")]
    XoRedaction(String),

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
mod tests {
    use super::*;

    /// Exhaustive tag list for `XilinxError`. Keep this in lock-step
    /// with the enum. The `match` in `check_tag_matches_variant` below
    /// is `wildcard_enum_match_arm`-deny-clean, so adding a new variant
    /// without extending this list fails to compile.
    const ALL_TAGS: &[&str] = &[
        "ToolNotFound",
        "MissingXilinxHls",
        "Config",
        "ToolFailure",
        "ToolTimeout",
        "ToolSignaled",
        "SshConnect",
        "SshMuxLost",
        "RemoteTransfer",
        "DeviceConfig",
        "PlatformNotFound",
        "HlsReportParse",
        "HlsRetryExhausted",
        "KernelXml",
        "XoRedaction",
        "Io",
        "Zip",
        "Xml",
        "Json",
    ];

    fn tag(e: &XilinxError) -> &'static str {
        match e {
            XilinxError::ToolNotFound(_) => "ToolNotFound",
            XilinxError::MissingXilinxHls => "MissingXilinxHls",
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
            XilinxError::KernelXml(_) => "KernelXml",
            XilinxError::XoRedaction(_) => "XoRedaction",
            XilinxError::Io(_) => "Io",
            XilinxError::Zip(_) => "Zip",
            XilinxError::Xml(_) => "Xml",
            XilinxError::Json(_) => "Json",
        }
    }

    fn all_variants() -> Vec<XilinxError> {
        vec![
            XilinxError::ToolNotFound("vitis_hls".into()),
            XilinxError::MissingXilinxHls,
            XilinxError::Config {
                path: PathBuf::from("/tmp/.taparc"),
                source: serde_yaml::from_str::<serde_yaml::Value>(":\n-\n:").unwrap_err(),
            },
            XilinxError::ToolFailure {
                program: "vivado".into(),
                code: 1,
                stderr: "boom".into(),
            },
            XilinxError::ToolTimeout {
                program: "vitis_hls".into(),
                timeout_secs: 60,
            },
            XilinxError::ToolSignaled {
                program: "vitis_hls".into(),
            },
            XilinxError::SshConnect {
                host: "x".into(),
                detail: "nope".into(),
            },
            XilinxError::SshMuxLost {
                detail: "lost".into(),
            },
            XilinxError::RemoteTransfer("tar failed".into()),
            XilinxError::DeviceConfig {
                path: PathBuf::from("/tmp/x.xpfm"),
                detail: "missing element".into(),
            },
            XilinxError::PlatformNotFound(PathBuf::from("x.xpfm")),
            XilinxError::HlsReportParse("bad".into()),
            XilinxError::HlsRetryExhausted { attempts: 3 },
            XilinxError::KernelXml("empty args".into()),
            XilinxError::XoRedaction("corrupt".into()),
            XilinxError::Io(std::io::Error::other("io")),
            XilinxError::Zip(zip::result::ZipError::FileNotFound),
            XilinxError::Xml({
                let mut r = quick_xml::Reader::from_str("<a attr='>");
                loop {
                    match r.read_event() {
                        Err(e) => break e,
                        Ok(quick_xml::events::Event::Eof) => {
                            unreachable!("malformed xml should error")
                        }
                        Ok(_) => {}
                    }
                }
            }),
            XilinxError::Json(serde_json::from_str::<serde_json::Value>("not-json").unwrap_err()),
        ]
    }

    #[test]
    fn every_variant_covered_and_displays() {
        let mut seen = std::collections::HashSet::new();
        for e in all_variants() {
            let t = tag(&e);
            assert!(!e.to_string().is_empty(), "empty Display for {t}");
            assert!(seen.insert(t), "duplicate tag in fixtures: {t}");
        }
        assert_eq!(
            seen.len(),
            ALL_TAGS.len(),
            "expected every variant covered; got {seen:?}"
        );
        for t in ALL_TAGS {
            assert!(seen.contains(t), "variant {t} missing from fixtures");
        }
    }
}
