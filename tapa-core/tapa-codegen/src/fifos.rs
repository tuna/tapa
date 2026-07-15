//! FIFO instantiation and connection.

use crate::error::CodegenError;
use crate::rtl_state::TopologyWithRtl;
use tapa_protocol::{
    FIFO_READ_PORTS, FIFO_WRITE_PORTS, HANDSHAKE_CLK, HANDSHAKE_RST, ISTREAM_SUFFIXES,
    OSTREAM_SUFFIXES, STREAM_DATA_SUFFIXES, STREAM_PORT_DIRECTION,
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
                PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
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
        PortArg::new("clk", Expr::ident(HANDSHAKE_CLK)),
        PortArg::new("reset", Expr::ident(HANDSHAKE_RST)),
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

/// Producer endpoint for a FIFO in the parent task.
#[derive(Clone, Debug)]
struct FifoProducer {
    task_name: String,
    port_name: Option<String>,
}

/// FIFO entry: (name, depth, `is_consumed`, `producer_endpoint`).
type FifoEntry = (String, Option<u32>, bool, Option<FifoProducer>);
/// FIFO connection entry: (name, depth, `has_consumer`, `has_producer`, `producer_endpoint`).
type FifoConnEntry = (String, Option<u32>, bool, bool, Option<FifoProducer>);

/// Instantiate FIFOs for a task.
///
/// Internal FIFOs (with depth) get a `fifo` module instance.
/// External FIFOs (no depth) get wire assignments connecting to external ports.
/// FIFO width is resolved from the producer child's attached RTL module ports.
pub(crate) fn instantiate_fifos(
    state: &mut TopologyWithRtl,
    task_name: &str,
) -> Result<(), CodegenError> {
    let task = &state.design.tasks[task_name];

    // Collect FIFO info before mutating
    let fifo_entries: Vec<FifoEntry> = task
        .fifos
        .iter()
        .map(|(name, fifo)| {
            let is_consumed = fifo.consumed_by.is_some();
            let producer = fifo_producer_for(task, name, fifo.produced_by.as_ref());
            (name.clone(), fifo.depth, is_consumed, producer)
        })
        .collect();

    for (fifo_name, depth, is_consumed, producer) in fifo_entries {
        if let Some(depth) = depth {
            // Buffered internal FIFOs must have a producer to size the buffer.
            let producer = producer
                .as_ref()
                .ok_or_else(|| CodegenError::FifoWidthUnresolved(fifo_name.clone()))?;
            let width = resolve_fifo_width(state, producer, &fifo_name)?;
            let fifo_inst = build_fifo_instance(
                &fifo_name,
                Expr::ident(HANDSHAKE_RST),
                Expr::int(u64::from(width)),
                depth,
            );
            if let Some(mm) = state.module_map.get_mut(task_name) {
                mm.add_instance(fifo_inst);
            }
        } else {
            // External FIFO: wire assigns if internal/external names differ
            let assigns = build_external_fifo_assigns(&fifo_name, &fifo_name, is_consumed);
            if let Some(mm) = state.module_map.get_mut(task_name) {
                for assign in assigns {
                    mm.add_assign(assign);
                }
            }
        }
    }
    Ok(())
}

#[allow(
    clippy::single_option_map,
    reason = "keeping the Option in the signature lets both callers avoid inline duplication"
)]
fn fifo_producer_for(
    task: &tapa_ir::Task,
    fifo_name: &str,
    endpoint: Option<&tapa_ir::interconnect::EndpointRef>,
) -> Option<FifoProducer> {
    endpoint.map(|ep| {
        let port_name = task
            .tasks
            .get(&ep.0)
            .and_then(|instances| instances.get(ep.1 as usize))
            .and_then(|instance| {
                instance
                    .args
                    .iter()
                    .find(|(_, arg)| arg.arg == fifo_name)
                    .map(|(port_name, _)| port_name.clone())
            });
        FifoProducer {
            task_name: ep.0.clone(),
            port_name,
        }
    })
}

