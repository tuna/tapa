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
    pub fn open_from_config(spec: &KernelSpec, config_json: &str) -> Result<Self> {
        let config: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut buffers = HashMap::new();
        let mut base_addresses = HashMap::new();

        for arg in &spec.args {
            if let ArgKind::Mmap { .. } = &arg.kind {
                let entry = &config["buffers"][&arg.name];
                let path = entry["path"].as_str().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("missing path for buffer '{}' in dpi_config.json", arg.name),
                    )
                })?;
                let size = entry["size_bytes"].as_u64().unwrap_or(0) as usize;
                let base = entry["base_addr"].as_u64().unwrap_or(0);
                let seg = MmapSegment::open(path, size)?;
                buffers.insert(arg.name.clone(), seg);
                base_addresses.insert(arg.name.clone(), base);
            }
        }

        Ok(Self {
            buffers,
            streams: HashMap::new(),
            stream_path_overrides: HashMap::new(),
            base_addresses,
        })
    }

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
                    // The host enqueues elem_t<T> = { T val; bool eot; } so each
                    // element is sizeof(T) + 1 bytes regardless of protocol.
                    // For AXIS the extra byte carries TLAST; for ApFifo it carries
                    // the EOS bit that maps to the MSB of the dout/din FIFO port.
                    let width_bytes = (*width).div_ceil(8) + 1;
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

    fn resize_buffer_with<F>(&mut self, name: &str, size: usize, create: F) -> Result<()>
    where
        F: FnOnce(&str, usize) -> std::io::Result<MmapSegment>,
    {
        let seg = {
            let Some(old) = self.buffers.get(name) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("unknown buffer arg '{name}'"),
                )
                .into());
            };
            let mut seg = create(name, size.max(1))?;
            let len = old.len().min(seg.len());
            if len > 0 {
                seg.as_mut_slice()[..len].copy_from_slice(&old.as_slice()[..len]);
            }
            seg
        };
        self.buffers.insert(name.to_owned(), seg);
        Ok(())
    }

    pub fn resize_buffer(&mut self, name: &str, size: usize) -> Result<()> {
        self.resize_buffer_with(name, size, MmapSegment::create)
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
                serde_json::json!({
                    "path": stream_path,
                    "dpi_width_bytes": q.width(),
                }),
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
            tcl_files: vec![],
            xci_files: vec![],
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
        assert!(v["streams"]["s"]["path"].is_string());
        assert_eq!(v["streams"]["s"]["dpi_width_bytes"].as_u64(), Some(5));
    }

    #[test]
    fn base_addresses_are_unique() {
        let ctx = CosimContext::new(&make_spec()).expect("new");
        let addrs: Vec<_> = ctx.base_addresses.values().collect();
        assert_eq!(addrs.len(), 1);
        assert_eq!(*addrs[0], 0x1000_0000u64);
    }

    #[test]
    fn resize_buffer_updates_config_size() {
        let mut ctx = CosimContext::new(&make_spec()).expect("new");
        ctx.resize_buffer("a", 5 * 1024 * 1024).expect("resize");
        assert_eq!(ctx.buffers["a"].len(), 5 * 1024 * 1024);

        let json = ctx.dpi_config_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(
            v["buffers"]["a"]["size_bytes"].as_u64(),
            Some(5 * 1024 * 1024)
        );
    }

    #[test]
    fn resize_buffer_failure_preserves_existing_buffer() {
        let mut ctx = CosimContext::new(&make_spec()).expect("new");
        ctx.buffers.get_mut("a").expect("buffer").as_mut_slice()[0] = 0xaa;
        let original_path = ctx.buffers["a"].path().to_owned();

        let err = ctx
            .resize_buffer_with("a", 5 * 1024 * 1024, |_name, _size| {
                Err(std::io::Error::other("injected failure"))
            })
            .expect_err("resize should fail");
        assert!(err.to_string().contains("injected failure"));
        assert_eq!(ctx.buffers["a"].len(), 4 * 1024 * 1024);
        assert_eq!(ctx.buffers["a"].path(), original_path.as_path());
        assert_eq!(ctx.buffers["a"].as_slice()[0], 0xaa);
    }
}
