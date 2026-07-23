//! Async mmap bridge wiring.
//!
//! Lower HLS tasks expose `tapa::async_mmap` as FIFO-style channels
//! (`mem_read_addr_s_din`, `mem_read_data_s_dout`, etc.).  Parent tasks
//! connect those channels to the shared AXI fabric through `async_mmap`.

use std::collections::BTreeSet;

use tapa_protocol::{
    HANDSHAKE_CLK, ISTREAM_SUFFIXES, M_AXI_PORTS, M_AXI_PREFIX, M_AXI_SUFFIXES_COMPACT,
    OSTREAM_SUFFIXES,
};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{wide_wire, wire, MutableModule};
use tapa_rtl::VerilogModule;

pub const READ_ADDR: &str = "read_addr";
pub const READ_DATA: &str = "read_data";
pub const WRITE_ADDR: &str = "write_addr";
pub const WRITE_DATA: &str = "write_data";
pub const WRITE_RESP: &str = "write_resp";
pub const AXI_ADDR_WIDTH: u32 = 64;
pub const AXI_ID_WIDTH: u32 = 1;

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

fn tag_data_width(tag: &str, suffix: &str, data_width: u32) -> u32 {
    if !matches!(suffix, "_din" | "_dout") {
        return 1;
    }
    match tag {
        READ_ADDR | WRITE_ADDR => 64,
        WRITE_RESP => 8,
        _ => data_width,
    }
}

pub fn signal_name(base: &str, tag: &str, suffix: &str) -> String {
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
    let source = strip_verilog_comments(&child_rtl.source);
    let group_enabled = |group: &[(&str, &str)]| {
        for &(tag, activity_suffix) in group {
            if !tags.contains(tag) {
                continue;
            }
            let prefix = format!("{child_port}_{tag}");
            let Some(port) = child_rtl.find_port_by_affixes(&prefix, activity_suffix) else {
                return true;
            };
            if !has_unconditional_zero_assign(&source, &port.name) {
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
        || M_AXI_SUFFIXES_COMPACT.iter().any(|suffix| {
            child_rtl
                .find_port(&format!("{M_AXI_PREFIX}{child_port}{suffix}"))
                .is_some()
        })
}

fn strip_verilog_comments(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '/' {
            result.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('/') => {
                let _ = chars.next();
                for comment in chars.by_ref() {
                    if comment == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            }
            Some('*') => {
                let _ = chars.next();
                let mut previous = '\0';
                for comment in chars.by_ref() {
                    if previous == '*' && comment == '/' {
                        break;
                    }
                    previous = comment;
                }
                result.push(' ');
            }
            _ => result.push(ch),
        }
    }
    result
}

fn has_unconditional_zero_assign(source: &str, port: &str) -> bool {
    source.split(';').any(|statement| {
        let statement = statement.trim();
        let Some(assign) = statement.strip_prefix("assign") else {
            return false;
        };
        let Some((lhs, rhs)) = assign.split_once('=') else {
            return false;
        };
        lhs.trim() == port && is_zero_literal(rhs.trim())
    })
}

fn is_zero_literal(literal: &str) -> bool {
    let mut literal = literal.trim();
    while let Some(inner) = literal
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        literal = inner.trim();
    }
    let compact = literal.replace('_', "");
    if compact == "0" || compact == "'0" {
        return true;
    }
    let Some((width, value)) = compact.split_once('\'') else {
        return false;
    };
    if width.is_empty() || !width.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    let value = value
        .strip_prefix('s')
        .or_else(|| value.strip_prefix('S'))
        .unwrap_or(value);
    let Some(digits) = value
        .get(1..)
        .filter(|_| value.starts_with(['b', 'B', 'o', 'O', 'd', 'D', 'h', 'H']))
    else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|ch| ch == '0')
}

pub fn child_connection_expr(base: &str, tag: &str, suffix: &str) -> Expr {
    let signal = Expr::ident(signal_name(base, tag, suffix));
    if matches!((tag, suffix), (READ_DATA | WRITE_RESP, "_dout")) {
        Expr::concat(vec![Expr::lit("1'b0"), signal])
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
) {
    for &tag in active_tags {
        for &suffix in tag_suffixes(tag) {
            let name = signal_name(bridge_base, tag, suffix);
            let width = tag_data_width(tag, suffix, data_width);
            let signal = if width > 1 {
                wide_wire(&name, &(width - 1).to_string(), "0")
            } else {
                wire(&name)
            };
            let _ = module.add_signal(signal);
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
    connect_optional_axi_ports: bool,
    reset: Expr,
) -> ModuleInstance {
    let bytes = data_width / 8;
    let bytes_log = if bytes <= 1 {
        0
    } else {
        (bytes - 1).ilog2() + 1
    };
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
        ParamArg::new("BurstLenWidth", Expr::int(9)),
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
        if let Some(subports) = M_AXI_PORTS.get(channel) {
            for &(subport, _) in *subports {
                let suffix = format!("_{channel}{subport}");
                let is_compact = M_AXI_SUFFIXES_COMPACT.contains(&suffix.as_str());
                let connect_optional = connect_optional_axi_ports
                    && (!channel_enabled || active_m_axi_prefix == inactive_m_axi_prefix);
                let connection = if is_compact || connect_optional {
                    Expr::ident(format!("{channel_prefix}{suffix}"))
                } else {
                    Expr::lit("")
                };
                ports.push(PortArg::new(
                    format!("m_axi_{channel}{subport}"),
                    connection,
                ));
            }
        }
    }

    for &tag in TAGS {
        for &suffix in tag_suffixes(tag) {
            let connection = if active_tags.contains(tag) {
                Expr::ident(signal_name(bridge_base, tag, suffix))
            } else if suffix.ends_with("_read") || suffix.ends_with("_write") {
                Expr::lit("1'b0")
            } else if suffix.ends_with("_din") {
                Expr::lit("'d0")
            } else {
                Expr::lit("")
            };
            ports.push(PortArg::new(format!("{tag}{suffix}"), connection));
        }
    }

    ModuleInstance::new("async_mmap", instance_name)
        .with_params(std::mem::take(&mut params))
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
    fn add_bridge_signals_creates_wires() {
        let module = VerilogModule::parse("module top(); endmodule").unwrap();
        let mut mm = tapa_rtl::mutation::MutableModule::from_parsed(module);
        let mut tags = BTreeSet::new();
        tags.insert(READ_ADDR);
        tags.insert(READ_DATA);
        add_bridge_signals(&mut mm, "chan", &tags, 64);
        let emitted = mm.emit();
        assert!(emitted.contains("chan_read_addr__din"), "got:\n{emitted}");
        assert!(emitted.contains("chan_read_data__dout"), "got:\n{emitted}");
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
            true,
            Expr::ident("bridge_reset"),
        );
        let text = inst.to_string();
        assert!(text.contains("async_mmap"), "got:\n{text}");
        assert!(text.contains("DataWidth(512)"), "got:\n{text}");
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
