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
