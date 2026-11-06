//! Async mmap bridge wiring.
//!
//! Lower HLS tasks expose `tapa::async_mmap` as FIFO-style channels
//! (`mem_read_addr_s_din`, `mem_read_data_s_dout`, etc.).  Parent tasks
//! connect those channels to the shared AXI fabric through `async_mmap`.

use std::collections::BTreeSet;

use tapa_protocol::{
    m_axi_channel, m_axi_compact_suffixes, AXI_ADDR_WIDTH, HANDSHAKE_CLK, ISTREAM_SUFFIXES,
    M_AXI_PREFIX, OSTREAM_SUFFIXES, STREAM_DATA_SUFFIXES,
};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::MutableModule;
use tapa_rtl::VerilogModule;

use crate::rtl_state::routing_id_bits;

const READ_ADDR: &str = "read_addr";
const READ_DATA: &str = "read_data";
const WRITE_ADDR: &str = "write_addr";
const WRITE_DATA: &str = "write_data";
const WRITE_RESP: &str = "write_resp";

const TAGS: &[&str] = &[READ_ADDR, READ_DATA, WRITE_ADDR, WRITE_DATA, WRITE_RESP];

/// AXI directions that survive synthesis in one FIFO-style async mmap leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnabledAxiDirections {
    pub read: bool,
    pub write: bool,
}

fn tag_suffixes(tag: &str) -> &'static [&'static str] {
    match tag {
        READ_ADDR | WRITE_ADDR | WRITE_DATA => OSTREAM_SUFFIXES,
        READ_DATA | WRITE_RESP => ISTREAM_SUFFIXES,
        _ => &[],
    }
}

fn tag_data_width(tag: &str, suffix: &str, data_width: u32, request_width: u32) -> u32 {
    if !STREAM_DATA_SUFFIXES.contains(&suffix) {
        return 1;
    }
    match (tag, suffix) {
        (READ_ADDR | WRITE_ADDR, _) => request_width,
        (WRITE_RESP, _) => 8,
        // `tapa::ostream<T>` carries EoT in the MSB. The AXI bridge consumes
        // only T, but the child-facing wire must retain the complete port.
        (WRITE_DATA, "_din") => data_width.saturating_add(1),
        _ => data_width,
    }
}

fn signal_name(base: &str, tag: &str, suffix: &str) -> String {
    let suffix = suffix.trim_start_matches('_');
    format!("{}_{}__{suffix}", sanitize_array_name(base), tag)
}

pub fn bridge_base_from_m_axi_prefix(prefix: &str) -> String {
    prefix
        .strip_prefix(M_AXI_PREFIX)
        .unwrap_or(prefix)
        .to_owned()
}

pub fn active_tags(child_rtl: &VerilogModule, child_port: &str) -> BTreeSet<&'static str> {
    TAGS.iter()
        .copied()
        .filter(|tag| {
            let prefix = format!("{child_port}_{tag}");
            tag_suffixes(tag).iter().any(|suffix| {
                child_rtl.find_port_by_affixes(&prefix, suffix).is_some()
                    || child_rtl
                        .find_port_by_affixes(&format!("{prefix}_peek"), suffix)
                        .is_some()
            }) || child_rtl.find_port_by_affixes(&prefix, "_offset").is_some()
        })
        .collect()
}

/// Bits of one address request of a FIFO-style async mmap: an element
/// address, or `{count, address}` when the task's TAPA headers issue bursts.
/// Read from the child's own ports, so RTL built before bursts keeps
/// one-address requests.
pub fn request_width(child_rtl: &VerilogModule, child_port: &str) -> u32 {
    [READ_ADDR, WRITE_ADDR]
        .iter()
        .filter_map(|tag| {
            child_rtl
                .find_port_by_affixes(&format!("{child_port}_{tag}"), "_din")
                .and_then(tapa_rtl::port::Port::bit_width)
        })
        .max()
        .unwrap_or(AXI_ADDR_WIDTH)
}

