//! Async mmap bridge wiring.
//!
//! Lower HLS tasks expose `tapa::async_mmap` as FIFO-style channels
//! (`mem_read_addr_s_din`, `mem_read_data_s_dout`, etc.).  Parent tasks
//! connect those channels to the shared AXI fabric through `async_mmap`.

use std::collections::BTreeSet;

use tapa_protocol::{ISTREAM_SUFFIXES, M_AXI_PORTS, OSTREAM_SUFFIXES};
use tapa_rtl::builder::{Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{wide_wire, wire, MutableModule};
use tapa_rtl::VerilogModule;

pub const READ_ADDR: &str = "read_addr";
pub const READ_DATA: &str = "read_data";
pub const WRITE_ADDR: &str = "write_addr";
pub const WRITE_DATA: &str = "write_data";
pub const WRITE_RESP: &str = "write_resp";

const TAGS: &[&str] = &[READ_ADDR, READ_DATA, WRITE_ADDR, WRITE_DATA, WRITE_RESP];

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
    prefix.strip_prefix("m_axi_").unwrap_or(prefix).to_owned()
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

pub fn build_bridge_instance(
    bridge_base: &str,
    m_axi_prefix: &str,
    active_tags: &BTreeSet<&'static str>,
    data_width: u32,
    connect_optional_axi_ports: bool,
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
    let enable_read = active_tags.contains(READ_ADDR) || active_tags.contains(READ_DATA);
    let enable_write = active_tags.contains(WRITE_ADDR)
        || active_tags.contains(WRITE_DATA)
        || active_tags.contains(WRITE_RESP);

    let mut params = vec![
        ParamArg::new("DataWidth", Expr::int(u64::from(data_width))),
        ParamArg::new("DataWidthBytesLog", Expr::int(u64::from(bytes_log))),
        ParamArg::new("AddrWidth", Expr::int(64)),
        ParamArg::new("WaitTimeWidth", Expr::int(2)),
        ParamArg::new("MaxWaitTime", Expr::int(3)),
        ParamArg::new("BurstLenWidth", Expr::int(9)),
        ParamArg::new("MaxBurstLen", Expr::int(u64::from(max_burst_len))),
        ParamArg::new("EnableReadChannel", Expr::int(u64::from(enable_read as u8))),
        ParamArg::new(
            "EnableWriteChannel",
            Expr::int(u64::from(enable_write as u8)),
        ),
    ];

    let mut ports = vec![
        PortArg::new("clk", Expr::ident("ap_clk")),
        PortArg::new("rst", Expr::ident("ap_rst")),
    ];

    for channel in ["AW", "W", "B", "AR", "R"] {
        if let Some(subports) = M_AXI_PORTS.get(channel) {
            for &(subport, _) in *subports {
                let suffix = format!("_{channel}{subport}");
                let is_compact = tapa_protocol::M_AXI_SUFFIXES_COMPACT
                    .iter()
                    .any(|compact| *compact == suffix);
                let connection = if is_compact || connect_optional_axi_ports {
                    Expr::ident(format!("{m_axi_prefix}{suffix}"))
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

    ModuleInstance::new("async_mmap", format!("{bridge_base}__m_axi"))
        .with_params(std::mem::take(&mut params))
        .with_ports(ports)
}
