use super::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir};
use crate::error::{CosimError, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Deserialize)]
struct GraphYaml {
    top: String,
    args: Vec<YamlArg>,
    #[serde(default)]
    part: Option<String>,
}

#[derive(Deserialize)]
struct YamlArg {
    name: String,
    id: u32,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_depth")]
    depth: u32,
    #[serde(default = "default_addr_width")]
    addr_width: u32,
    dir: Option<String>,
}

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
    let g: GraphYaml = serde_yaml::from_str(yaml).map_err(|e| CosimError::Metadata(e.to_string()))?;
    let args = g
        .args
        .into_iter()
        .map(|a| {
            let kind = match a.kind.as_str() {
                "scalar" => ArgKind::Scalar { width: a.width },
                "mmap" => ArgKind::Mmap {
                    data_width: a.width,
                    addr_width: a.addr_width,
                },
                "stream" => {
                    let dir = match a.dir.as_deref().unwrap_or("in") {
                        "out" => StreamDir::Out,
                        _ => StreamDir::In,
                    };
                    ArgKind::Stream {
                        width: a.width,
                        depth: a.depth,
                        dir,
                    }
                }
                k => return Err(CosimError::Metadata(format!("unknown arg type: {k}"))),
            };
            Ok(ArgSpec {
                name: a.name,
                id: a.id,
                kind,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(KernelSpec {
        top_name: g.top,
        mode: Mode::Hls,
        args,
        part_num: g.part,
        verilog_files: vec![],
        scalar_register_map: HashMap::new(),
    })
}
