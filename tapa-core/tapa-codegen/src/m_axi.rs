//! M-AXI port generation and parameterized AXI crossbar emission.

use tapa_protocol::{
    PortDir, HANDSHAKE_CLK, HANDSHAKE_RST, M_AXI_PORTS, M_AXI_PORT_WIDTHS, M_AXI_PREFIX,
    M_AXI_SUFFIXES_COMPACT,
};
use tapa_rtl::builder::{ContinuousAssign, Expr, ModuleInstance, ParamArg, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::mutation::{simple_port, wide_port, MutableModule};
use tapa_rtl::port::Direction;

use crate::children;
use crate::error::CodegenError;
use crate::rtl_state::TopologyWithRtl;
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

/// Determine if an AXI crossbar is needed for an mmap connection:
/// multiple child ports share the argument, or the port is an hmap
/// (any explicit channel count, including 1).
pub fn needs_crossbar(conn: &MMapConnection) -> bool {
    conn.thread_count() > 1 || conn.chan_count.is_some()
}

/// Build crossbar module name: `axi_crossbar_{slaves}x{channels}`.
pub fn crossbar_module_name(conn: &MMapConnection) -> String {
    format!(
        "axi_crossbar_{}x{}",
        conn.thread_count(),
        conn.channel_count()
    )
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
#[must_use]
pub fn build_crossbar_params(conn: &MMapConnection) -> Vec<ParamArg> {
    try_build_crossbar_params(conn).expect("invalid mmap connection")
}

/// Build crossbar parameter arguments after validating the connection.
pub fn try_build_crossbar_params(conn: &MMapConnection) -> Result<Vec<ParamArg>, CodegenError> {
    validate_mmap_connection(conn)?;
    let addr_width = try_get_addr_width(conn.chan_size, conn.data_width)?;
    let mut params = vec![
        ParamArg::new("DATA_WIDTH", Expr::int(u64::from(conn.data_width))),
        ParamArg::new("ADDR_WIDTH", Expr::int(64)),
        ParamArg::new(
            "S_ID_WIDTH",
            Expr::int(u64::from(crossbar_slave_id_width(conn))),
        ),
        ParamArg::new("M_ID_WIDTH", Expr::int(u64::from(conn.id_width()))),
    ];

    for idx in 0..conn.channel_count() {
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

    // Per-slave thread parameters: a leaf slave tracks 1 in-flight
    // thread; an upper child that internally shares the mmap needs its
    // aggregated total so the crossbar can track every outstanding ID.
    for (idx, slave) in conn.slaves.iter().enumerate() {
        params.push(ParamArg::new(
            format!("S{idx:02}_THREADS"),
            Expr::int(u64::from(slave.threads)),
        ));
    }

    Ok(params)
}

pub fn crossbar_slave_id_width(conn: &MMapConnection) -> u32 {
    conn.id_width()
        .saturating_sub(routing_id_bits(conn.thread_count()))
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
#[must_use]
pub fn build_crossbar_instance(conn: &MMapConnection) -> ModuleInstance {
    try_build_crossbar_instance(conn).expect("invalid mmap connection")
}

/// Build a crossbar instance after validating the connection.
pub fn try_build_crossbar_instance(conn: &MMapConnection) -> Result<ModuleInstance, CodegenError> {
    let module_name = crossbar_module_name(conn);
    let arg_name = sanitize_array_name(&conn.arg_name);
    let instance_name = format!("axi_crossbar__{arg_name}");
    let params = try_build_crossbar_params(conn)?;

    let mut ports = vec![
        PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("rst", Expr::ident(HANDSHAKE_RST)),
    ];

    // Upstream master ports.
    let chan_count = conn.channel_count();
    for channel_idx in 0..chan_count {
        let m_prefix = if chan_count > 1 {
            format!("{M_AXI_PREFIX}{arg_name}_{channel_idx}")
        } else {
            format!("{M_AXI_PREFIX}{arg_name}")
        };
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let signal = if chan_count > 1 && suffix.ends_with("ADDR") {
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
    for slave_idx in 0..conn.slaves.len() {
        let s_wire_prefix = crossbar_slave_prefix(&arg_name, slave_idx);
        for suffix in M_AXI_SUFFIXES_COMPACT {
            ports.push(PortArg::new(
                format!("s{slave_idx:02}{suffix}"),
                Expr::ident(format!("{s_wire_prefix}{suffix}")),
            ));
        }
    }

    Ok(ModuleInstance::new(module_name, instance_name)
        .with_params(params)
        .with_ports(ports))
}

/// Compute address width from a channel size and data width.
///
/// A zero channel size denotes a plain mmap with the full address space.
#[must_use]
pub fn get_addr_width(chan_size: u32, data_width: u32) -> u32 {
    if chan_size == 0 {
        return 64;
    }
    try_get_addr_width(Some(chan_size), data_width).expect("invalid mmap channel geometry")
}

/// Compute address width while reporting invalid channel geometry.
pub fn try_get_addr_width(chan_size: Option<u32>, data_width: u32) -> Result<u32, CodegenError> {
    if data_width == 0 || !data_width.is_multiple_of(8) {
        return Err(CodegenError::InvalidMmapConnection(format!(
            "M-AXI data width must be a nonzero multiple of 8 bits, got {data_width}"
        )));
    }
    let Some(chan_size) = chan_size else {
        return Ok(64);
    };
    let bytes = u64::from(chan_size) * u64::from(data_width / 8);
    if bytes == 0 {
        return Err(CodegenError::InvalidMmapConnection(
            "hmap channel size must be greater than zero".to_owned(),
        ));
    }
    if !bytes.is_power_of_two() {
        return Err(CodegenError::InvalidMmapConnection(format!(
            "hmap channel byte size must be a power of two: \
             chan_size={chan_size} * data_width={data_width} / 8 = {bytes} bytes"
        )));
    }
    Ok(bytes.ilog2())
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
    match (conn.chan_count, conn.chan_size) {
        (Some(0), _) => {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "hmap channel count is 0 for argument '{}'",
                conn.arg_name
            )));
        }
        (None, None) | (Some(_), Some(_)) => {}
        _ => {
            return Err(CodegenError::InvalidMmapConnection(format!(
                "hmap argument '{}' must specify both chan_count and chan_size",
                conn.arg_name
            )));
        }
    }
    if conn.slaves.iter().any(|slave| slave.threads == 0) {
        return Err(CodegenError::InvalidMmapConnection(format!(
            "M-AXI slave thread count is 0 for argument '{}'",
            conn.arg_name
        )));
    }
    try_get_addr_width(conn.chan_size, conn.data_width)?;
    if needs_crossbar(conn) && conn.slaves.is_empty() {
        return Err(CodegenError::InvalidMmapConnection(format!(
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
#[allow(
    clippy::too_many_lines,
    reason = "crossbar RTL emission is inherently sequential; \
              splitting would fragment the AXI channel wiring"
)]
pub fn generate_crossbar_rtl(conn: &MMapConnection) -> String {
    let module_name = crossbar_module_name(conn);
    let slaves = conn.thread_count();
    let channels = conn.channel_count();

    let mut params: Vec<String> = vec![
        "parameter DATA_WIDTH = 32".to_string(),
        "parameter ADDR_WIDTH = 64".to_string(),
        "parameter S_ID_WIDTH = 1".to_string(),
        format!("parameter M_ID_WIDTH = S_ID_WIDTH+$clog2({slaves})"),
    ];
    for idx in 0..channels {
        params.push(format!("parameter M{idx:02}_BASE_ADDR = 0"));
        params.push(format!("parameter M{idx:02}_ADDR_WIDTH = ADDR_WIDTH"));
        params.push(format!("parameter M{idx:02}_ISSUE = 16"));
    }
    for idx in 0..slaves {
        params.push(format!("parameter S{idx:02}_THREADS = 1"));
    }

    let mut ports: Vec<String> = vec!["input wire clk".to_string(), "input wire rst".to_string()];
    for ch_idx in 0..channels {
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let direction = if is_master_output_suffix(suffix) {
                "output"
            } else {
                "input"
            };
            let width = crossbar_port_width(suffix, true);
            ports.push(format!(
                "{direction} wire{width_decl} m{ch_idx:02}{suffix}",
                width_decl = width_decl(&width)
            ));
        }
    }
    for s_idx in 0..slaves {
        for suffix in M_AXI_SUFFIXES_COMPACT {
            let direction = if is_master_output_suffix(suffix) {
                "input"
            } else {
                "output"
            };
            let width = crossbar_port_width(suffix, false);
            ports.push(format!(
                "{direction} wire{width_decl} s{s_idx:02}{suffix}",
                width_decl = width_decl(&width)
            ));
        }
    }

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
    let connect_bits = channels * slaves;

    let axi_params: Vec<String> = vec![
        format!(".S_COUNT({slaves})"),
        format!(".M_COUNT({channels})"),
        ".DATA_WIDTH(DATA_WIDTH)".to_string(),
        ".ADDR_WIDTH(ADDR_WIDTH)".to_string(),
        ".S_ID_WIDTH(S_ID_WIDTH)".to_string(),
        ".M_ID_WIDTH(M_ID_WIDTH)".to_string(),
        format!(".S_THREADS({{{s_threads}}})"),
        format!(".S_ACCEPT({{{slaves}{{32'd16}}}})"),
        ".M_REGIONS(1)".to_string(),
        format!(".M_BASE_ADDR({{{m_base_addr}}})"),
        format!(".M_ADDR_WIDTH({{{m_addr_width}}})"),
        format!(".M_CONNECT_READ({{{connect_bits}{{1'b1}}}})"),
        format!(".M_CONNECT_WRITE({{{connect_bits}{{1'b1}}}})"),
        format!(".M_ISSUE({{{m_issue}}})"),
    ];

    let mut axi_ports: Vec<String> = vec![".clk(clk)".to_string(), ".rst(rst)".to_string()];

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
        axi_ports.push(format!(".s_axi_{name}({{{slaves}{{{value}}}}})"));
    }
    for suffix in M_AXI_SUFFIXES_COMPACT {
        let signal = concat_ports("s", slaves, suffix);
        let axi_suffix = suffix.trim_start_matches('_').to_ascii_lowercase();
        axi_ports.push(format!(".s_axi_{axi_suffix}({signal})"));
    }
    for (name, _value) in optional_addr_ports {
        axi_ports.push(format!(".m_axi_{name}()"));
    }
    for suffix in M_AXI_SUFFIXES_COMPACT {
        let signal = concat_ports("m", channels, suffix);
        let axi_suffix = suffix.trim_start_matches('_').to_ascii_lowercase();
        axi_ports.push(format!(".m_axi_{axi_suffix}({signal})"));
    }

    axi_ports.push(format!(".s_axi_awuser({slaves}'b0)"));
    axi_ports.push(format!(".s_axi_wuser({slaves}'b0)"));
    axi_ports.push(format!(".s_axi_aruser({slaves}'b0)"));
    axi_ports.push(".m_axi_awregion()".to_string());
    axi_ports.push(".m_axi_awuser()".to_string());
    axi_ports.push(".m_axi_arregion()".to_string());
    axi_ports.push(".m_axi_aruser()".to_string());
    axi_ports.push(".s_axi_buser()".to_string());
    axi_ports.push(".s_axi_ruser()".to_string());
    axi_ports.push(format!(".m_axi_buser({channels}'b0)"));
    axi_ports.push(format!(".m_axi_ruser({channels}'b0)"));

    let mut env = minijinja::Environment::new();
    env.add_template("crossbar_rtl", include_str!("templates/crossbar_rtl.v.j2"))
        .expect("template parses");
    env.get_template("crossbar_rtl")
        .expect("template exists")
        .render(minijinja::context! {
            module_name,
            slaves,
            channels,
            params,
            ports,
            axi_params,
            axi_ports,
        })
        .expect("render succeeds")
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

pub(crate) fn add_crossbar_slave_id_padding(
    state: &mut TopologyWithRtl,
    task_name: &str,
    args: &std::collections::BTreeMap<String, tapa_topology::instance::ArgDesign>,
    mmap_bindings: &children::ChildMmapBindings,
) {
    let mut assigns = Vec::new();
    for arg in args.values() {
        if !matches!(
            arg.cat,
            tapa_task_graph::port::ArgCategory::Mmap
                | tapa_task_graph::port::ArgCategory::AsyncMmap
        ) {
            continue;
        }
        let Some(&slave_idx) = mmap_bindings.slave_indices.get(&arg.arg) else {
            continue;
        };
        let Some(target_width) = mmap_bindings.wire_id_width(&arg.arg) else {
            continue;
        };
        let Some(child_width) = mmap_bindings.child_id_width(&arg.arg) else {
            continue;
        };
        if child_width >= target_width {
            continue;
        }
        let wire_prefix = crossbar_slave_prefix(&arg.arg, slave_idx);
        for suffix in ["_ARID", "_AWID"] {
            let wire_name = format!("{wire_prefix}{suffix}");
            assigns.push(ContinuousAssign::new(
                Expr::range(
                    Expr::ident(wire_name),
                    Expr::int(u64::from(target_width - 1)),
                    Expr::int(u64::from(child_width)),
                ),
                Expr::int_const(target_width - child_width, 0),
            ));
        }
    }
    if let Some(mm) = state.module_map.get_mut(task_name) {
        for assign in assigns {
            mm.add_assign(assign);
        }
    }
}

/// Add M-AXI ports, crossbar instances, and emit crossbar aux files.
pub(crate) fn add_m_axi_and_crossbars(
    state: &mut TopologyWithRtl,
    task_name: &str,
    mmap_conns: &std::collections::BTreeMap<String, crate::rtl_state::MMapConnection>,
) -> Result<(), CodegenError> {
    for conn in mmap_conns.values() {
        validate_mmap_connection(conn)?;
    }

    for conn in mmap_conns.values() {
        if let Some(mm) = state.module_map.get_mut(task_name) {
            if conn.channel_count() > 1 {
                for channel_idx in 0..conn.channel_count() {
                    add_m_axi_ports_with_id_width(
                        mm,
                        &format!("{}_{}", conn.arg_name, channel_idx),
                        conn.data_width,
                        64,
                        conn.id_width(),
                    );
                }
            } else if needs_crossbar(conn) || conn.id_width() > 1 {
                add_m_axi_ports_with_id_width(
                    mm,
                    &conn.arg_name,
                    conn.data_width,
                    64,
                    conn.id_width(),
                );
            } else {
                add_m_axi_ports(mm, &conn.arg_name, conn.data_width, 64);
            }
        }
        if needs_crossbar(conn) {
            // Declare downstream m_axi_{arg}_{idx}_* wires in parent
            // Size each wire using protocol metadata for correct widths
            if let Some(mm) = state.module_map.get_mut(task_name) {
                if conn.channel_count() > 1 {
                    let addr_width = try_get_addr_width(conn.chan_size, conn.data_width)?;
                    for channel_idx in 0..conn.channel_count() {
                        let channel_prefix = format!(
                            "m_axi_{}_{}",
                            tapa_rtl::module::sanitize_array_name(&conn.arg_name),
                            channel_idx
                        );
                        let offset_name = format!(
                            "{}_{}_offset",
                            tapa_rtl::module::sanitize_array_name(&conn.arg_name),
                            channel_idx
                        );
                        for suffix in ["_ARADDR", "_AWADDR"] {
                            let raw = crossbar_master_addr_raw(&conn.arg_name, channel_idx, suffix);
                            let _ = mm.add_signal(tapa_rtl::mutation::wide_wire(&raw, "63", "0"));
                            let local_addr = if addr_width >= 64 {
                                Expr::ident(&raw)
                            } else {
                                Expr::range(
                                    Expr::ident(&raw),
                                    Expr::int(u64::from(addr_width - 1)),
                                    Expr::int(0),
                                )
                            };
                            let rhs = Expr::plus(Expr::ident(&offset_name), local_addr);
                            mm.add_assign(ContinuousAssign::new(
                                Expr::ident(format!("{channel_prefix}{suffix}")),
                                rhs,
                            ));
                        }
                    }
                }

                for slave_idx in 0..conn.slaves.len() {
                    let wire_prefix = crossbar_slave_prefix(&conn.arg_name, slave_idx);
                    for suffix in tapa_protocol::M_AXI_SUFFIXES_COMPACT {
                        let wire_name = format!("{wire_prefix}{suffix}");
                        // Resolve width from suffix name using protocol constants
                        let width = crossbar_slave_suffix_width(conn, suffix);
                        let sig = if width > 1 {
                            tapa_rtl::mutation::wide_wire(&wire_name, &(width - 1).to_string(), "0")
                        } else {
                            tapa_rtl::mutation::wire(&wire_name)
                        };
                        let _ = mm.add_signal(sig);
                    }
                }
            }

            let crossbar_inst = try_build_crossbar_instance(conn)?;
            if let Some(mm) = state.module_map.get_mut(task_name) {
                mm.add_instance(crossbar_inst);
            }
            let crossbar_rtl = generate_crossbar_rtl(conn);
            let file_name = format!("{}.v", crossbar_module_name(conn));
            state.generated_files.insert(file_name, crossbar_rtl);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtl_state::MMapSlave;

    fn slave_w(task: &str, inst_idx: u32, port: &str, threads: u32, id_width: u32) -> MMapSlave {
        MMapSlave {
            task: task.into(),
            inst_idx,
            port: port.into(),
            threads,
            id_width,
        }
    }

    fn slave(task: &str, inst_idx: u32, port: &str, threads: u32) -> MMapSlave {
        MMapSlave {
            task: task.into(),
            inst_idx,
            port: port.into(),
            threads,
            id_width: 1,
        }
    }

    #[test]
    fn crossbar_needed_for_multiple_threads() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1), slave("task_b", 0, "data", 1)],
            chan_count: None,
            chan_size: None,
            data_width: 32,
        };
        assert!(needs_crossbar(&conn));
    }

    #[test]
    fn no_crossbar_for_single_thread() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1)],
            chan_count: None,
            chan_size: None,
            data_width: 32,
        };
        assert!(!needs_crossbar(&conn));
    }

    #[test]
    fn crossbar_needed_for_single_channel_hmap() {
        // chan_count = Some(1) is still an hmap and needs the crossbar
        // (a plain mmap has chan_count = None).
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1)],
            chan_count: Some(1),
            chan_size: Some(1024),
            data_width: 32,
        };
        assert!(needs_crossbar(&conn));
    }

    #[test]
    fn crossbar_params_use_nested_slave_threads() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave_w("leaf", 0, "d", 1, 2), slave("mid", 0, "data", 2)],
            chan_count: None,
            chan_size: None,
            data_width: 32,
        };
        let params = try_build_crossbar_params(&conn).expect("valid crossbar parameters");
        let rendered: Vec<String> = params.iter().map(|p| format!("{p}")).collect();
        assert!(
            rendered.iter().any(|p| p.contains("S00_THREADS(1)")),
            "got: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|p| p.contains("S01_THREADS(2)")),
            "got: {rendered:?}"
        );
    }

    #[test]
    fn crossbar_needed_for_hmap_channels() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1), slave("task_a", 1, "data", 1)],
            chan_count: Some(2),
            chan_size: Some(1024),
            data_width: 32,
        };
        assert!(needs_crossbar(&conn));
    }

    #[test]
    fn crossbar_params_structure() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1), slave("task_b", 0, "data", 1)],
            chan_count: None,
            chan_size: None,
            data_width: 64,
        };
        let params = try_build_crossbar_params(&conn).expect("valid crossbar parameters");
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
            slaves: vec![
                slave("task_a", 0, "data", 1),
                slave("task_b", 0, "data", 1),
                slave("task_c", 0, "data", 1),
            ],
            chan_count: None,
            chan_size: None,
            data_width: 64,
        };
        let text = try_build_crossbar_instance(&conn)
            .expect("valid crossbar instance")
            .to_string();
        assert!(text.contains(".S_ID_WIDTH(1)"), "got:\n{text}");
        assert!(text.contains(".M_ID_WIDTH(3)"), "got:\n{text}");
    }

    #[test]
    fn crossbar_module_name_format() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![
                slave_w("a", 0, "d", 1, 0),
                slave("b", 0, "d", 1),
                slave("c", 0, "d", 1),
            ],
            chan_count: Some(2),
            chan_size: None,
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
    fn addr_width_rejects_non_power_of_two_channel_bytes() {
        let err = try_get_addr_width(Some(3), 32).expect_err("12 bytes is not a power of two");
        assert!(err.to_string().contains("power of two"), "got: {err}");
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
            slaves: vec![
                slave_w("task_a", 0, "data", 1, 2),
                slave("task_b", 0, "data", 1),
            ],
            chan_count: None,
            chan_size: None,
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
            slaves: vec![slave("task_a", 0, "mem", 1)],
            chan_count: None,
            chan_size: None,
            data_width: 32,
        };
        let text = try_build_crossbar_instance(&conn)
            .expect("valid crossbar instance")
            .to_string();
        assert!(text.contains("axi_crossbar__chan_0"), "got:\n{text}");
        assert!(text.contains("m_axi_chan_0_ARADDR"), "got:\n{text}");
        assert!(!text.contains("chan[0]"), "got:\n{text}");
    }

    #[test]
    fn crossbar_instance_connects_multiple_parent_channels() {
        let conn = MMapConnection {
            arg_name: "mat_a".into(),
            slaves: vec![slave("task_a", 0, "mem", 1), slave("task_a", 1, "mem", 1)],
            chan_count: Some(2),
            chan_size: Some(1024),
            data_width: 512,
        };
        let text = try_build_crossbar_instance(&conn)
            .expect("valid crossbar instance")
            .to_string();
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
            slaves: vec![slave("task_a", 0, "data", 1)],
            chan_count: None,
            chan_size: None,
            data_width: 0,
        };
        validate_mmap_connection(&conn).unwrap_err();
    }

    #[test]
    fn validate_rejects_incomplete_hmap_shape() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1)],
            chan_count: Some(2),
            chan_size: None,
            data_width: 32,
        };
        let err = validate_mmap_connection(&conn).expect_err("chan_size is required");
        assert!(
            err.to_string().contains("both chan_count and chan_size"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_hmap_channels() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1)],
            chan_count: Some(0),
            chan_size: Some(1024),
            data_width: 32,
        };
        let err = validate_mmap_connection(&conn).expect_err("channel count must be nonzero");
        assert!(err.to_string().contains("channel count is 0"), "got: {err}");
    }

    #[test]
    fn validate_rejects_zero_slave_threads() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 0)],
            chan_count: None,
            chan_size: None,
            data_width: 32,
        };
        let err = validate_mmap_connection(&conn).expect_err("thread count must be nonzero");
        assert!(err.to_string().contains("thread count is 0"), "got: {err}");
    }

    #[test]
    fn validate_rejects_empty_crossbar_downstream() {
        // An hmap port with no downstream child connections.
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![],
            chan_count: Some(1),
            chan_size: None,
            data_width: 32,
        };
        validate_mmap_connection(&conn).unwrap_err();
    }

    #[test]
    fn crossbar_rtl_generation() {
        let conn = MMapConnection {
            arg_name: "mem".into(),
            slaves: vec![slave("task_a", 0, "data", 1), slave("task_b", 0, "data", 1)],
            chan_count: None,
            chan_size: None,
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
            slaves: vec![slave("task_a", 0, "data", 1), slave("task_b", 0, "data", 1)],
            chan_count: Some(2),
            chan_size: Some(1024),
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
