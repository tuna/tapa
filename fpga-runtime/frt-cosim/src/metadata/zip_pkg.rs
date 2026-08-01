use super::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir, StreamProtocol};
use crate::error::{CosimError, Result};
use std::collections::HashMap;
use tapa_ir::port::{sanitize_array_name, ArgCategory, Port};
use tapa_ir::TaskGraph;

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
        // The frontend spells an array interface's channels `name[i]`, but
        // every RTL identifier collapses that to `name_i` -- and these names
        // are what the testbench binds ports, buffers and offset registers
        // by, so they have to be the RTL spelling, not the schema spelling.
        let name = sanitize_array_name(&port.name);
        let name = name.as_str();
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
                // Legacy-archive compatibility: `tapa pack` stamps
                // `mmap_addr_width` today, so `None` means an archive
                // written before the field existed. Those were always
                // simulated with a 64-bit address (the removed
                // `MMAP_ADDR_WIDTH` fallback), and unwrapping to it keeps
                // their argument shape bit for bit.
                let kind = ArgKind::Mmap {
                    data_width: width,
                    addr_width: port.mmap_addr_width.unwrap_or(64),
                };
                // `chan_count` is what makes an mmap port an `hmap`: the
                // frontend fills it in for `hmap` and nothing else, so a plain
                // mmap always leaves it unset (a `Some(1)` hmap is still an
                // hmap). An `hmap<T, N, S>` is one host buffer that the host
                // splits into N kernel m_axi arguments named `{name}_{i}` --
                // the same fan-out `tapa pack` projects into `kernel.xml` and
                // `tapa-codegen` wires to the AXI crossbar. Binding one
                // argument here would bind the wrong ports *and* shift every
                // later argument's id.
                if let Some(hmap_chans) = port.chan_count {
                    if hmap_chans == 0 {
                        return Err(CosimError::Metadata(format!(
                            "hmap channel count is 0 for argument '{name}'"
                        )));
                    }
                    for i in 0..hmap_chans {
                        args.push(ArgSpec {
                            name: format!("{name}_{i}"),
                            id: next_id,
                            kind: kind.clone(),
                        });
                        next_id += 1;
                    }
                } else {
                    args.push(ArgSpec {
                        name: name.to_owned(),
                        id: next_id,
                        kind,
                    });
                    next_id += 1;
                }
            }
            ArgCategory::Istream | ArgCategory::Ostream => {
                // Same legacy-archive story as the mmap address width
                // above: `None` marks a pre-field archive, and 16 is the
                // depth those have always been simulated with.
                args.push(ArgSpec {
                    name: format!("{name}_s"),
                    id: next_id,
                    kind: ArgKind::Stream {
                        width,
                        depth: port.stream_depth.unwrap_or(16),
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
                            depth: port.stream_depth.unwrap_or(16),
                            dir: stream_dir(port),
                            protocol: StreamProtocol::ApFifo,
                        },
                    });
                    next_id += 1;
                }
            }
            // Read-only / write-only mmaps have never been wired up here.
            // This arm is the backstop of the pack-time frontier in
            // `tapa-core/tapa-cli/src/steps/pack/cosim_compat.rs`: fresh
            // archives are rejected there, before they ship, so the error
            // below fires only for pre-frontier or hand-built archives.
            // Keep the cosim-consumable category set in sync with that
            // file — a two-workspace seam the `frt-cbindgen` drift guard
            // does not cover (it guards generated headers). Rejecting
            // keeps that an explicit, loud limitation rather than silently
            // binding them as plain mmaps.
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
