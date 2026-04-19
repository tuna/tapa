//! FIFO instantiation and connection.
//!
//! Ports `tapa/task_codegen/fifos.py` and `tapa/program_codegen/fifos.py`.

use tapa_protocol::{
    FIFO_READ_PORTS, FIFO_WRITE_PORTS, ISTREAM_SUFFIXES, OSTREAM_SUFFIXES, STREAM_PORT_DIRECTION,
};
use tapa_rtl::builder::{ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;

/// Build a FIFO module instance with WIDTH and DEPTH parameters.
///
/// The FIFO module has internal port names like `if_dout`, `if_read`, etc.
/// The connection side uses stream suffix naming: `{name}_dout`, `{name}_read`.
/// This matches how children and `connect_fifos` declare wires.
pub fn build_fifo_instance(name: &str, rst: Expr, width: Expr, depth: u32) -> ModuleInstance {
    let name = sanitize_array_name(name);
    let addr_width = if depth <= 1 {
        1
    } else {
        u64::from(depth - 1).ilog2() + 1
    };
    ModuleInstance::new("fifo", format!("{name}_fifo"))
        .with_params(vec![
            ParamArg::new("DATA_WIDTH", width),
            ParamArg::new("ADDR_WIDTH", Expr::int(u64::from(addr_width))),
            ParamArg::new("DEPTH", Expr::int(u64::from(depth))),
        ])
        .with_ports({
            let mut ports = vec![
                PortArg::new("clk", Expr::ident("ap_clk")),
                PortArg::new("reset", rst),
            ];
            // FIFO_READ_PORTS are if_* names; strip "if" prefix for wire names
            // if_dout -> {name}_dout, if_empty_n -> {name}_empty_n, etc.
            for port_name in FIFO_READ_PORTS {
                let wire_suffix = port_name.strip_prefix("if").unwrap_or(port_name);
                let expr = if *port_name == "if_read_ce" {
                    Expr::lit("1'b1")
                } else {
                    Expr::ident(format!("{name}{wire_suffix}"))
                };
                ports.push(PortArg::new(*port_name, expr));
            }
            for port_name in FIFO_WRITE_PORTS {
                let wire_suffix = port_name.strip_prefix("if").unwrap_or(port_name);
                let expr = if *port_name == "if_write_ce" {
                    Expr::lit("1'b1")
                } else {
                    Expr::ident(format!("{name}{wire_suffix}"))
                };
                ports.push(PortArg::new(*port_name, expr));
            }
            ports
        })
}

/// Generate wire assignments for an external FIFO passthrough.
///
/// For an external FIFO (no depth), creates assigns connecting the
/// FIFO's internal signal names (`fifo_name + suffix`) to the parent
/// module's port names (`fifo_name + suffix`). The internal and external
/// names are the same — the parent module already has these as ports.
///
/// Directionality is respected:
/// - Input suffixes (`_dout`, `_empty_n`, `_full_n`): these are driven by
///   the external side (the parent port drives the internal wire)
/// - Output suffixes (`_read`, `_din`, `_write`): these are driven by
///   the internal side (the child instance drives the port)
///
/// For external FIFOs, no assigns are needed when names match —
/// the child instance portargs connect directly to the parent ports.
/// This function returns assigns only when the FIFO internal name
/// differs from the parent port name (e.g., renamed FIFOs).
pub fn build_external_fifo_assigns(
    internal_name: &str,
    external_name: &str,
    is_consumed: bool,
) -> Vec<ContinuousAssign> {
    let internal_name = sanitize_array_name(internal_name);
    let external_name = sanitize_array_name(external_name);
    if internal_name == external_name {
        return Vec::new(); // Names match, no assigns needed
    }

    let suffixes: &[&str] = if is_consumed {
        ISTREAM_SUFFIXES
    } else {
        OSTREAM_SUFFIXES
    };

    suffixes
        .iter()
        .map(|suffix| {
            let is_input_dir = STREAM_PORT_DIRECTION
                .get(suffix)
                .is_some_and(|&d| d == "input");

            if is_input_dir {
                ContinuousAssign::new(
                    Expr::ident(format!("{internal_name}{suffix}")),
                    Expr::ident(format!("{external_name}{suffix}")),
                )
            } else {
                ContinuousAssign::new(
                    Expr::ident(format!("{external_name}{suffix}")),
                    Expr::ident(format!("{internal_name}{suffix}")),
                )
            }
        })
        .collect()
}

/// Build an AXIS-to-stream or stream-to-AXIS adapter instance.
///
/// `is_input`: true for `axis_to_stream_adapter`, false for `stream_to_axis_adapter`.
pub fn build_axis_adapter(fifo_name: &str, data_width: u32, is_input: bool) -> ModuleInstance {
    let fifo_name = sanitize_array_name(fifo_name);
    let module_name = if is_input {
        "axis_to_stream_adapter"
    } else {
        "stream_to_axis_adapter"
    };
    let instance_name = format!("tapa_axis_{fifo_name}");

    let mut ports = vec![
        PortArg::new("clk", Expr::ident("ap_clk")),
        PortArg::new("reset", Expr::ident("ap_rst")),
    ];

    if is_input {
        ports.extend([
            PortArg::new("s_axis_tdata", Expr::ident(format!("{fifo_name}_TDATA"))),
            PortArg::new("s_axis_tvalid", Expr::ident(format!("{fifo_name}_TVALID"))),
            PortArg::new("s_axis_tready", Expr::ident(format!("{fifo_name}_TREADY"))),
            PortArg::new("s_axis_tlast", Expr::ident(format!("{fifo_name}_TLAST"))),
            PortArg::new("m_stream_dout", Expr::ident(format!("{fifo_name}_dout"))),
            PortArg::new(
                "m_stream_empty_n",
                Expr::ident(format!("{fifo_name}_empty_n")),
            ),
            PortArg::new("m_stream_read", Expr::ident(format!("{fifo_name}_read"))),
        ]);
    } else {
        ports.extend([
            PortArg::new("s_stream_din", Expr::ident(format!("{fifo_name}_din"))),
            PortArg::new(
                "s_stream_full_n",
                Expr::ident(format!("{fifo_name}_full_n")),
            ),
            PortArg::new("s_stream_write", Expr::ident(format!("{fifo_name}_write"))),
            PortArg::new("m_axis_tdata", Expr::ident(format!("{fifo_name}_TDATA"))),
            PortArg::new("m_axis_tvalid", Expr::ident(format!("{fifo_name}_TVALID"))),
            PortArg::new("m_axis_tready", Expr::ident(format!("{fifo_name}_TREADY"))),
            PortArg::new("m_axis_tlast", Expr::ident(format!("{fifo_name}_TLAST"))),
        ]);
    }

    ModuleInstance::new(module_name, instance_name)
        .with_params(vec![ParamArg::new(
            "DATA_WIDTH",
            Expr::int(u64::from(data_width)),
        )])
        .with_ports(ports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_instance_has_params() {
        let inst = build_fifo_instance("data_q", Expr::ident("ap_rst"), Expr::int(32), 16);
        assert_eq!(inst.module_name, "fifo");
        assert_eq!(inst.instance_name, "data_q_fifo");
        assert_eq!(inst.params.len(), 3);
        // clk + reset + 4 read + 4 write = 10
        assert_eq!(inst.ports.len(), 10);
        let text = inst.to_string();
        assert!(text.contains(".ADDR_WIDTH(4)"), "got:\n{text}");
        assert!(text.contains(".if_read_ce(1'b1)"), "got:\n{text}");
        assert!(text.contains(".if_write_ce(1'b1)"), "got:\n{text}");
    }

    #[test]
    fn fifo_instance_addr_width_matches_depth() {
        let inst = build_fifo_instance("deep_q", Expr::ident("ap_rst"), Expr::int(65), 4096);
        let text = inst.to_string();
        assert!(text.contains(".ADDR_WIDTH(12)"), "got:\n{text}");
        assert!(text.contains(".DEPTH(4096)"), "got:\n{text}");
    }

    #[test]
    fn fifo_instance_sanitizes_indexed_names() {
        let inst = build_fifo_instance("qs[24]_Network", Expr::ident("ap_rst"), Expr::int(32), 16);
        let text = inst.to_string();
        assert!(text.contains("qs_24_Network_fifo"), "got:\n{text}");
        assert!(
            text.contains(".if_dout(qs_24_Network_dout)"),
            "got:\n{text}"
        );
        assert!(!text.contains("qs[24]"), "got:\n{text}");
    }

    #[test]
    fn external_fifo_assigns_when_renamed() {
        let assigns = build_external_fifo_assigns("int_fifo", "ext_fifo", true);
        assert_eq!(assigns.len(), ISTREAM_SUFFIXES.len());
        // Input dir (_dout): assign int_fifo_dout = ext_fifo_dout
        let text = assigns[0].to_string();
        assert!(text.contains("int_fifo"), "got: {text}");
        assert!(text.contains("ext_fifo"), "got: {text}");
    }

    #[test]
    fn external_fifo_no_assigns_when_same_name() {
        let assigns = build_external_fifo_assigns("fifo_0", "fifo_0", true);
        assert!(assigns.is_empty(), "same name should produce no assigns");
    }

    #[test]
    fn axis_input_adapter() {
        let inst = build_axis_adapter("data_in", 32, true);
        assert_eq!(inst.module_name, "axis_to_stream_adapter");
        let text = inst.to_string();
        assert!(text.contains(".DATA_WIDTH(32)"), "got:\n{text}");
        assert!(
            text.contains(".s_axis_tdata(data_in_TDATA)"),
            "got:\n{text}"
        );
        assert!(
            text.contains(".m_stream_dout(data_in_dout)"),
            "got:\n{text}"
        );
    }

    #[test]
    fn axis_output_adapter() {
        let inst = build_axis_adapter("data_out", 64, false);
        assert_eq!(inst.module_name, "stream_to_axis_adapter");
        let text = inst.to_string();
        assert!(text.contains(".DATA_WIDTH(64)"), "got:\n{text}");
        assert!(text.contains(".s_stream_din(data_out_din)"), "got:\n{text}");
        assert!(
            text.contains(".m_axis_tlast(data_out_TLAST)"),
            "got:\n{text}"
        );
    }
}
