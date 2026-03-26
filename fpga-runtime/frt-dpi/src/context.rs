use frt_shm::{MmapSegment, SharedMemoryQueue};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct BufferEntry {
    pub path: String,
    pub size_bytes: usize,
    #[serde(default)]
    pub base_addr: u64,
}

#[derive(Debug, Deserialize)]
pub struct DpiConfig {
    pub buffers: HashMap<String, BufferEntry>,
    pub streams: HashMap<String, String>,
}

pub struct DpiContext {
    pub buffers: HashMap<String, (MmapSegment, u64)>,
    pub streams: HashMap<String, std::sync::Mutex<SharedMemoryQueue>>,
}

#[derive(Debug, thiserror::Error)]
pub enum DpiError {
    #[error("TAPA_DPI_CONFIG not set")]
    EnvMissing,
    #[error("TAPA_DPI_CONFIG parse error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("shm open error: {0}")]
    Io(#[from] std::io::Error),
}

impl DpiContext {
    pub fn from_env() -> Result<Self, DpiError> {
        let raw = std::env::var("TAPA_DPI_CONFIG").map_err(|_| DpiError::EnvMissing)?;
        let cfg: DpiConfig = serde_json::from_str(&raw)?;

        let mut buffers = HashMap::new();
        for (name, entry) in cfg.buffers {
            let seg = MmapSegment::open(&entry.path, entry.size_bytes)?;
            buffers.insert(name, (seg, entry.base_addr));
        }

        let mut streams = HashMap::new();
        for (name, path) in cfg.streams {
            let q = SharedMemoryQueue::open(&path, 32)?;
            streams.insert(name, std::sync::Mutex::new(q));
        }

        Ok(Self { buffers, streams })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_config() {
        let json = r#"{
          "buffers": { "a": { "path": "/tmp/buf_a", "size_bytes": 4096 } },
          "streams": { "s": "/tmp/stream_s" }
        }"#;
        let cfg: DpiConfig = serde_json::from_str(json).expect("json");
        assert_eq!(cfg.buffers["a"].path, "/tmp/buf_a");
        assert_eq!(cfg.buffers["a"].size_bytes, 4096);
        assert_eq!(cfg.streams["s"], "/tmp/stream_s");
    }

    #[test]
    fn from_env_missing_var() {
        std::env::remove_var("TAPA_DPI_CONFIG");
        assert!(DpiContext::from_env().is_err());
    }

    #[test]
    fn from_env_malformed_json() {
        std::env::set_var("TAPA_DPI_CONFIG", "not-json");
        assert!(DpiContext::from_env().is_err());
        std::env::remove_var("TAPA_DPI_CONFIG");
    }
}
