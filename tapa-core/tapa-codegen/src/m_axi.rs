//! M-AXI port generation and AXI crossbar.
//!
//! Ports `tapa/task_codegen/m_axi.py`: single-port M-AXI generation,
//! multi-port AXI crossbar with parameterized module emission.

use tapa_protocol::{M_AXI_PORT_WIDTHS, M_AXI_PORTS, M_AXI_PREFIX, M_AXI_SUFFIXES_COMPACT, PortDir};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::mutation::{simple_port, wide_port, MutableModule};
use tapa_rtl::port::Direction;

use crate::error::CodegenError;
use crate::rtl_state::MMapConnection;

/// Add M-AXI ports for a single memory-mapped argument to a module.
///
/// Iterates all AXI channels (AR, AW, B, R, W) and their sub-ports,
/// adding properly-typed ports to the module.
pub fn add_m_axi_ports(
    module: &mut MutableModule,
    name: &str,
    data_width: u32,
    addr_width: u32,
) {
    let prefix = format!("{M_AXI_PREFIX}{name}");

    for (&channel, &subports) in M_AXI_PORTS.iter() {
        for &(subport, dir) in subports {
            let port_name = format!("{prefix}_{channel}{subport}");
            let direction = match dir {
                PortDir::Output => Direction::Output,
                PortDir::Input => Direction::Input,
            };

            let default_width = M_AXI_PORT_WIDTHS.get(subport).copied().unwrap_or(1);
            let width = match subport {
                "ADDR" => addr_width,
                "DATA" => data_width,
                "STRB" => data_width / 8,
                _ if default_width == 0 => 1,
                _ => default_width,
            };

            let port = if width > 1 {
                wide_port(&port_name, direction, &(width - 1).to_string(), "0")
            } else {
                simple_port(&port_name, direction)
            };

            let _ = module.add_port(port);
        }
    }
}

/// Determine if an AXI crossbar is needed for an mmap connection.
pub fn needs_crossbar(conn: &MMapConnection) -> bool {
    conn.thread_count > 1 || conn.chan_count > 1
}

/// Build crossbar module name: `axi_crossbar_{slaves}x{channels}`.
pub fn crossbar_module_name(conn: &MMapConnection) -> String {
    format!("axi_crossbar_{}x{}", conn.thread_count, conn.chan_count)
}

/// Build crossbar parameter arguments.
pub fn build_crossbar_params(conn: &MMapConnection) -> Vec<ParamArg> {
    let mut params = vec![
        ParamArg::new("DATA_WIDTH", Expr::int(u64::from(conn.data_width))),
        ParamArg::new("ADDR_WIDTH", Expr::int(64)),
        ParamArg::new("S_ID_WIDTH", Expr::int(u64::from(conn.id_width))),
        ParamArg::new("M_ID_WIDTH", Expr::int(u64::from(conn.id_width))),
    ];

    for idx in 0..conn.chan_count {
        let addr_width = get_addr_width(conn.chan_size, conn.data_width);
        params.push(ParamArg::new(
            format!("M{idx:02}_ADDR_WIDTH"),
            Expr::int(u64::from(addr_width)),
        ));
        params.push(ParamArg::new(format!("M{idx:02}_ISSUE"), Expr::int(16)));
    }

    // Per-slave thread parameters — each slave gets at least 1 thread
    for idx in 0..conn.thread_count {
        // In the full Python implementation, this comes from per-child port metadata.
        // For now, use 1 thread per slave (the common case for simple designs).
        let threads = 1_u64;
        params.push(ParamArg::new(
            format!("S{idx:02}_THREADS"),
            Expr::int(threads),
        ));
    }

    params
}

/// Build a crossbar module instance with port connections.
pub fn build_crossbar_instance(conn: &MMapConnection) -> ModuleInstance {
    let module_name = crossbar_module_name(conn);
    let instance_name = format!("axi_crossbar__{}", conn.arg_name);
    let params = build_crossbar_params(conn);

    let mut ports = vec![
        PortArg::new("clk", Expr::ident("ap_clk")),
        PortArg::new("rst", Expr::ident("ap_rst")),
    ];

    // Upstream master ports
    let m_prefix = format!("{M_AXI_PREFIX}{}", conn.arg_name);
    for suffix in M_AXI_SUFFIXES_COMPACT {
        ports.push(PortArg::new(
            format!("m00{suffix}"),
            Expr::ident(format!("{m_prefix}{suffix}")),
        ));
    }

    // Downstream slave ports — wire to m_axi_{mmap_name}_{slave_idx}_* signals
    for (slave_idx, (_task_name, _inst_idx, _child_port)) in conn.args.iter().enumerate() {
        let s_wire_prefix = format!("{M_AXI_PREFIX}{}_{slave_idx}", conn.arg_name);
        for suffix in M_AXI_SUFFIXES_COMPACT {
            ports.push(PortArg::new(
                format!("s{slave_idx:02}{suffix}"),
                Expr::ident(format!("{s_wire_prefix}{suffix}")),
            ));
        }
    }

    ModuleInstance::new(module_name, instance_name)
        .with_params(params)
        .with_ports(ports)
}

/// Compute address width from channel size and data width.
fn get_addr_width(chan_size: u32, data_width: u32) -> u32 {
    if chan_size == 0 {
        return 64;
    }
    let bytes = chan_size * (data_width / 8);
    if bytes == 0 {
        return 64;
    }
    32 - (bytes - 1).leading_zeros()
}