/// Conservatively determine which AXI halves a FIFO-style async mmap uses.
///
/// Vitis HLS can retain every async FIFO port even when one direction is
/// unused. A direction is disabled only when every present activity output in
/// that group is an unconditional continuous assignment to zero. Missing or
/// otherwise ambiguous activity outputs keep a present group enabled.
pub fn enabled_axi_directions(
    child_rtl: &VerilogModule,
    child_port: &str,
    tags: &BTreeSet<&'static str>,
) -> EnabledAxiDirections {
    let zero_driven = child_rtl.unconditionally_zero_driven_names();
    let group_enabled = |group: &[(&str, &str)]| {
        for &(tag, activity_suffix) in group {
            if !tags.contains(tag) {
                continue;
            }
            let prefix = format!("{child_port}_{tag}");
            let Some(port) = child_rtl.find_port_by_affixes(&prefix, activity_suffix) else {
                return true;
            };
            if !zero_driven.contains(&port.name) {
                return true;
            }
        }
        false
    };

    EnabledAxiDirections {
        read: group_enabled(&[(READ_ADDR, "_write"), (READ_DATA, "_read")]),
        write: group_enabled(&[
            (WRITE_ADDR, "_write"),
            (WRITE_DATA, "_write"),
            (WRITE_RESP, "_read"),
        ]),
    }
}

/// Whether codegen will bind this async mmap as a direct compact M-AXI child
/// instead of inserting the FIFO-to-AXI bridge.
pub fn has_direct_m_axi_ports(child_rtl: &VerilogModule, child_port: &str) -> bool {
    child_rtl
        .find_port(&format!("{child_port}_offset"))
        .is_some()
        || m_axi_compact_suffixes().any(|suffix| {
            child_rtl
                .find_port(&format!("{M_AXI_PREFIX}{child_port}{suffix}"))
                .is_some()
        })
}

fn child_connection_expr(base: &str, tag: &str, suffix: &str) -> Expr {
    let signal = Expr::ident(signal_name(base, tag, suffix));
    if matches!((tag, suffix), (READ_DATA | WRITE_RESP, "_dout")) {
        Expr::concat(vec![Expr::lit("1'b0"), signal])
    } else {
        signal
    }
}

fn bridge_connection_expr(base: &str, tag: &str, suffix: &str, data_width: u32) -> Expr {
    let signal = Expr::ident(signal_name(base, tag, suffix));
    if matches!((tag, suffix), (WRITE_DATA, "_din")) {
        Expr::range(
            signal,
            Expr::int(u64::from(data_width.saturating_sub(1))),
            Expr::int(0),
        )
    } else {
        signal
    }
}

pub fn child_portargs(
    child_rtl: &VerilogModule,
    child_port: &str,
    bridge_base: &str,
    offset_signal: &str,
) -> Vec<PortArg> {
    let mut ports = Vec::new();
    for &tag in TAGS {
        let prefix = format!("{child_port}_{tag}");
        for &suffix in tag_suffixes(tag) {
            if let Some(port) = child_rtl.find_port_by_affixes(&prefix, suffix) {
                ports.push(PortArg::new(
                    port.name.clone(),
                    child_connection_expr(bridge_base, tag, suffix),
                ));
            }
            if matches!(tag, READ_DATA | WRITE_RESP) {
                let peek_prefix = format!("{prefix}_peek");
                if matches!(suffix, "_dout" | "_empty_n") {
                    if let Some(port) = child_rtl.find_port_by_affixes(&peek_prefix, suffix) {
                        ports.push(PortArg::new(
                            port.name.clone(),
                            child_connection_expr(bridge_base, tag, suffix),
                        ));
                    }
                }
            }
        }
        // The unified HLS stream type leaves an input-only peek member on
        // output channels. It has no protocol role for an ostream, but Vitis
        // retains it on some async-mmap channels, so drive the artifact.
        if matches!(tag, READ_ADDR | WRITE_ADDR | WRITE_DATA) {
            if let Some(port) = child_rtl.find_port(&format!("{prefix}_peek")) {
                ports.push(PortArg::new(port.name.clone(), Expr::lit("'d0")));
            }
        }
        if tag.ends_with("_addr") {
            if let Some(port) = child_rtl.find_port_by_affixes(&prefix, "_offset") {
                ports.push(PortArg::new(port.name.clone(), Expr::ident(offset_signal)));
            }
        }
    }
    ports
}

