//! Error types for task-graph parsing.

/// Errors produced when parsing a task-graph payload.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON parse error at {path}: {message}")]
    Schema { path: String, message: String },

    #[error("JSON syntax error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(
        "task graph schema version {found} is newer than the supported \
         version {supported}; regenerate with a matching tapa, or upgrade \
         this tapa installation"
    )]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error(
        "task graph schema version {found} predates typed invoke-site \
         constants (version {supported}); a stale graph's constants would \
         be misread as wire names — regenerate the task graph with this tapa"
    )]
    OutdatedSchemaVersion { found: u32, supported: u32 },
}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        Self::Schema {
            path: "<io>".to_string(),
            message: e.to_string(),
        }
    }
}
