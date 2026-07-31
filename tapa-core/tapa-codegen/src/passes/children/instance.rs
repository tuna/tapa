//! Child `ModuleInstance` assembly: handshake, scalar, stream, and mmap
//! port-arg wiring for one child task instance.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::port::ArgCategory;
use tapa_ir::Arg;
use tapa_protocol::{
    stream_peek_port_name, stream_port_name, HANDSHAKE_CLK, HANDSHAKE_RST_N, ISTREAM_SUFFIXES,
    M_AXI_PREFIX, OSTREAM_SUFFIXES,
};
use tapa_rtl::builder::{Expr, ModuleInstance, PortArg};
use tapa_rtl::module::sanitize_array_name;
use tapa_rtl::VerilogModule;

use crate::instance_signals::InstanceSignals;
use crate::async_mmap;

/// Build a child task `ModuleInstance` with all port argument bindings.
///
/// Connects handshake signals (from `InstanceSignals`), scalar arguments,
/// stream arguments (istream/ostream suffixes), and mmap offset arguments.
///
/// Mmap bindings describe how each child mmap argument reaches parent AXI wires.
#[derive(Debug, Default)]
pub struct ChildMmapBindings {
    pub slave_indices: BTreeMap<String, usize>,
    pub wire_id_widths: BTreeMap<String, u32>,
    pub child_id_widths: BTreeMap<String, u32>,
    pub direct_wire_prefixes: BTreeMap<String, String>,
}

impl ChildMmapBindings {
    pub fn upstream_wire_prefix(&self, arg_name: &str) -> String {
        mmap_wire_prefix(arg_name, &self.slave_indices)
    }

    pub fn wire_prefix(&self, arg_name: &str) -> String {
        self.direct_wire_prefixes
            .get(arg_name)
            .cloned()
            .unwrap_or_else(|| self.upstream_wire_prefix(arg_name))
    }

    pub fn wire_id_width(&self, arg_name: &str) -> Option<u32> {
        self.wire_id_widths.get(arg_name).copied()
    }

