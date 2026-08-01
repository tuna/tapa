use frt_shm::{MmapSegment, SharedMemoryQueue};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Deserialize)]
pub struct BufferEntry {
    pub path: String,
    #[serde(default)]
    pub base_addr: u64,
}

#[derive(Debug, Deserialize)]
pub struct StreamEntry {
    pub path: String,
    /// Byte width of the DPI-side payload, always emitted by `frt-cosim`.
    pub dpi_width_bytes: usize,
}

#[derive(Debug, Deserialize)]
pub struct DpiConfig {
    pub buffers: HashMap<String, BufferEntry>,
    pub streams: HashMap<String, StreamEntry>,
}

pub struct DpiStream {
    pub inner: Mutex<DpiStreamInner>,
    pub dpi_width_bytes: usize,
}

pub struct DpiStreamInner {
    pub queue: SharedMemoryQueue,
    pub last_istream_valid: bool,
    pub last_ostream_ready: bool,
}

pub struct DpiContext {
    pub buffers: HashMap<String, (MmapSegment, u64)>,
    pub streams: HashMap<String, DpiStream>,
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
        let raw =
            std::env::var(frt_shm::env::TAPA_DPI_CONFIG).map_err(|_e| DpiError::EnvMissing)?;
        let cfg: DpiConfig = serde_json::from_str(&raw)?;

        let mut buffers = HashMap::new();
        for (name, entry) in cfg.buffers {
            let seg = MmapSegment::open(&entry.path)?;
            buffers.insert(name, (seg, entry.base_addr));
        }

        let mut streams = HashMap::new();
        for (name, entry) in cfg.streams {
            let q = SharedMemoryQueue::open(&entry.path)?;
            if frt_shm::env_bool(frt_shm::env::FRT_STREAM_DEBUG) {
                eprintln!(
                    "frt-dpi: stream '{name}' path={} depth={} width={} dpi_width={}",
                    entry.path,
                    q.depth(),
                    q.width(),
                    entry.dpi_width_bytes,
                );
            }
            streams.insert(
                name,
                DpiStream {
                    inner: Mutex::new(DpiStreamInner {
                        queue: q,
                        last_istream_valid: false,
                        last_ostream_ready: false,
                    }),
                    dpi_width_bytes: entry.dpi_width_bytes,
                },
            );
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
          "streams": {
            "s": { "path": "/tmp/stream_s", "dpi_width_bytes": 5 }
          }
        }"#;
        let cfg: DpiConfig = serde_json::from_str(json).expect("json");
        assert_eq!(cfg.buffers["a"].path, "/tmp/buf_a");
        assert_eq!(cfg.streams["s"].path, "/tmp/stream_s");
        assert_eq!(cfg.streams["s"].dpi_width_bytes, 5);
    }

    #[test]
    fn from_env_missing_var() {
        std::env::remove_var(frt_shm::env::TAPA_DPI_CONFIG);
        assert!(DpiContext::from_env().is_err());
    }

    #[test]
    fn from_env_malformed_json() {
        std::env::set_var(frt_shm::env::TAPA_DPI_CONFIG, "not-json");
        assert!(DpiContext::from_env().is_err());
        std::env::remove_var(frt_shm::env::TAPA_DPI_CONFIG);
    }
}
