use super::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir, StreamProtocol};
use crate::error::{CosimError, Result};
use std::collections::HashMap;
use tapa_ir::port::{ArgCategory, Port};
use tapa_ir::TaskGraph;

/// FIFO depth assumed for every stream argument the archive declares.
///
/// The archive carries no per-port depth: [`tapa_ir::Port`] has no such
/// field, and the archive is written straight from that type. The previous
/// untyped reader looked for a `depth` key and, for exactly that reason,
/// always fell back to this value — so hardcoding it here preserves the
/// argument shape bit for bit.
const STREAM_DEPTH: u32 = 16;

/// Address width assumed for every mmap argument the archive declares.
/// Same story as [`STREAM_DEPTH`]: not in the schema, always defaulted.
const MMAP_ADDR_WIDTH: u32 = 64;

/// Project the packed task graph into the flat kernel argument list.
///
/// The top task's `ports` expand into one or more kernel arguments each,
/// numbered in declaration order — that numbering is the kernel ABI the
/// simulator binds against, so the order here is load-bearing.
pub fn spec_from_task_graph(graph: &TaskGraph) -> Result<KernelSpec> {
    let top_task = graph.tasks.get(&graph.top).ok_or_else(|| {
        CosimError::Metadata(format!("top task '{}' missing from tasks", graph.top))
    })?;

    let mut args = Vec::new();
    let mut next_id = 0u32;
    for port in &top_task.ports {
        let name = port.name.as_str();
        let width = port.width;
        let chan_count = port.chan_count.unwrap_or(1);

        match port.cat {
            ArgCategory::Scalar => {
                args.push(ArgSpec {
                    name: name.to_owned(),
                    id: next_id,
                    kind: ArgKind::Scalar { width },
                });
                next_id += 1;
            }
            // `is_mmap_like` deliberately not used: it also covers `immap` /
            // `ommap`, which this reader has never accepted (see below).
            ArgCategory::Mmap | ArgCategory::AsyncMmap => {
                args.push(ArgSpec {
                    name: name.to_owned(),
                    id: next_id,
                    kind: ArgKind::Mmap {
                        data_width: width,
                        addr_width: MMAP_ADDR_WIDTH,
                    },
                });
                next_id += 1;
            }
            ArgCategory::Istream | ArgCategory::Ostream => {
                args.push(ArgSpec {
                    name: format!("{name}_s"),
                    id: next_id,
                    kind: ArgKind::Stream {
                        width,
                        depth: STREAM_DEPTH,
                        dir: stream_dir(port),
                        protocol: StreamProtocol::ApFifo,
                    },
                });
                next_id += 1;
            }
            ArgCategory::Istreams | ArgCategory::Ostreams => {
                for i in 0..chan_count {
                    args.push(ArgSpec {
                        name: format!("{name}_{i}"),
                        id: next_id,
                        kind: ArgKind::Stream {
                            width,
                            depth: STREAM_DEPTH,
                            dir: stream_dir(port),
                            protocol: StreamProtocol::ApFifo,
                        },
                    });
                    next_id += 1;
                }
            }
            // Read-only / write-only mmaps have never been wired up here.
            // Rejecting them keeps that an explicit, loud limitation rather
            // than silently binding them as plain mmaps.
            ArgCategory::Immap | ArgCategory::Ommap => {
                return Err(CosimError::Metadata(format!(
                    "unsupported port category '{}'",
                    port.cat.as_str()
                )));
            }
        }
    }

    Ok(KernelSpec {
        top_name: graph.top.clone(),
        mode: Mode::Hls,
        args,
        // Recovered by the caller from the state file's flow settings, the
        // one place the resolved part number is written.
        part_num: None,
        verilog_files: vec![],
        tcl_files: vec![],
        xci_files: vec![],
        scalar_register_map: HashMap::new(),
    })
}

/// Direction of a stream port, from its category.
fn stream_dir(port: &Port) -> StreamDir {
    if port.cat.is_output_stream() {
        StreamDir::Out
    } else {
        StreamDir::In
    }
}