    pub fn child_id_width(&self, arg_name: &str) -> Option<u32> {
        self.child_id_widths.get(arg_name).copied()
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "child instance assembly needs the parent and child module \
              headers alongside the signal/binding contexts"
)]
#[allow(
    clippy::too_many_lines,
    reason = "child instance assembly is inherently sequential; \
              splitting would fragment the port-arg wiring logic"
)]
pub fn build_child_instance(
    child_task_name: &str,
    instance_name: &str,
    sig: &InstanceSignals,
    args: &BTreeMap<String, Arg>,
    mmap_bindings: &ChildMmapBindings,
    parent_fifos: &BTreeSet<String>,
    parent_rtl: Option<&VerilogModule>,
    child_rtl: Option<&VerilogModule>,
) -> ModuleInstance {
    build_child_instance_with_reset(
        child_task_name,
        instance_name,
        sig,
        args,
        mmap_bindings,
        parent_fifos,
        parent_rtl,
        child_rtl,
        Expr::ident(HANDSHAKE_RST_N),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "child instance assembly needs an explicit local reset on the distributed path"
)]
#[allow(
    clippy::too_many_lines,
    reason = "this is the same sequential port wiring as the public constructor"
)]
pub(super) fn build_child_instance_with_reset(
    child_task_name: &str,
    instance_name: &str,
    sig: &InstanceSignals,
    args: &BTreeMap<String, Arg>,
    mmap_bindings: &ChildMmapBindings,
    parent_fifos: &BTreeSet<String>,
    parent_rtl: Option<&VerilogModule>,
    child_rtl: Option<&VerilogModule>,
    reset_n: Expr,
) -> ModuleInstance {
    let mut port_args = Vec::new();

    // Clock and reset
    port_args.push(PortArg::new(HANDSHAKE_CLK, Expr::ident(HANDSHAKE_CLK)));
    port_args.push(PortArg::new(HANDSHAKE_RST_N, reset_n));

    // Handshake signals from InstanceSignals
    port_args.extend(sig.instance_portargs());

    let resolve_child_stream_port = |name: &str, suffix: &str| {
        child_rtl
            .and_then(|module| module.get_port_of(name, suffix))
            .map_or_else(
                || stream_port_name(&sanitize_array_name(name), suffix),
                |p| p.name.clone(),
            )
    };
    let resolve_child_stream_peek_port =
        |name: &str, suffix: &str| resolve_peek_port_name(child_rtl?, name, suffix);
    // A stream argument binds to either a FIFO declared in the parent task
    // (signal wires are named `{fifo}{suffix}`, e.g. `a_q_dout`) or, for a
    // passthrough, the parent's own stream port. The parent port name is
    // resolved against the parent's RTL module: Vitis HLS spells it
    // `{port}_s{suffix}` for scalar streams but `{port}{suffix}` for array
    // elements (and `{base}_peek_{idx}{suffix}` for array peeks), so a fixed
    // `_s`/`_peek` infix does not fit all cases. The `{port}_s{suffix}` /
    // `{port}_peek{suffix}` spellings apply when the parent module is
    // unavailable. The leaf module port itself is resolved separately by
    // `resolve_child_stream_port`.
    let stream_signal = |name: &str, suffix: &str| {
        let base = sanitize_array_name(name);
        if parent_fifos.contains(name) {
            return format!("{base}{suffix}");
        }
        parent_rtl
            .and_then(|module| module.get_port_of(name, suffix))
            .map_or_else(|| stream_port_name(&base, suffix), |p| p.name.clone())
    };
    let peek_signal = |name: &str, suffix: &str| {
        let base = sanitize_array_name(name);
        if parent_fifos.contains(name) {
            return format!("{base}{suffix}");
        }
        parent_rtl
            .and_then(|module| resolve_peek_port_name(module, name, suffix))
            .unwrap_or_else(|| stream_peek_port_name(&base, suffix))
    };

    // Argument port bindings
    for (child_port, arg) in args {
        match arg.cat {
            ArgCategory::Scalar => {
                // Scalar: connect to per-instance pipeline signal
                let pipeline_name = format!("{instance_name}__{child_port}");
                port_args.push(PortArg::new(
                    child_port.as_str(),
                    Expr::ident(pipeline_name),
                ));
            }
            ArgCategory::Istream | ArgCategory::Istreams => {
                // Input stream: connect with ISTREAM_SUFFIXES
                for suffix in ISTREAM_SUFFIXES {
                    let signal = Expr::ident(stream_signal(&arg.arg, suffix));
                    port_args.push(PortArg::new(
                        resolve_child_stream_port(child_port, suffix),
                        signal.clone(),
                    ));
                    if matches!(*suffix, "_dout" | "_empty_n") {
                        if let Some(peek_port) = resolve_child_stream_peek_port(child_port, suffix)
                        {
                            port_args.push(PortArg::new(
                                peek_port,
                                Expr::ident(peek_signal(&arg.arg, suffix)),
                            ));
                        }
                    }
                }
            }
            ArgCategory::Ostream | ArgCategory::Ostreams => {
                // Output stream: connect with OSTREAM_SUFFIXES
                for suffix in OSTREAM_SUFFIXES {
                    port_args.push(PortArg::new(
                        resolve_child_stream_port(child_port, suffix),
                        Expr::ident(stream_signal(&arg.arg, suffix)),
                    ));
                }
            }
            ArgCategory::Mmap | ArgCategory::AsyncMmap => {
                // Connect to per-instance pipeline offset
                let offset_sig = format!("{instance_name}__{child_port}_offset");
                // Bind M-AXI channel ports:
                // If crossbar exists (slave index present), bind to downstream wires
                // Otherwise bind directly to upstream parent m_axi signals
                let m_axi_wire_prefix = mmap_bindings.wire_prefix(&arg.arg);
                let m_axi_wire_id_width = mmap_bindings.wire_id_width(&arg.arg);
                let child_m_axi_id_width = mmap_bindings.child_id_width(&arg.arg);
                if matches!(arg.cat, ArgCategory::AsyncMmap) {
                    if child_has_direct_mmap_ports(child_rtl, child_port) {
                        let child_rtl_filter =
                            if child_has_direct_mmap_offset(child_rtl, child_port) {
                                None
                            } else {
                                child_rtl
                            };
                        add_direct_mmap_portargs(
                            &mut port_args,
                            child_port,
                            &offset_sig,
                            &m_axi_wire_prefix,
                            m_axi_wire_id_width,
                            child_m_axi_id_width,
                            child_rtl_filter,
                            child_rtl,
                        );
                    } else if let Some(module) = child_rtl {
                        let bridge_base =
                            crate::async_mmap::bridge_base_from_m_axi_prefix(&m_axi_wire_prefix);
                        port_args.extend(crate::async_mmap::child_portargs(
                            module,
                            child_port,
                            &bridge_base,
                            &offset_sig,
                        ));
                    }
                } else {
                    add_direct_mmap_portargs(
                        &mut port_args,
                        child_port,
                        &offset_sig,
                        &m_axi_wire_prefix,
                        m_axi_wire_id_width,
                        child_m_axi_id_width,
                        None,
                        child_rtl,
                    );
                }
            }
            ArgCategory::Immap | ArgCategory::Ommap => {
                // Other categories: direct connection
                port_args.push(PortArg::new(
                    sanitize_array_name(child_port),
                    Expr::ident(sanitize_array_name(&arg.arg)),
                ));
            }
        }
    }

    ModuleInstance::new(child_task_name, instance_name).with_ports(port_args)
}