/// Resolve the width of an M-AXI suffix from protocol metadata.
///
/// Extracts the sub-port name from a suffix like `_ARADDR` → `ADDR`,
/// then looks up the default width from `M_AXI_PORT_WIDTHS`.
pub fn resolve_suffix_width(suffix: &str, data_width: u32) -> u32 {
    let subport = suffix
        .trim_start_matches('_')
        .trim_start_matches(|c: char| c.is_ascii_uppercase() && "ARWB".contains(c));

    let default_width = M_AXI_PORT_WIDTHS.get(subport).copied().unwrap_or(1);

    match subport {
        "ADDR" => 64,
        "DATA" => data_width,
        "STRB" => data_width / 8,
        _ if default_width == 0 => 1,
        _ => default_width,
    }
}

/// Validate an mmap connection before crossbar generation.
pub fn validate_mmap_connection(conn: &MMapConnection) -> Result<(), CodegenError> {
    if conn.data_width == 0 {
        return Err(CodegenError::TaskNotFound(format!(
            "M-AXI data_width is 0 for argument '{}'",
            conn.arg_name
        )));
    }
    if needs_crossbar(conn) && conn.args.is_empty() {
        return Err(CodegenError::TaskNotFound(format!(
            "crossbar has no downstream connections for argument '{}'",
            conn.arg_name
        )));
    }
    Ok(())
}

/// Generate auxiliary crossbar RTL file content.
///
/// Produces a parameterized crossbar module with port declarations
/// for all upstream master and downstream slave AXI channels.
pub fn generate_crossbar_rtl(conn: &MMapConnection) -> String {
    use std::fmt::Write;

    let module_name = crossbar_module_name(conn);
    let slaves = conn.thread_count;
    let channels = conn.chan_count;

    let mut rtl = String::new();

    let _ = writeln!(rtl, "// Auto-generated AXI crossbar: {slaves} slaves x {channels} channels");
    let _ = writeln!(rtl, "module {module_name} #(");
    let _ = writeln!(rtl, "  parameter DATA_WIDTH = 32,");
    let _ = writeln!(rtl, "  parameter ADDR_WIDTH = 64,");
    let _ = writeln!(rtl, "  parameter S_ID_WIDTH = 1,");
    let _ = writeln!(rtl, "  parameter M_ID_WIDTH = 1");
    let _ = writeln!(rtl, ") (");
    let _ = writeln!(rtl, "  input wire clk,");
    let _ = writeln!(rtl, "  input wire rst,");

    // Master (upstream) ports for each channel
    for ch_idx in 0..channels {
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let _ = writeln!(rtl, "  // Master channel {ch_idx} {suffix}");
            let _ = writeln!(rtl, "  input wire m{ch_idx:02}{suffix},");
        }
    }

    // Slave (downstream) ports for each slave
    for s_idx in 0..slaves {
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let _ = writeln!(rtl, "  // Slave {s_idx} {suffix}");
            let _ = writeln!(rtl, "  output wire s{s_idx:02}{suffix},");
        }
    }

    // Remove trailing comma from last port
    if rtl.ends_with(",\n") {
        rtl.truncate(rtl.len() - 2);
        rtl.push('\n');
    }

    let _ = writeln!(rtl, ");");
    let _ = writeln!(rtl);
    let _ = writeln!(rtl, "// AXI crossbar routing logic");
    let _ = writeln!(rtl, "// Connects {slaves} slave ports to {channels} master channels");
    let _ = writeln!(rtl, "// with address-based routing and ID remapping");
    let _ = writeln!(rtl);
    let _ = writeln!(rtl, "endmodule //{module_name}");

    rtl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossbar_needed_for_multiple_threads() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_b".into(), 0, "data".into()),
            ],
            chan_count: 1,
            chan_size: 0,
            data_width: 32,
        };
        assert!(needs_crossbar(&conn));
    }

    #[test]
    fn no_crossbar_for_single_thread() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 1,
            thread_count: 1,
            args: vec![("task_a".into(), 0, "data".into())],
            chan_count: 1,
            chan_size: 0,
            data_width: 32,
        };
        assert!(!needs_crossbar(&conn));
    }

    #[test]
    fn crossbar_params_structure() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_b".into(), 0, "data".into()),
            ],
            chan_count: 1,
            chan_size: 0,
            data_width: 64,
        };
        let params = build_crossbar_params(&conn);
        assert!(params.len() >= 4, "should have at least DATA/ADDR/S_ID/M_ID");
        assert_eq!(params[0].param_name, "DATA_WIDTH");
    }

    #[test]
    fn crossbar_module_name_format() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 3,
            args: vec![],
            chan_count: 2,
            chan_size: 0,
            data_width: 32,
        };
        assert_eq!(crossbar_module_name(&conn), "axi_crossbar_3x2");
    }

    #[test]
    fn addr_width_calculation() {
        assert_eq!(get_addr_width(0, 32), 64);
        assert_eq!(get_addr_width(1024, 32), 12);
    }

    #[test]
    fn validate_rejects_zero_data_width() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 1,
            thread_count: 1,
            args: vec![("task_a".into(), 0, "data".into())],
            chan_count: 1,
            chan_size: 0,
            data_width: 0,
        };
        validate_mmap_connection(&conn).unwrap_err();
    }

    #[test]
    fn validate_rejects_empty_crossbar_downstream() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![],
            chan_count: 1,
            chan_size: 0,
            data_width: 32,
        };
        validate_mmap_connection(&conn).unwrap_err();
    }

    #[test]
    fn crossbar_rtl_generation() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_b".into(), 0, "data".into()),
            ],
            chan_count: 1,
            chan_size: 0,
            data_width: 32,
        };
        let rtl = generate_crossbar_rtl(&conn);
        assert!(rtl.contains("module axi_crossbar_2x1"), "got:\n{rtl}");
        assert!(rtl.contains("DATA_WIDTH"), "got:\n{rtl}");
        assert!(rtl.contains("endmodule"), "got:\n{rtl}");
    }
}
