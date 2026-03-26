use crate::error::Result;
use crate::metadata::{ArgKind, KernelSpec};
use frt_shm::{MmapSegment, SharedMemoryQueue};
use std::collections::HashMap;

pub struct CosimContext {
    pub buffers: HashMap<String, MmapSegment>,
    pub streams: HashMap<String, SharedMemoryQueue>,
    pub stream_path_overrides: HashMap<String, String>,
    pub base_addresses: HashMap<String, u64>,
}

impl CosimContext {
    pub fn new(spec: &KernelSpec) -> Result<Self> {
        let mut buffers = HashMap::new();
        let mut streams = HashMap::new();
        let mut base_addresses = HashMap::new();

        let mut mmap_idx = 1u64;
        for arg in &spec.args {
            match &arg.kind {
                ArgKind::Mmap { .. } => {
                    let size = 4 * 1024 * 1024usize;
                    let seg = MmapSegment::create(&arg.name, size)?;
                    let base = mmap_idx * 0x1000_0000u64;
                    buffers.insert(arg.name.clone(), seg);
                    base_addresses.insert(arg.name.clone(), base);
                    mmap_idx += 1;
                }
                ArgKind::Stream { width, depth, .. } => {
                    let width_bytes = (*width).div_ceil(8);
                    let q = SharedMemoryQueue::create(&arg.name, *depth, width_bytes)?;
                    streams.insert(arg.name.clone(), q);
                }
                ArgKind::Scalar { .. } => {}
            }
        }

        Ok(Self {
            buffers,
            streams,
            stream_path_overrides: HashMap::new(),
            base_addresses,
        })
    }

    pub fn bind_stream_path(&mut self, name: &str, path: &str) -> Result<()> {
        if !self.streams.contains_key(name) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown stream arg '{name}'"),
            )
            .into());
        }
        if path.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty stream shm path",
            )
            .into());
        }
        self.stream_path_overrides
            .insert(name.to_owned(), path.to_owned());
        Ok(())
    }

    pub fn dpi_config_json(&self) -> String {
        let mut buf_map = serde_json::Map::new();
        for (name, seg) in &self.buffers {
            buf_map.insert(
                name.clone(),
                serde_json::json!({
                    "path": seg.path().to_string_lossy(),
                    "size_bytes": seg.len(),
                    "base_addr": self.base_addresses.get(name).copied().unwrap_or(0),
                }),
            );
        }

        let mut stream_map = serde_json::Map::new();
        for (name, q) in &self.streams {
            let stream_path = self
                .stream_path_overrides
                .get(name)
                .cloned()
                .unwrap_or_else(|| q.path().to_string_lossy().to_string());
            stream_map.insert(
                name.clone(),
                serde_json::Value::String(stream_path),
            );
        }

        serde_json::json!({
            "buffers": buf_map,
            "streams": stream_map,
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{ArgSpec, Mode, StreamDir};
    use std::collections::HashMap;

    fn make_spec() -> KernelSpec {
        KernelSpec {
            top_name: "vadd".into(),
            mode: Mode::Hls,
            part_num: None,
            verilog_files: vec![],
            scalar_register_map: HashMap::new(),
            args: vec![
                ArgSpec {
                    name: "a".into(),
                    id: 0,
                    kind: ArgKind::Mmap {
                        data_width: 512,
                        addr_width: 64,
                    },
                },
                ArgSpec {
                    name: "s".into(),
                    id: 1,
                    kind: ArgKind::Stream {
                        width: 32,
                        depth: 8,
                        dir: StreamDir::In,
                        protocol: crate::metadata::StreamProtocol::ApFifo,
                    },
                },
            ],
        }
    }

    #[test]
    fn creates_shm_resources() {
        let ctx = CosimContext::new(&make_spec()).expect("new");
        assert!(ctx.buffers.contains_key("a"));
        assert!(ctx.streams.contains_key("s"));
    }

    #[test]
    fn dpi_config_json_is_valid() {
        let ctx = CosimContext::new(&make_spec()).expect("new");
        let json = ctx.dpi_config_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert!(v["buffers"]["a"]["path"].is_string());
        assert!(v["buffers"]["a"]["size_bytes"].is_number());
        assert!(v["streams"]["s"].is_string());
    }

    #[test]
    fn base_addresses_are_unique() {
        let ctx = CosimContext::new(&make_spec()).expect("new");
        let addrs: Vec<_> = ctx.base_addresses.values().collect();
        assert_eq!(addrs.len(), 1);
        assert_eq!(*addrs[0], 0x1000_0000u64);
    }
}
