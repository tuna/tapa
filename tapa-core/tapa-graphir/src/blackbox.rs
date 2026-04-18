//! `BlackBox` file references (base64-encoded, zlib-compressed).

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::ParseError;

/// A blackbox file bundled in the `GraphIR` project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlackBox {
    /// Relative file path.
    pub path: String,
    /// Base64-encoded, zlib-compressed binary content.
    pub base64: String,
}

impl BlackBox {
    /// Decode and decompress the stored content.
    pub fn get_binary(&self) -> Result<Vec<u8>, ParseError> {
        let compressed = base64::engine::general_purpose::STANDARD
            .decode(&self.base64)?;
        let mut decoder = flate2::read::ZlibDecoder::new(&compressed[..]);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut out)
            .map_err(|e| ParseError::Zlib(e.to_string()))?;
        Ok(out)
    }

    /// Create from raw binary content.
    #[must_use]
    pub fn from_binary(path: String, data: &[u8]) -> Self {
        use flate2::write::ZlibEncoder;
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).expect("zlib compression should not fail on in-memory data");
        let compressed = encoder.finish().expect("zlib finish should not fail");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);
        Self { path, base64: b64 }
    }
}