/// Resolve FIFO width from the producer child's attached RTL module.
///
/// Looks for the producer's bound stream port on the parsed child RTL and uses
/// its width, falling back to the producer task's topology port width. Errors
/// when neither yields a width, which for a well-formed program only happens
/// if the producer task, its instance, or its bound stream port is missing.
fn resolve_fifo_width(
    state: &TopologyWithRtl,
    producer: &FifoProducer,
    fifo_name: &str,
) -> Result<u32, CodegenError> {
    // Check attached RTL module for producer port width
    if let Some(mm) = state.module_map.get(producer.task_name.as_str()) {
        if let Some(port_name) = producer.port_name.as_deref() {
            for suffix in ["_din", "_dout"] {
                if let Some(port) = mm.inner.get_port_of(port_name, suffix) {
                    if let Some(width) = port.bit_width() {
                        return Ok(width);
                    }
                }
            }
        } else {
            // Incomplete external topology may not identify the logical port;
            // use the first matching stream signal as a best-effort width.
            for port in &mm.inner.ports {
                if port.name.ends_with("_dout") || port.name.ends_with("_din") {
                    if let Some(width) = port.bit_width() {
                        return Ok(width);
                    }
                }
            }
        }
    }
    // Fallback: check topology port definitions for the producer task
    if let Some(task) = state.design.tasks.get(producer.task_name.as_str()) {
        if let Some(port_name) = producer.port_name.as_deref() {
            let logical_port_name =
                tapa_rtl::module::match_array_name(port_name).map_or(port_name, |(base, _)| base);
            if let Some(port) = task.ports.iter().find(|port| {
                port.name == logical_port_name
                    && matches!(
                        port.cat,
                        tapa_ir::port::ArgCategory::Ostream | tapa_ir::port::ArgCategory::Ostreams
                    )
            }) {
                return Ok(port.width.saturating_add(1));
            }
        }
        for port in &task.ports {
            if matches!(
                port.cat,
                tapa_ir::port::ArgCategory::Ostream | tapa_ir::port::ArgCategory::Ostreams
            ) {
                return Ok(port.width.saturating_add(1));
            }
        }
    }
    Err(CodegenError::FifoWidthUnresolved(fifo_name.to_owned()))
}

