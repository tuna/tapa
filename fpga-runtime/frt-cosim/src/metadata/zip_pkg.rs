use super::{ArgKind, ArgSpec, KernelSpec, Mode, StreamDir, StreamProtocol};

use crate::error::{CosimError, Result};
use std::collections::HashMap;
use tapa_ir::port::{sanitize_array_name, ArgCategory, Port};
use tapa_ir::{Target, TaskGraph};

/// Project the packed task graph into the flat kernel argument list.
///
/// The top task's `ports` expand into one or more kernel arguments each,
/// numbered in declaration order — that numbering is the kernel ABI the
/// simulator binds against, so the order here is load-bearing.
pub fn spec_from_task_graph(graph: &TaskGraph) -> Result<KernelSpec> {
    let top_task = graph.tasks.get(&graph.top).ok_or_else(|| {
        CosimError::Metadata(format!(
            "top task `{}` is missing from the task graph",
            graph.top
        ))
    })?;

    let mut args = Vec::new();
    for port in &top_task.ports {
        let width = port.width;
        let kind = match port.cat {
            ArgCategory::Scalar => ArgKind::Scalar { width },
            ArgCategory::Mmap | ArgCategory::AsyncMmap => {
                if port.chan_count == Some(0) {
                    return Err(CosimError::Metadata(format!(
                        "hmap channel count is 0 for argument '{}'",
                        sanitize_array_name(&port.name)
                    )));
                }
                ArgKind::Mmap {
                    data_width: width,
                    addr_width: required_cosim_metadata(
                        port.mmap_addr_width,
                        port,
                        "mmap_addr_width",
                    )?,
                }
            }
            ArgCategory::Istream
            | ArgCategory::Ostream
            | ArgCategory::Istreams
            | ArgCategory::Ostreams => ArgKind::Stream {
                width,
                depth: required_cosim_metadata(port.stream_depth, port, "stream_depth")?,
                dir: stream_dir(port),
                protocol: StreamProtocol::ApFifo,
            },
            // A frame port never reaches a kernel boundary (graph
            // validation); this backstops hand-built archives.
            ArgCategory::Iframes | ArgCategory::Oframes => {
                return Err(CosimError::Metadata(format!(
                    "unsupported port category '{}'",
                    port.cat.as_str()
                )));
            }
        };
        for name in port.kernel_arg_names(Target::XilinxHls) {
            args.push(ArgSpec {
                name,
                id: u32::try_from(args.len()).expect("kernel argument count fits u32"),
                kind: kind.clone(),
            });
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
        inlined: Vec::new(),
        memories: Vec::new(),
        rings: Vec::new(),
    })
}

fn required_cosim_metadata<T>(value: Option<T>, port: &Port, field: &str) -> Result<T> {
    value.ok_or_else(|| {
        CosimError::Metadata(format!(
            "port '{}' has no {field}; archive predates cosim port metadata; re-run `tapa pack`",
            sanitize_array_name(&port.name)
        ))
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
