//! Error types for `GraphIR` parsing.

/// Errors produced when parsing a `graphir.json` payload.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("`GraphIR` parse error at {path}: {message}")]
    Schema { path: String, message: String },

    #[error("JSON syntax error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("zlib decompress error: {0}")]
    Zlib(String),
}