fn child_has_direct_mmap_offset(child_rtl: Option<&VerilogModule>, child_port: &str) -> bool {
    child_rtl.is_some_and(|module| module.find_port(&format!("{child_port}_offset")).is_some())
}

fn child_has_direct_mmap_ports(child_rtl: Option<&VerilogModule>, child_port: &str) -> bool {
    child_rtl.is_some_and(|module| async_mmap::has_direct_m_axi_ports(module, child_port))
}

#[allow(
    clippy::too_many_arguments,
    reason = "mmap port arg wiring needs all 8 parameters"
)]
fn add_direct_mmap_portargs(
    port_args: &mut Vec<PortArg>,
    child_port: &str,
    offset_sig: &str,
    m_axi_wire_prefix: &str,
    m_axi_wire_id_width: Option<u32>,
    child_m_axi_id_width: Option<u32>,
    child_rtl_filter: Option<&VerilogModule>,
    child_rtl_for_width: Option<&VerilogModule>,
) {
    let offset_port = format!("{child_port}_offset");
    if child_rtl_filter.is_none_or(|module| module.find_port(&offset_port).is_some()) {
        port_args.push(PortArg::new(offset_port, Expr::ident(offset_sig)));
    }
    for suffix in tapa_protocol::M_AXI_SUFFIXES_COMPACT {
        let child_axi_port = format!("m_axi_{child_port}{suffix}");
        if child_rtl_filter.is_none_or(|module| module.find_port(&child_axi_port).is_some()) {
            let wire_name = format!("{m_axi_wire_prefix}{suffix}");
            port_args.push(PortArg::new(
                child_axi_port.as_str(),
                direct_mmap_connection_expr(
                    child_rtl_for_width,
                    &child_axi_port,
                    &wire_name,
                    suffix,
                    m_axi_wire_id_width,
                    child_m_axi_id_width,
                ),
            ));
        }
    }
}

fn direct_mmap_connection_expr(
    child_rtl: Option<&VerilogModule>,
    child_axi_port: &str,
    wire_name: &str,
    suffix: &str,
    m_axi_wire_id_width: Option<u32>,
    child_m_axi_id_width: Option<u32>,
) -> Expr {
    let is_id = matches!(suffix, "_ARID" | "_AWID" | "_BID" | "_RID");
    let Some(target_width) = m_axi_wire_id_width else {
        return Expr::ident(wire_name);
    };
    if !is_id || target_width <= 1 {
        return Expr::ident(wire_name);
    }
    let child_width = child_m_axi_id_width
        .or_else(|| {
            child_rtl
                .and_then(|module| module.find_port(child_axi_port))
                .and_then(tapa_rtl::port::Port::bit_width)
        })
        .unwrap_or(target_width);
    if child_width >= target_width {
        return Expr::ident(wire_name);
    }
    Expr::range(
        Expr::ident(wire_name),
        Expr::int(u64::from(child_width - 1)),
        Expr::int(0),
    )
}

pub fn mmap_wire_prefix(
    arg_name: &str,
    mmap_slave_indices: &std::collections::BTreeMap<String, usize>,
) -> String {
    let sanitized_arg = sanitize_array_name(arg_name);
    if let Some(slave_idx) = mmap_slave_indices.get(arg_name) {
        crate::m_axi::crossbar_slave_prefix(&sanitized_arg, *slave_idx)
    } else {
        format!("{M_AXI_PREFIX}{sanitized_arg}")
    }
}

fn array_name_parts(name: &str) -> Option<(&str, &str, &str)> {
    let left = name.find('[')?;
    let right = name[left + 1..].find(']')? + left + 1;
    Some((&name[..left], &name[left + 1..right], &name[right + 1..]))
}

/// Resolve the peek port belonging to stream `name` on `module`. Array
/// elements (`in_q[0]`) map to `{base}_peek_{idx}{tail}{suffix}`; scalar
/// streams go through the usual `{name}{infix}_peek{suffix}` infix search.
fn resolve_peek_port_name(module: &VerilogModule, name: &str, suffix: &str) -> Option<String> {
    if let Some((base, idx, tail)) = array_name_parts(name) {
        let candidate = format!("{base}_peek_{idx}{tail}{suffix}");
        return module.find_port(&candidate).map(|p| p.name.clone());
    }
    module
        .get_port_of(name, &format!("_peek{suffix}"))
        .map(|p| p.name.clone())
}

#[cfg(test)]
mod tests;