/// Connect FIFOs: declare inter-task wires and connect external FIFOs.
///
/// For internal FIFOs (both endpoints in this task): declare wires with
/// proper width using stream suffixes so child instances can connect.
/// For external FIFOs: connect to parent module ports, potentially
/// through AXIS adapters.
#[allow(
    clippy::too_many_lines,
    reason = "FIFO connection orchestration is inherently sequential; \
              splitting would fragment the wiring logic"
)]
pub(crate) fn connect_fifos(
    state: &mut TopologyWithRtl,
    task_name: &str,
) -> Result<(), CodegenError> {
    use tapa_rtl::signal::{Signal, SignalKind};

    let task = &state.design.tasks[task_name];

    // Collect FIFO connection info with producer endpoint for width resolution
    let fifo_entries: Vec<FifoConnEntry> = task
        .fifos
        .iter()
        .map(|(name, fifo)| {
            let has_consumer = fifo.consumed_by.is_some();
            let has_producer = fifo.produced_by.is_some();
            let producer = fifo_producer_for(task, name, fifo.produced_by.as_ref());
            (
                name.clone(),
                fifo.depth,
                has_consumer,
                has_producer,
                producer,
            )
        })
        .collect();

    for (fifo_name, depth, has_consumer, has_producer, producer) in &fifo_entries {
        let sanitized_fifo_name = tapa_rtl::module::sanitize_array_name(fifo_name);

        if depth.is_some() && *has_consumer && *has_producer {
            // Internal FIFO: declare wires for both read and write sides.
            // A produced+consumed internal FIFO always has a producer endpoint.
            let producer = producer
                .as_ref()
                .ok_or_else(|| CodegenError::FifoWidthUnresolved(fifo_name.clone()))?;
            let width = resolve_fifo_width(state, producer, fifo_name)?;
            if let Some(mm) = state.module_map.get_mut(task_name) {
                // Declare wires for both read and write sides; the data
                // suffixes (`_dout`/`_din`) carry the FIFO width.
                for suffix in ISTREAM_SUFFIXES.iter().chain(OSTREAM_SUFFIXES) {
                    let wire_name = format!("{sanitized_fifo_name}{suffix}");
                    let sig = if STREAM_DATA_SUFFIXES.contains(suffix) {
                        tapa_rtl::mutation::wide_wire(&wire_name, &(width - 1).to_string(), "0")
                    } else {
                        tapa_rtl::mutation::wire(&wire_name)
                    };
                    let _ = mm.add_signal(sig);
                }
            }
        } else if depth.is_none() {
            // External FIFO: parent module ports exist, just need to ensure
            // wires exist for child instance connections. The stream port with
            // this FIFO's name carries the payload width; a plural stream FIFO
            // like `data[0]` maps to the base port `data`.
            let logical_fifo_name = tapa_rtl::module::match_array_name(fifo_name)
                .map_or(fifo_name.as_str(), |(base, _)| base);
            let stream_width = task
                .ports
                .iter()
                .find(|p| p.name == *fifo_name || p.name == logical_fifo_name)
                .ok_or_else(|| CodegenError::FifoWidthUnresolved(fifo_name.clone()))?
                .width;
            let is_vitis_top_axis =
                task_name == state.design.top && state.design.target == "xilinx-vitis";
            if let Some(mm) = state.module_map.get_mut(task_name) {
                let suffixes: &[&str] = if *has_consumer {
                    ISTREAM_SUFFIXES
                } else {
                    OSTREAM_SUFFIXES
                };
                let canonical_base = tapa_rtl::module::sanitize_array_name(fifo_name);
                for suffix in suffixes {
                    let canonical = format!("{canonical_base}{suffix}");
                    let signal_width = if suffix.ends_with("dout") || suffix.ends_with("din") {
                        Some(tapa_rtl::port::Width {
                            msb: tapa_rtl::expression::tokenize_expression(
                                &stream_width.to_string(),
                            ),
                            lsb: tapa_rtl::expression::tokenize_expression("0"),
                        })
                    } else {
                        None
                    };
                    let _ = mm.add_signal(Signal {
                        name: canonical.clone(),
                        kind: SignalKind::Wire,
                        width: signal_width,
                    });
                    let Some(parent_port) = mm.inner.get_port_of(fifo_name, suffix).cloned() else {
                        continue;
                    };
                    if parent_port.name == canonical {
                        continue;
                    }
                    let parent = Expr::ident(parent_port.name);
                    let internal = Expr::ident(canonical);
                    let is_input_dir = STREAM_PORT_DIRECTION
                        .get(suffix)
                        .is_some_and(|&d| d == "input");
                    if is_input_dir {
                        mm.add_assign(ContinuousAssign::new(internal, parent));
                    } else {
                        mm.add_assign(ContinuousAssign::new(parent, internal));
                    }
                }
            }

            // Check if this should be an AXIS adapter
            if is_vitis_top_axis {
                // Instantiate AXIS adapter
                let is_input = *has_consumer;
                let adapter = build_axis_adapter(fifo_name, stream_width, is_input);
                if let Some(mm) = state.module_map.get_mut(task_name) {
                    mm.add_instance(adapter);
                    if !is_input {
                        let canonical_base = tapa_rtl::module::sanitize_array_name(fifo_name);
                        if mm
                            .inner
                            .find_port(&format!("{canonical_base}_TKEEP"))
                            .is_some()
                        {
                            let keep_width = stream_width.div_ceil(8).max(1);
                            mm.add_assign(ContinuousAssign::new(
                                Expr::ident(format!("{canonical_base}_TKEEP")),
                                Expr::lit(format!(
                                    "{keep_width}'b{}",
                                    "1".repeat(keep_width as usize)
                                )),
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
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

    #[test]
    fn fifo_width_topology_fallback_uses_bound_wire_width() {
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "consumer": [{"args": {
                            "narrow_in": {"arg": "narrow_fifo", "cat": "istream"},
                            "wide_in[0]": {"arg": "wide_fifo", "cat": "istream"}
                        }}],
                        "producer": [{"args": {
                            "narrow": {"arg": "narrow_fifo", "cat": "ostream"},
                            "wide[0]": {"arg": "wide_fifo", "cat": "ostream"}
                        }}]
                    },
                    "fifos": {
                        "narrow_fifo": {
                            "depth": 2,
                            "produced_by": ["producer", 0],
                            "consumed_by": ["consumer", 0]
                        },
                        "wide_fifo": {
                            "depth": 2,
                            "produced_by": ["producer", 0],
                            "consumed_by": ["consumer", 0]
                        }
                    }
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "narrow_in", "type": "uint8_t", "width": 8},
                        {"cat": "istreams", "name": "wide_in", "type": "uint32_t", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "producer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "narrow", "type": "uint8_t", "width": 8},
                        {"cat": "ostreams", "name": "wide", "type": "uint32_t", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let mut state = TopologyWithRtl::new(program);

        let top = &state.design.tasks["top"];
        let narrow = fifo_producer_for(
            top,
            "narrow_fifo",
            top.fifos["narrow_fifo"].produced_by.as_ref(),
        )
        .unwrap();
        let wide = fifo_producer_for(
            top,
            "wide_fifo",
            top.fifos["wide_fifo"].produced_by.as_ref(),
        )
        .unwrap();
        assert_eq!(
            resolve_fifo_width(&state, &narrow, "narrow_fifo").unwrap(),
            9
        );
        assert_eq!(resolve_fifo_width(&state, &wide, "wide_fifo").unwrap(), 33);

        state
            .attach_module(
                "producer",
                tapa_rtl::VerilogModule::parse(
                    "module producer #(parameter WIDTH = 32) \
                     (output wire [WIDTH:0] wide_0_s_din); endmodule",
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(
            resolve_fifo_width(&state, &wide, "wide_fifo").unwrap(),
            33,
            "symbolic RTL width must fall back to the bound topology port"
        );
    }

    #[test]
    fn fifo_width_unresolved_is_an_error() {
        // The producer task exposes no stream port and has no attached RTL, so
        // neither resolution path yields a width. This must fail rather than
        // silently default to 32.
        let program = crate::design_from_fixture_json(serde_json::json!({
            "top": "top",
            "target": "xilinx-hls",
            "tasks": {
                "top": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "producer": [{"args": {"mem": {"arg": "orphan_fifo", "cat": "mmap"}}}]
                    },
                    "fifos": {
                        "orphan_fifo": {
                            "depth": 2,
                            "produced_by": ["producer", 0],
                            "consumed_by": ["consumer", 0]
                        }
                    }
                },
                "producer": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [{"cat": "mmap", "name": "mem", "type": "float*", "width": 32}],
                    "tasks": {}, "fifos": {}
                }
            }
        }));
        let state = TopologyWithRtl::new(program);
        let producer = FifoProducer {
            task_name: "producer".to_owned(),
            port_name: Some("mem".to_owned()),
        };
        let err = resolve_fifo_width(&state, &producer, "orphan_fifo")
            .expect_err("mmap-only producer has no stream width");
        assert!(
            matches!(err, CodegenError::FifoWidthUnresolved(ref f) if f == "orphan_fifo"),
            "expected FifoWidthUnresolved(orphan_fifo), got {err:?}"
        );
    }
}
