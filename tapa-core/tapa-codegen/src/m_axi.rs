//! M-AXI port generation and AXI crossbar.
//!
//! Implements: single-port M-AXI generation,
//! multi-port AXI crossbar with parameterized module emission.

use tapa_protocol::{
    PortDir, M_AXI_PORTS, M_AXI_PORT_WIDTHS, M_AXI_PREFIX, M_AXI_SUFFIXES_COMPACT,
};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{simple_port, wide_port, MutableModule};
use tapa_rtl::port::Direction;

use crate::error::CodegenError;
use crate::rtl_state::{routing_id_bits, MMapConnection};

const M_AXI_CHANNEL_ORDER: &[&str] = &["AR", "AW", "B", "R", "W"];

/// Add M-AXI ports for a single memory-mapped argument to a module.
///
/// Iterates all AXI channels (AR, AW, B, R, W) and their sub-ports,
/// adding properly-typed ports to the module.
pub fn add_m_axi_ports(module: &mut MutableModule, name: &str, data_width: u32, addr_width: u32) {
    add_m_axi_ports_with_id_width(module, name, data_width, addr_width, 1);
}

/// Add M-AXI ports with an explicit AXI ID width.
pub fn add_m_axi_ports_with_id_width(
    module: &mut MutableModule,
    name: &str,
    data_width: u32,
    addr_width: u32,
    id_width: u32,
) {
    let prefix = format!("{M_AXI_PREFIX}{}", sanitize_array_name(name));

    for &channel in M_AXI_CHANNEL_ORDER {
        let Some(&subports) = M_AXI_PORTS.get(channel) else {
            continue;
        };
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
                "ID" => id_width,
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

/// Internal wire prefix for a crossbar slave-side child connection.
pub fn crossbar_slave_prefix(arg_name: &str, slave_idx: usize) -> String {
    format!(
        "{M_AXI_PREFIX}{}_s{slave_idx}",
        sanitize_array_name(arg_name)
    )
}

/// Internal raw address signal emitted by a multi-channel crossbar.
pub fn crossbar_master_addr_raw(arg_name: &str, channel_idx: u32, suffix: &str) -> String {
    format!(
        "{M_AXI_PREFIX}{}_{channel_idx}{suffix}_raw",
        sanitize_array_name(arg_name)
    )
}

/// Build crossbar parameter arguments.
pub fn build_crossbar_params(conn: &MMapConnection) -> Vec<ParamArg> {
    let mut params = vec![
        ParamArg::new("DATA_WIDTH", Expr::int(u64::from(conn.data_width))),
        ParamArg::new("ADDR_WIDTH", Expr::int(64)),
        ParamArg::new(
            "S_ID_WIDTH",
            Expr::int(u64::from(crossbar_slave_id_width(conn))),
        ),
        ParamArg::new("M_ID_WIDTH", Expr::int(u64::from(conn.id_width))),
    ];

    for idx in 0..conn.chan_count {
        let addr_width = get_addr_width(conn.chan_size, conn.data_width);
        let base = if addr_width >= 64 {
            "64'd0".to_owned()
        } else {
            let base = u64::from(idx) << addr_width;
            format!("64'd{base}")
        };
        params.push(ParamArg::new(
            format!("M{idx:02}_BASE_ADDR"),
            Expr::lit(base),
        ));
        params.push(ParamArg::new(
            format!("M{idx:02}_ADDR_WIDTH"),
            Expr::int(u64::from(addr_width)),
        ));
        params.push(ParamArg::new(format!("M{idx:02}_ISSUE"), Expr::int(16)));
    }

    // Per-slave thread parameters — each slave gets at least 1 thread
    for idx in 0..conn.thread_count {
        // In the full implementation, this comes from per-child port metadata.
        // For now, use 1 thread per slave (the common case for simple designs).
        let threads = 1_u64;
        params.push(ParamArg::new(
            format!("S{idx:02}_THREADS"),
            Expr::int(threads),
        ));
    }

    params
}

pub fn crossbar_slave_id_width(conn: &MMapConnection) -> u32 {
    conn.id_width
        .saturating_sub(routing_id_bits(conn.thread_count))
        .max(1)
}

pub fn crossbar_slave_suffix_width(conn: &MMapConnection, suffix: &str) -> u32 {
    if matches!(suffix, "_ARID" | "_AWID" | "_BID" | "_RID") {
        crossbar_slave_id_width(conn)
    } else {
        resolve_suffix_width(suffix, conn.data_width)
    }
}

/// Build a crossbar module instance with port connections.
pub fn build_crossbar_instance(conn: &MMapConnection) -> ModuleInstance {
    let module_name = crossbar_module_name(conn);
    let arg_name = sanitize_array_name(&conn.arg_name);
    let instance_name = format!("axi_crossbar__{arg_name}");
    let params = build_crossbar_params(conn);

    let mut ports = vec![
        PortArg::new("clk", Expr::ident("ap_clk")),
        PortArg::new("rst", Expr::ident("ap_rst")),
    ];

    // Upstream master ports.
    for channel_idx in 0..conn.chan_count {
        let m_prefix = if conn.chan_count > 1 {
            format!("{M_AXI_PREFIX}{arg_name}_{channel_idx}")
        } else {
            format!("{M_AXI_PREFIX}{arg_name}")
        };
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let signal = if conn.chan_count > 1 && suffix.ends_with("ADDR") {
                crossbar_master_addr_raw(&arg_name, channel_idx, suffix)
            } else {
                format!("{m_prefix}{suffix}")
            };
            ports.push(PortArg::new(
                format!("m{channel_idx:02}{suffix}"),
                Expr::ident(signal),
            ));
        }
    }

    // Downstream slave ports — wire to internal per-child signals.
    for (slave_idx, (_task_name, _inst_idx, _child_port)) in conn.args.iter().enumerate() {
        let s_wire_prefix = crossbar_slave_prefix(&arg_name, slave_idx);
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
pub fn get_addr_width(chan_size: u32, data_width: u32) -> u32 {
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
    let suffix = suffix.trim_start_matches('_');
    let subport = suffix
        .strip_prefix("AR")
        .or_else(|| suffix.strip_prefix("AW"))
        .or_else(|| suffix.strip_prefix('R'))
        .or_else(|| suffix.strip_prefix('W'))
        .or_else(|| suffix.strip_prefix('B'))
        .unwrap_or(suffix);

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

    let _ = writeln!(
        rtl,
        "// Auto-generated AXI crossbar: {slaves} slaves x {channels} channels"
    );
    let _ = writeln!(rtl, "module {module_name} #(");
    let _ = writeln!(rtl, "  parameter DATA_WIDTH = 32,");
    let _ = writeln!(rtl, "  parameter ADDR_WIDTH = 64,");
    let _ = writeln!(rtl, "  parameter S_ID_WIDTH = 1,");
    let _ = writeln!(rtl, "  parameter M_ID_WIDTH = S_ID_WIDTH+$clog2({slaves}),");
    for idx in 0..channels {
        let comma = if idx + 1 == channels && slaves == 0 {
            ""
        } else {
            ","
        };
        let _ = writeln!(rtl, "  parameter M{idx:02}_BASE_ADDR = 0,");
        let _ = writeln!(rtl, "  parameter M{idx:02}_ADDR_WIDTH = ADDR_WIDTH,");
        let _ = writeln!(rtl, "  parameter M{idx:02}_ISSUE = 16{comma}");
    }
    for idx in 0..slaves {
        let comma = if idx + 1 < slaves { "," } else { "" };
        let _ = writeln!(rtl, "  parameter S{idx:02}_THREADS = 1{comma}");
    }
    let _ = writeln!(rtl, ") (");
    let _ = writeln!(rtl, "  input wire clk,");
    let _ = writeln!(rtl, "  input wire rst,");

    // Master-facing ports to the external memory channels.
    for ch_idx in 0..channels {
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let direction = if is_master_output_suffix(suffix) {
                "output"
            } else {
                "input"
            };
            let width = crossbar_port_width(suffix, true);
            let _ = writeln!(
                rtl,
                "  {direction} wire{} m{ch_idx:02}{suffix},",
                width_decl(&width)
            );
        }
    }

    // Slave-facing ports from child AXI masters.
    for s_idx in 0..slaves {
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let direction = if is_master_output_suffix(suffix) {
                "input"
            } else {
                "output"
            };
            let width = crossbar_port_width(suffix, false);
            let _ = writeln!(
                rtl,
                "  {direction} wire{} s{s_idx:02}{suffix},",
                width_decl(&width)
            );
        }
    }

    // Remove trailing comma from last port
    if rtl.ends_with(",\n") {
        rtl.truncate(rtl.len() - 2);
        rtl.push('\n');
    }

    let _ = writeln!(rtl, ");");
    let _ = writeln!(rtl);
    let m_base_addr = (0..channels)
        .rev()
        .map(|idx| format!("M{idx:02}_BASE_ADDR"))
        .collect::<Vec<_>>()
        .join(", ");
    let m_addr_width = (0..channels)
        .rev()
        .map(|idx| format!("M{idx:02}_ADDR_WIDTH"))
        .collect::<Vec<_>>()
        .join(", ");
    let m_issue = (0..channels)
        .rev()
        .map(|idx| format!("M{idx:02}_ISSUE"))
        .collect::<Vec<_>>()
        .join(", ");
    let s_threads = (0..slaves)
        .rev()
        .map(|idx| format!("S{idx:02}_THREADS"))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(rtl, "axi_crossbar #(");
    let _ = writeln!(rtl, "  .S_COUNT({slaves}),");
    let _ = writeln!(rtl, "  .M_COUNT({channels}),");
    let _ = writeln!(rtl, "  .DATA_WIDTH(DATA_WIDTH),");
    let _ = writeln!(rtl, "  .ADDR_WIDTH(ADDR_WIDTH),");
    let _ = writeln!(rtl, "  .S_ID_WIDTH(S_ID_WIDTH),");
    let _ = writeln!(rtl, "  .M_ID_WIDTH(M_ID_WIDTH),");
    let _ = writeln!(rtl, "  .S_THREADS({{{s_threads}}}),");
    let _ = writeln!(rtl, "  .S_ACCEPT({{{slaves}{{32'd16}}}}),");
    let _ = writeln!(rtl, "  .M_REGIONS(1),");
    let _ = writeln!(rtl, "  .M_BASE_ADDR({{{m_base_addr}}}),");
    let _ = writeln!(rtl, "  .M_ADDR_WIDTH({{{m_addr_width}}}),");
    let connect_bits = channels * slaves;
    let _ = writeln!(rtl, "  .M_CONNECT_READ({{{connect_bits}{{1'b1}}}}),");
    let _ = writeln!(rtl, "  .M_CONNECT_WRITE({{{connect_bits}{{1'b1}}}}),");
    let _ = writeln!(rtl, "  .M_ISSUE({{{m_issue}}})");
    let _ = writeln!(rtl, ") xbar (");
    let _ = writeln!(rtl, "  .clk(clk),");
    let _ = writeln!(rtl, "  .rst(rst),");
    write_axi_crossbar_port_connections(&mut rtl, "s_axi", "s", slaves, true);
    write_axi_crossbar_port_connections(&mut rtl, "m_axi", "m", channels, false);
    let _ = writeln!(rtl, "  .s_axi_awuser({slaves}'b0),");
    let _ = writeln!(rtl, "  .s_axi_wuser({slaves}'b0),");
    let _ = writeln!(rtl, "  .s_axi_aruser({slaves}'b0),");
    let _ = writeln!(rtl, "  .m_axi_awregion(),");
    let _ = writeln!(rtl, "  .m_axi_awuser(),");
    let _ = writeln!(rtl, "  .m_axi_arregion(),");
    let _ = writeln!(rtl, "  .m_axi_aruser(),");
    let _ = writeln!(rtl, "  .s_axi_buser(),");
    let _ = writeln!(rtl, "  .s_axi_ruser(),");
    let _ = writeln!(rtl, "  .m_axi_buser({channels}'b0),");
    let _ = writeln!(rtl, "  .m_axi_ruser({channels}'b0)");
    let _ = writeln!(rtl, ");");
    let _ = writeln!(rtl);
    let _ = writeln!(rtl, "endmodule //{module_name}");

    rtl
}

fn is_master_output_suffix(suffix: &str) -> bool {
    let channel_sub = suffix.trim_start_matches('_');
    if channel_sub.starts_with("AW")
        || channel_sub.starts_with("AR")
        || channel_sub.starts_with('W')
    {
        !channel_sub.ends_with("READY")
    } else {
        channel_sub.ends_with("READY")
    }
}

fn crossbar_port_width(suffix: &str, master_side: bool) -> String {
    let subport = suffix
        .trim_start_matches('_')
        .strip_prefix("AR")
        .or_else(|| suffix.trim_start_matches('_').strip_prefix("AW"))
        .or_else(|| suffix.trim_start_matches('_').strip_prefix('R'))
        .or_else(|| suffix.trim_start_matches('_').strip_prefix('W'))
        .or_else(|| suffix.trim_start_matches('_').strip_prefix('B'))
        .unwrap_or_else(|| suffix.trim_start_matches('_'));

    if subport == "ID" {
        if master_side {
            "M_ID_WIDTH".to_owned()
        } else {
            "S_ID_WIDTH".to_owned()
        }
    } else if subport == "STRB" {
        "DATA_WIDTH/8".to_owned()
    } else {
        match resolve_suffix_width(suffix, 0) {
            0 => "DATA_WIDTH".to_owned(),
            64 if suffix.ends_with("ADDR") => "ADDR_WIDTH".to_owned(),
            n => n.to_string(),
        }
    }
}

fn width_decl(width: &str) -> String {
    if width == "1" {
        String::new()
    } else {
        format!(" [{width}-1:0]")
    }
}

fn concat_ports(prefix: &str, count: u32, suffix: &str) -> String {
    let port_name = |idx| format!("{prefix}{idx:02}{suffix}");
    if count == 1 {
        port_name(0)
    } else {
        format!(
            "{{{}}}",
            (0..count)
                .rev()
                .map(port_name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn write_axi_crossbar_port_connections(
    rtl: &mut String,
    axi_prefix: &str,
    port_prefix: &str,
    count: u32,
    slave_side: bool,
) {
    use std::fmt::Write;

    let optional_addr_ports = [
        ("awlock", "1'b0"),
        ("awcache", "4'b0011"),
        ("awprot", "3'b000"),
        ("awqos", "4'b0000"),
        ("arlock", "1'b0"),
        ("arcache", "4'b0011"),
        ("arprot", "3'b000"),
        ("arqos", "4'b0000"),
    ];
    for (name, value) in optional_addr_ports {
        if slave_side {
            let _ = writeln!(rtl, "  .{axi_prefix}_{name}({{{count}{{{value}}}}}),");
        } else {
            let _ = writeln!(rtl, "  .{axi_prefix}_{name}(),");
        }
    }

    for suffix in M_AXI_SUFFIXES_COMPACT {
        let signal = concat_ports(port_prefix, count, suffix);
        let axi_suffix = suffix.trim_start_matches('_').to_ascii_lowercase();
        let _ = writeln!(rtl, "  .{axi_prefix}_{axi_suffix}({signal}),");
    }
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
    fn crossbar_needed_for_hmap_channels() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_a".into(), 1, "data".into()),
            ],
            chan_count: 2,
            chan_size: 1024,
            data_width: 32,
        };
        assert!(needs_crossbar(&conn));
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
        assert!(
            params.len() >= 4,
            "should have at least DATA/ADDR/S_ID/M_ID"
        );
        assert_eq!(params[0].param_name, "DATA_WIDTH");
    }

    #[test]
    fn crossbar_params_expand_master_id_width_for_slave_routing() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 3,
            thread_count: 3,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_b".into(), 0, "data".into()),
                ("task_c".into(), 0, "data".into()),
            ],
            chan_count: 1,
            chan_size: 0,
            data_width: 64,
        };
        let text = build_crossbar_instance(&conn).to_string();
        assert!(text.contains(".S_ID_WIDTH(1)"), "got:\n{text}");
        assert!(text.contains(".M_ID_WIDTH(3)"), "got:\n{text}");
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
    fn resolve_suffix_width_extracts_axi_subport_names() {
        assert_eq!(resolve_suffix_width("_ARADDR", 512), 64);
        assert_eq!(resolve_suffix_width("_AWADDR", 512), 64);
        assert_eq!(resolve_suffix_width("_WDATA", 512), 512);
        assert_eq!(resolve_suffix_width("_WSTRB", 512), 64);
        assert_eq!(resolve_suffix_width("_RID", 512), 1);
    }

    #[test]
    fn crossbar_slave_suffix_width_only_widens_id_ports() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 3,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_b".into(), 0, "data".into()),
            ],
            chan_count: 1,
            chan_size: 0,
            data_width: 32,
        };

        assert_eq!(crossbar_slave_suffix_width(&conn, "_ARID"), 2);
        assert_eq!(crossbar_slave_suffix_width(&conn, "_ARVALID"), 1);
        assert_eq!(crossbar_slave_suffix_width(&conn, "_AWVALID"), 1);
        assert_eq!(crossbar_slave_suffix_width(&conn, "_WDATA"), 32);
    }

    #[test]
    fn add_m_axi_ports_sanitizes_indexed_names() {
        let module = tapa_rtl::VerilogModule::parse("module top(); endmodule").unwrap();
        let mut module = MutableModule::from_parsed(module);
        add_m_axi_ports(&mut module, "chan[0]", 512, 64);
        let text = module.emit();
        assert!(text.contains("m_axi_chan_0_ARADDR"), "got:\n{text}");
        assert!(!text.contains("m_axi_chan[0]"), "got:\n{text}");
    }

    #[test]
    fn add_m_axi_ports_uses_explicit_id_width() {
        let module = tapa_rtl::VerilogModule::parse("module top(); endmodule").unwrap();
        let mut module = MutableModule::from_parsed(module);
        add_m_axi_ports_with_id_width(&mut module, "mem", 512, 64, 2);
        let text = module.emit();
        assert!(text.contains("m_axi_mem_ARID"), "got:\n{text}");
        assert!(text.contains("[1:0] m_axi_mem_ARID"), "got:\n{text}");
    }

    #[test]
    fn add_m_axi_ports_uses_stable_channel_order() {
        let module = tapa_rtl::VerilogModule::parse("module top(); endmodule").unwrap();
        let mut module = MutableModule::from_parsed(module);
        add_m_axi_ports(&mut module, "mem", 32, 64);
        let text = module.emit();

        let ar = text.find("m_axi_mem_ARADDR").unwrap();
        let aw = text.find("m_axi_mem_AWADDR").unwrap();
        let b = text.find("m_axi_mem_BID").unwrap();
        let r = text.find("m_axi_mem_RDATA").unwrap();
        let w = text.find("m_axi_mem_WDATA").unwrap();
        assert!(
            ar < aw && aw < b && b < r && r < w,
            "M-AXI ports must be emitted deterministically by channel:\n{text}"
        );
    }

    #[test]
    fn crossbar_instance_sanitizes_indexed_names() {
        let conn = MMapConnection {
            arg_name: "chan[0]".into(),
            id_width: 1,
            thread_count: 1,
            args: vec![("task_a".into(), 0, "mem".into())],
            chan_count: 1,
            chan_size: 0,
            data_width: 32,
        };
        let text = build_crossbar_instance(&conn).to_string();
        assert!(text.contains("axi_crossbar__chan_0"), "got:\n{text}");
        assert!(text.contains("m_axi_chan_0_ARADDR"), "got:\n{text}");
        assert!(!text.contains("chan[0]"), "got:\n{text}");
    }

    #[test]
    fn crossbar_instance_connects_multiple_parent_channels() {
        let conn = MMapConnection {
            arg_name: "mat_a".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "mem".into()),
                ("task_a".into(), 1, "mem".into()),
            ],
            chan_count: 2,
            chan_size: 1024,
            data_width: 512,
        };
        let text = build_crossbar_instance(&conn).to_string();
        assert!(
            text.contains(".m00_ARADDR(m_axi_mat_a_0_ARADDR_raw)"),
            "got:\n{text}"
        );
        assert!(
            text.contains(".m01_ARADDR(m_axi_mat_a_1_ARADDR_raw)"),
            "got:\n{text}"
        );
        assert!(text.contains(".M01_BASE_ADDR(64'd65536)"), "got:\n{text}");
        assert!(
            text.contains(".s00_ARADDR(m_axi_mat_a_s0_ARADDR)"),
            "got:\n{text}"
        );
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
        assert!(rtl.contains("axi_crossbar #("), "got:\n{rtl}");
        assert!(rtl.contains("parameter M00_ADDR_WIDTH"), "got:\n{rtl}");
        assert!(
            rtl.contains(".s_axi_araddr({s01_ARADDR, s00_ARADDR})"),
            "got:\n{rtl}"
        );
        assert!(rtl.contains(".m_axi_araddr(m00_ARADDR)"), "got:\n{rtl}");
        assert!(rtl.contains("endmodule"), "got:\n{rtl}");
    }

    #[test]
    fn crossbar_rtl_exposes_multi_channel_base_addresses() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            id_width: 2,
            thread_count: 2,
            args: vec![
                ("task_a".into(), 0, "data".into()),
                ("task_b".into(), 0, "data".into()),
            ],
            chan_count: 2,
            chan_size: 1024,
            data_width: 32,
        };
        let rtl = generate_crossbar_rtl(&conn);
        assert!(rtl.contains("parameter M01_BASE_ADDR = 0,"), "got:\n{rtl}");
        assert!(
            rtl.contains(".M_BASE_ADDR({M01_BASE_ADDR, M00_BASE_ADDR})"),
            "got:\n{rtl}"
        );
        assert!(
            rtl.contains(".m_axi_araddr({m01_ARADDR, m00_ARADDR})"),
            "got:\n{rtl}"
        );
    }
}
