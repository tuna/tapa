use super::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir, StreamProtocol};
use crate::error::{CosimError, Result};
use std::collections::HashMap;
use std::path::Path;

fn default_width() -> u32 {
    32
}

fn default_depth() -> u32 {
    16
}

fn default_addr_width() -> u32 {
    64
}

pub fn parse_graph_yaml(yaml: &str, _verilog_dir: &Path) -> Result<KernelSpec> {
    let root: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|e| CosimError::Metadata(e.to_string()))?;
    if let Some(spec) = parse_simple_schema(&root)? {
        return Ok(spec);
    }

    parse_legacy_tapa_schema(&root)
}

fn parse_simple_schema(root: &serde_yaml::Value) -> Result<Option<KernelSpec>> {
    let Some(top) = root.get("top").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(args_yaml) = root.get("args").and_then(|v| v.as_sequence()) else {
        return Ok(None);
    };

    let mut args = Vec::with_capacity(args_yaml.len());
    for entry in args_yaml {
        let name = required_str(entry, "name")?.to_owned();
        let id = required_u32(entry, "id")?;
        let kind_name = required_str(entry, "type")?;
        let width = optional_u32(entry, "width").unwrap_or_else(default_width);
        let depth = optional_u32(entry, "depth").unwrap_or_else(default_depth);
        let addr_width = optional_u32(entry, "addr_width").unwrap_or_else(default_addr_width);

        let kind = match kind_name {
            "scalar" => ArgKind::Scalar { width },
            "mmap" => ArgKind::Mmap {
                data_width: width,
                addr_width,
            },
            "stream" => {
                let dir = match entry.get("dir").and_then(|x| x.as_str()).unwrap_or("in") {
                    "out" => StreamDir::Out,
                    _ => StreamDir::In,
                };
                ArgKind::Stream {
                    width,
                    depth,
                    dir,
                    protocol: StreamProtocol::ApFifo,
                }
            }
            other => return Err(CosimError::Metadata(format!("unknown arg type: {other}"))),
        };
        args.push(ArgSpec { name, id, kind });
    }

    Ok(Some(KernelSpec {
        top_name: top.to_owned(),
        mode: Mode::Hls,
        args,
        part_num: root
            .get("part")
            .and_then(|x| x.as_str())
            .map(ToOwned::to_owned),
        verilog_files: vec![],
        scalar_register_map: HashMap::new(),
    }))
}

fn parse_legacy_tapa_schema(root: &serde_yaml::Value) -> Result<KernelSpec> {
    let top = required_str(root, "top")?.to_owned();
    let tasks = root
        .get("tasks")
        .and_then(|x| x.as_mapping())
        .ok_or_else(|| CosimError::Metadata("graph.yaml missing tasks mapping".into()))?;
    let top_task = tasks
        .get(serde_yaml::Value::String(top.clone()))
        .ok_or_else(|| CosimError::Metadata(format!("top task '{top}' missing from tasks")))?;
    let ports = top_task
        .get("ports")
        .and_then(|x| x.as_sequence())
        .ok_or_else(|| CosimError::Metadata("top task missing ports array".into()))?;

    let mut args = Vec::new();
    let mut next_id = 0u32;
    for p in ports {
        let name = required_str(p, "name")?;
        let cat = required_str(p, "cat")?;
        let width = optional_u32(p, "width").unwrap_or_else(default_width);
        let depth = optional_u32(p, "depth").unwrap_or_else(default_depth);
        let addr_width = optional_u32(p, "addr_width").unwrap_or_else(default_addr_width);
        let chan_count = optional_u32(p, "chan_count").unwrap_or(1);

        match cat {
            "scalar" => {
                args.push(ArgSpec {
                    name: name.to_owned(),
                    id: next_id,
                    kind: ArgKind::Scalar { width },
                });
                next_id += 1;
            }
            "mmap" | "async_mmap" => {
                args.push(ArgSpec {
                    name: name.to_owned(),
                    id: next_id,
                    kind: ArgKind::Mmap {
                        data_width: width,
                        addr_width,
                    },
                });
                next_id += 1;
            }
            "mmaps" | "hmap" => {
                for i in 0..chan_count {
                    args.push(ArgSpec {
                        name: format!("{name}_{i}"),
                        id: next_id,
                        kind: ArgKind::Mmap {
                            data_width: width,
                            addr_width,
                        },
                    });
                    next_id += 1;
                }
            }
            "istream" | "ostream" => {
                let dir = if cat == "ostream" {
                    StreamDir::Out
                } else {
                    StreamDir::In
                };
                args.push(ArgSpec {
                    name: format!("{name}_s"),
                    id: next_id,
                    kind: ArgKind::Stream {
                        width,
                        depth,
                        dir,
                        protocol: StreamProtocol::ApFifo,
                    },
                });
                next_id += 1;
            }
            "istreams" | "ostreams" => {
                let dir = if cat == "ostreams" {
                    StreamDir::Out
                } else {
                    StreamDir::In
                };
                for i in 0..chan_count {
                    args.push(ArgSpec {
                        name: format!("{name}_{i}"),
                        id: next_id,
                        kind: ArgKind::Stream {
                            width,
                            depth,
                            dir: dir.clone(),
                            protocol: StreamProtocol::ApFifo,
                        },
                    });
                    next_id += 1;
                }
            }
            other => {
                return Err(CosimError::Metadata(format!(
                    "unsupported legacy port category '{other}'"
                )));
            }
        }
    }

    Ok(KernelSpec {
        top_name: top,
        mode: Mode::Hls,
        args,
        part_num: root
            .get("part")
            .and_then(|x| x.as_str())
            .map(ToOwned::to_owned),
        verilog_files: vec![],
        scalar_register_map: HashMap::new(),
    })
}

fn required_str<'a>(v: &'a serde_yaml::Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| CosimError::Metadata(format!("required string field '{key}' missing")))
}

fn required_u32(v: &serde_yaml::Value, key: &str) -> Result<u32> {
    optional_u32(v, key)
        .ok_or_else(|| CosimError::Metadata(format!("required integer field '{key}' missing")))
}

fn optional_u32(v: &serde_yaml::Value, key: &str) -> Option<u32> {
    v.get(key).and_then(|x| x.as_u64()).map(|x| x as u32)
}
