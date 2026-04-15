//! Error types for task-graph parsing.

/// Errors produced when parsing a `graph.json` payload.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON parse error at {path}: {message}")]
    Schema { path: String, message: String },

    #[error("JSON syntax error: {0}")]
    Json(#[from] serde_json::Error),
}
