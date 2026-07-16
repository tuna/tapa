//! Error types for task-graph parsing.

/// Errors produced when parsing a task-graph payload.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON parse error at {path}: {message}")]
    Schema { path: String, message: String },

    #[error("JSON syntax error: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        Self::Schema {
            path: "<io>".to_string(),
            message: e.to_string(),
        }
    }
}