pub fn add_bridge_signals(
    module: &mut MutableModule,
    bridge_base: &str,
    active_tags: &BTreeSet<&'static str>,
    data_width: u32,
    request_width: u32,
) {
    for &tag in active_tags {
        for &suffix in tag_suffixes(tag) {
            let name = signal_name(bridge_base, tag, suffix);
            let width = tag_data_width(tag, suffix, data_width, request_width);
            let _ = module.add_signal(super::width_wire(&name, width));
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "bridge wiring keeps the active and inactive AXI paths explicit"
)]
pub fn build_bridge_instance(
    bridge_base: &str,
    instance_name: &str,
    active_m_axi_prefix: &str,
    inactive_m_axi_prefix: &str,
    active_tags: &BTreeSet<&'static str>,
    enabled: EnabledAxiDirections,
    data_width: u32,
    request_width: u32,
    connect_optional_axi_ports: bool,
    reset: Expr,
) -> ModuleInstance {
    let bytes = data_width / 8;
    let bytes_log = routing_id_bits(bytes);
    let max_burst_len = 8192_u32
        .checked_div(data_width)
        .unwrap_or(0)
        .saturating_sub(1);
    let mut params = vec![
        ParamArg::new("DataWidth", Expr::int(u64::from(data_width))),
        ParamArg::new("DataWidthBytesLog", Expr::int(u64::from(bytes_log))),
        ParamArg::new("AddrWidth", Expr::int(u64::from(AXI_ADDR_WIDTH))),
        ParamArg::new("WaitTimeWidth", Expr::int(2)),
        ParamArg::new("MaxWaitTime", Expr::int(3)),
        ParamArg::new("BurstLenWidth", Expr::int(8)),
        ParamArg::new("MaxBurstLen", Expr::int(u64::from(max_burst_len))),
        ParamArg::new(
            "EnableReadChannel",
            Expr::int(u64::from(u8::from(enabled.read))),
        ),
        ParamArg::new(
            "EnableWriteChannel",
            Expr::int(u64::from(u8::from(enabled.write))),
        ),
    ];
    if request_width > AXI_ADDR_WIDTH {
        params.push(ParamArg::new(
            "ReqLenWidth",
            Expr::int(u64::from(request_width - AXI_ADDR_WIDTH)),
        ));
    }

    let mut ports = vec![
        PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("rst", reset),
    ];

    for channel in ["AW", "W", "B", "AR", "R"] {
        let channel_enabled = match channel {
            "AR" | "R" => enabled.read,
            "AW" | "W" | "B" => enabled.write,
            _ => unreachable!("known AXI channel"),
        };
        let channel_prefix = if channel_enabled {
            active_m_axi_prefix
        } else {
            inactive_m_axi_prefix
        };
        if let Some(info) = m_axi_channel(channel) {
            for port in info.ports {
                let suffix = port.suffix;
                let connect_optional = connect_optional_axi_ports
                    && (!channel_enabled || active_m_axi_prefix == inactive_m_axi_prefix);
                let connection = if port.is_compact || connect_optional {
                    Expr::ident(format!("{channel_prefix}{suffix}"))
                } else {
                    Expr::lit("")
                };
                ports.push(PortArg::new(format!("m_axi{suffix}"), connection));
            }
        }
    }

    for &tag in TAGS {
        for &suffix in tag_suffixes(tag) {
            let connection = if active_tags.contains(tag) {
                bridge_connection_expr(bridge_base, tag, suffix, data_width)
            } else if suffix.ends_with("_read") || suffix.ends_with("_write") {
                Expr::lit("1'b0")
            } else if suffix == "_din" {
                Expr::lit("'d0")
            } else {
                Expr::lit("")
            };
            ports.push(PortArg::new(format!("{tag}{suffix}"), connection));
        }
    }

    ModuleInstance::new("async_mmap", instance_name)
        .with_params(params)
        .with_ports(ports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tapa_rtl::VerilogModule;

    #[test]
    fn active_tags_detects_present_ports() {
        let module = VerilogModule::parse(
            "module child(\n\
             input wire ap_clk,\n\
             output wire [63:0] mem_read_addr_s_din,\n\
             input wire mem_read_data_s_dout,\n\
             output wire mem_write_addr_s_din,\n\
             output wire mem_write_data_s_din,\n\
             input wire mem_write_resp_s_dout\n\
             ); endmodule",
        )
        .unwrap();
        let tags = active_tags(&module, "mem");
        assert!(tags.contains(READ_ADDR));
        assert!(tags.contains(READ_DATA));
        assert!(tags.contains(WRITE_ADDR));
        assert!(tags.contains(WRITE_DATA));
        assert!(tags.contains(WRITE_RESP));
    }

    #[test]
    fn active_tags_empty_when_no_ports() {
        let module = VerilogModule::parse("module child(input wire ap_clk); endmodule").unwrap();
        let tags = active_tags(&module, "mem");
        assert!(tags.is_empty());
    }

    #[test]
    fn enabled_directions_only_prunes_proven_zero_activity() {
        let module = VerilogModule::parse(
            "module child(\n\
             output wire mem_read_addr_s_write,\n\
             output wire mem_read_data_s_read,\n\
             output wire mem_write_addr_s_write,\n\
             output wire mem_write_data_s_write,\n\
             output wire mem_write_resp_s_read\n\
             );\n\
             assign mem_read_addr_s_write = live_read;\n\
             assign mem_read_data_s_read = live_read;\n\
             assign mem_write_addr_s_write = 1'b0;\n\
             assign mem_write_data_s_write = 1'h0;\n\
             assign mem_write_resp_s_read = 1'd0;\n\
             endmodule",
        )
        .unwrap();
        let tags = active_tags(&module, "mem");

        assert_eq!(
            enabled_axi_directions(&module, "mem", &tags),
            EnabledAxiDirections {
                read: true,
                write: false,
            }
        );
    }

    #[test]
    fn commented_or_indirect_zero_assign_is_not_proof() {
        let module = VerilogModule::parse(
            "module child(\n\
             output wire mem_read_addr_s_write,\n\
             output wire mem_read_data_s_read\n\
             );\n\
             // assign mem_read_addr_s_write = 1'b0;\n\
             assign mem_read_addr_s_write = disabled;\n\
             assign disabled = 1'b0;\n\
             /* assign mem_read_data_s_read = 1'b0; */\n\
             assign mem_read_data_s_read = disabled;\n\
             endmodule",
        )
        .unwrap();
        let tags = active_tags(&module, "mem");

        assert!(enabled_axi_directions(&module, "mem", &tags).read);
    }

    #[test]
    fn conditional_or_procedural_zero_activity_stays_enabled() {
        for body in [
            "generate if (ENABLE) begin : conditional\n\
             assign unused = 1'b0;\n\
             assign mem_read_addr_s_write = 1'b0;\n\
             assign mem_read_data_s_read = 1'b0;\n\
             end endgenerate",
            "initial begin\n\
             mem_read_addr_s_write = 1'b0;\n\
             mem_read_data_s_read = 1'b0;\n\
             end",
            "assign mem_read_addr_s_write = 1'b0;\n\
             assign mem_read_data_s_read = 1'b0;\n\
             always @(*) mem_read_addr_s_write = live_read;",
        ] {
            let source = format!(
                "module child #(parameter ENABLE = 1)(\n\
                 output wire mem_read_addr_s_write,\n\
                 output wire mem_read_data_s_read\n\
                 );\n{body}\nendmodule"
            );
            let module = VerilogModule::parse(&source).unwrap();
            let tags = active_tags(&module, "mem");
            assert!(
                enabled_axi_directions(&module, "mem", &tags).read,
                "ambiguous activity must keep reads enabled:\n{source}"
            );
        }
    }

    #[test]
    fn child_connection_expr_prepends_eot_for_data() {
        let expr = child_connection_expr("base", READ_DATA, "_dout");
        let text = expr.to_string();
        assert!(text.contains("1'b0"), "expected EOT prepend, got: {text}");
        assert!(text.contains("base_read_data__dout"), "got: {text}");
    }

    #[test]
    fn child_connection_expr_no_eot_for_addr() {
        let expr = child_connection_expr("base", READ_ADDR, "_din");
        let text = expr.to_string();
        assert_eq!(text, "base_read_addr__din");
    }

    #[test]
    fn bridge_discards_write_data_eot_explicitly() {
        let expr = bridge_connection_expr("base", WRITE_DATA, "_din", 512);
        assert_eq!(expr.to_string(), "base_write_data__din[511:0]");
    }

    #[test]
    fn child_portargs_maps_peek_for_istream() {
        let module = VerilogModule::parse(
            "module child(\n\
             input wire ap_clk,\n\
             input wire [63:0] mem_read_data_s_dout,\n\
             input wire mem_read_data_s_empty_n,\n\
             output wire mem_read_data_s_read,\n\
             input wire [63:0] mem_read_data_peek_dout,\n\
             input wire mem_read_data_peek_empty_n\n\
             ); endmodule",
        )
        .unwrap();
        let ports = child_portargs(&module, "mem", "chan", "chan_offset");
        let names: Vec<_> = ports.iter().map(|p| p.port_name.clone()).collect();
        assert!(names.contains(&"mem_read_data_s_dout".to_string()));
        assert!(names.contains(&"mem_read_data_peek_dout".to_string()));
    }

    #[test]
    fn child_portargs_ties_off_ostream_peek_artifact() {
        let module = VerilogModule::parse(
            "module child(\n\
             output wire [64:0] mem_write_data_s_din,\n\
             input wire mem_write_data_s_full_n,\n\
             output wire mem_write_data_s_write,\n\
             input wire [64:0] mem_write_data_peek\n\
             ); endmodule",
        )
        .unwrap();
        let ports = child_portargs(&module, "mem", "chan", "chan_offset");
        let peek = ports
            .iter()
            .find(|port| port.port_name == "mem_write_data_peek")
            .expect("output peek binding");
        assert_eq!(peek.connection.to_string(), "'d0");
    }

    #[test]
    fn a_burst_request_port_expands_in_the_bridge() {
        let module = VerilogModule::parse(
            "module child(\n\
             output wire [95:0] mem_read_addr_s_din,\n\
             input wire [512:0] mem_read_data_s_dout\n\
             ); endmodule",
        )
        .unwrap();
        let width = request_width(&module, "mem");
        assert_eq!(width, 96);
        let tags = active_tags(&module, "mem");
        let inst = build_bridge_instance(
            "chan",
            "chan__m_axi",
            "m_axi_chan",
            "m_axi_chan",
            &tags,
            EnabledAxiDirections {
                read: true,
                write: false,
            },
            512,
            width,
            true,
            Expr::ident("bridge_reset"),
        );
        assert!(inst.to_string().contains("ReqLenWidth(32)"), "got:\n{inst}");
    }

    #[test]
    fn add_bridge_signals_creates_wires() {
        let module = VerilogModule::parse("module top(); endmodule").unwrap();
        let mut mm = tapa_rtl::mutation::MutableModule::from_parsed(module);
        let mut tags = BTreeSet::new();
        tags.insert(READ_ADDR);
        tags.insert(READ_DATA);
        tags.insert(WRITE_DATA);
        add_bridge_signals(&mut mm, "chan", &tags, 64, AXI_ADDR_WIDTH);
        let emitted = mm.emit();
        assert!(emitted.contains("chan_read_addr__din"), "got:\n{emitted}");
        assert!(emitted.contains("chan_read_data__dout"), "got:\n{emitted}");
        assert!(
            emitted.contains("wire [64:0] chan_write_data__din"),
            "write-data EoT bit must reach the child:\n{emitted}"
        );
    }

    #[test]
    fn build_bridge_instance_has_async_mmap_params() {
        let mut tags = BTreeSet::new();
        tags.insert(READ_ADDR);
        tags.insert(READ_DATA);
        let inst = build_bridge_instance(
            "chan",
            "chan__m_axi",
            "__tapa_axi_chan_child",
            "m_axi_chan",
            &tags,
            EnabledAxiDirections {
                read: true,
                write: false,
            },
            512,
            AXI_ADDR_WIDTH,
            true,
            Expr::ident("bridge_reset"),
        );
        let text = inst.to_string();
        assert!(text.contains("async_mmap"), "got:\n{text}");
        assert!(!text.contains("ReqLenWidth"), "got:\n{text}");
        assert!(text.contains("DataWidth(512)"), "got:\n{text}");
        assert!(text.contains("BurstLenWidth(8)"), "got:\n{text}");
        assert!(text.contains("EnableReadChannel(1)"), "got:\n{text}");
        assert!(text.contains("EnableWriteChannel(0)"), "got:\n{text}");
        assert!(text.contains("chan__m_axi"), "got:\n{text}");
        assert!(text.contains(".rst(bridge_reset)"), "got:\n{text}");
        assert!(
            text.contains(".m_axi_ARADDR(__tapa_axi_chan_child_ARADDR)"),
            "got:\n{text}"
        );
        assert!(
            text.contains(".m_axi_AWADDR(m_axi_chan_AWADDR)"),
            "got:\n{text}"
        );
        assert!(text.contains(".m_axi_ARLOCK()"), "got:\n{text}");
        assert!(
            !text.contains("__tapa_axi_chan_child_ARLOCK"),
            "routed optional outputs must not create implicit wires:\n{text}"
        );
    }

    #[test]
    fn bridge_base_from_prefix_strips_m_axi() {
        assert_eq!(bridge_base_from_m_axi_prefix("m_axi_mem"), "mem");
        assert_eq!(bridge_base_from_m_axi_prefix("mem"), "mem");
    }
}
