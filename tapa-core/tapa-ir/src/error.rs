//! Error types for task-graph parsing.

use serde::de::DeserializeOwned;

/// Errors produced when parsing a task-graph payload.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON parse error at {path}: {message}")]
    Schema { path: String, message: String },

    #[error(
        "work state version {found:?} does not match supported version {supported}; \
         re-run tapa analyze with this installation"
    )]
    WorkStateVersion { found: Option<u32>, supported: u32 },

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

/// Deserialize one complete JSON document with field-path error
/// diagnostics, rejecting content trailing the document.
pub(crate) fn parse_json<T: DeserializeOwned>(json: &str) -> Result<T, ParseError> {
    let mut de = serde_json::Deserializer::from_str(json);
    let value = serde_path_to_error::deserialize(&mut de).map_err(|e| ParseError::Schema {
        path: e.path().to_string(),
        message: e.inner().to_string(),
    })?;
    de.end()?;
    Ok(value)
}
