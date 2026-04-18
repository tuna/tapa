//! Error types for design.json parsing.

use thiserror::Error;

/// Errors from parsing design.json.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("JSON parse error: {0}")]
    Json(String),
}
