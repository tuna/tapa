//! `ABGraph` generation from topology.
//!
//! Builds the placement graph from a TAPA program's task/FIFO topology.

use std::collections::BTreeMap;

use regex::Regex;
use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_task_graph::port::ArgCategory;
use tapa_topology::program::Program;

use crate::abgraph::{ABEdge, ABGraph, ABVertex};
use crate::area::Area;
use crate::FloorplanError;

/// Prefix for port dummy vertices.
const TAPA_PORT_PREFIX: &str = "__tapa_port_";

/// Default MMAP width: `(write_width, read_width)`.
pub const MMAP_WIDTH: (u64, u64) = (405, 43);

/// Collect task areas from program annotations.
///
/// Returns a map from task name to resource area dict.
#[must_use]
pub fn collect_task_area(program: &Program) -> BTreeMap<String, Area> {
    let top = &program.tasks[&program.top];
    let mut areas = BTreeMap::new();
    for task_name in top.tasks.keys() {
        if let Some(task) = program.tasks.get(task_name) {
            let area = task
                .annotations
                .get("total_area")
                .and_then(|v| {
                    let map = v.as_object()?;
                    Area::from_resource_map(map).ok()
                })
                .unwrap_or_default();
            areas.insert(task_name.clone(), area);
        }
    }
    areas
}

/// Collect FIFO widths from topology port definitions.
///
/// For each FIFO in the top task, uses the port width from the topology.
#[must_use]
pub fn collect_fifo_width(program: &Program) -> BTreeMap<String, u64> {
    let top = &program.tasks[&program.top];
    let mut widths = BTreeMap::new();
    // For each FIFO, find the producer's port width from the child task definition
    for (fifo_name, fifo) in &top.fifos {
        if let Some(ref producer) = fifo.produced_by {
            let producer_task_name = &producer.0;
            if let Some(task) = program.tasks.get(producer_task_name) {
                // Find the port on the producer task that connects to this FIFO
                for port in &task.ports {
                    if port.cat == ArgCategory::Ostream || port.cat == ArgCategory::Ostreams {
                        // Check if any instance of this task maps this port to our FIFO
                        if let Some(instances) = top.tasks.get(producer_task_name) {
                            if let Some(inst) = instances.get(producer.1 as usize) {
                                for (port_name, arg) in &inst.args {
                                    if arg.arg == *fifo_name && *port_name == port.name {
                                        widths.insert(fifo_name.clone(), u64::from(port.width));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // Fallback: use a default width if not found
        widths.entry(fifo_name.clone()).or_insert(32);
    }
    widths
}

/// Collect port widths for top-level ports.
///
/// Returns a map from port name to width. MMAP ports get `(write, read)` pair.
/// Plural streams (`Istreams`/`Ostreams`) multiply the base width by the number
/// of connected FIFOs.
#[must_use]
pub fn collect_port_width(program: &Program) -> BTreeMap<String, PortWidth> {
    let top = &program.tasks[&program.top];
    let mut widths = BTreeMap::new();
    for port in &top.ports {
        let w = match port.cat {
            ArgCategory::Mmap | ArgCategory::AsyncMmap | ArgCategory::Immap | ArgCategory::Ommap => {
                PortWidth::Mmap(MMAP_WIDTH.0, MMAP_WIDTH.1)
            }
            ArgCategory::Istreams | ArgCategory::Ostreams => {
                // Plural streams: width * number of connected FIFOs
                let fifo_count = count_port_fifos(top, &port.name);
                PortWidth::Simple(u64::from(port.width) * u64::from(fifo_count.max(1)))
            }
            ArgCategory::Istream
            | ArgCategory::Ostream
            | ArgCategory::Scalar => PortWidth::Simple(u64::from(port.width)),
        };
        widths.insert(port.name.clone(), w);
    }
    widths
}

/// Count how many FIFOs are connected to a given port name.
fn count_port_fifos(top: &tapa_topology::task::TaskDesign, port_name: &str) -> u32 {
    let mut count = 0u32;
    for instances in top.tasks.values() {
        for inst in instances {
            for arg in inst.args.values() {
                if arg.arg == port_name {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Top-level `ABGraph` generation entrypoint.
///
/// Collects areas, FIFO widths, and port widths from the program, then
/// builds the full placement graph with FIFO edges, port dummy vertices,
/// and scalar FSM connections.
pub fn get_top_level_ab_graph(
    program: &Program,
    preassignments: &BTreeMap<String, String>,
    fsm_name: &str,
) -> Result<ABGraph, FloorplanError> {
    let areas = collect_task_area(program);
    let fifo_widths = collect_fifo_width(program);
    let port_widths = collect_port_width(program);
    let mut graph = get_basic_ab_graph(program, &areas, &fifo_widths);
    add_port_iface_connections(program, &mut graph, &port_widths, preassignments)?;
    add_scalar_connections(program, &mut graph, &port_widths, fsm_name);
    Ok(graph)
}

/// Top-level `ABGraph` generation from RTL-bearing state.
///
/// Like `get_top_level_ab_graph`, but derives FIFO widths from attached
/// RTL module ports instead of topology-only metadata.
pub fn get_top_level_ab_graph_from_rtl(
    state: &TopologyWithRtl,
    preassignments: &BTreeMap<String, String>,
    fsm_name: &str,
) -> Result<ABGraph, FloorplanError> {
    let program = &state.program;
    let areas = collect_task_area(program);
    let fifo_widths = collect_fifo_width_from_rtl(state);
    let port_widths = collect_port_width(program);
    let mut graph = get_basic_ab_graph(program, &areas, &fifo_widths);
    add_port_iface_connections(program, &mut graph, &port_widths, preassignments)?;
    add_scalar_connections(program, &mut graph, &port_widths, fsm_name);
    Ok(graph)
}

/// Collect FIFO widths from attached RTL modules.
///
/// For each FIFO, finds the producer task's RTL module and extracts the
/// port width from the parsed Verilog, matching Python's `get_fifo_width`.
#[must_use]
pub fn collect_fifo_width_from_rtl(state: &TopologyWithRtl) -> BTreeMap<String, u64> {
    let program = &state.program;
    let top = &program.tasks[&program.top];
    let mut widths = BTreeMap::new();

    for (fifo_name, fifo) in &top.fifos {
        if let Some(ref producer) = fifo.produced_by {
            let producer_task_name = &producer.0;
            // Try to get width from RTL module's parsed port widths.
            //
            // Mirrors Python `tapa.verilog.xilinx.module_ops.ports.get_port_of`,
            // which probes the producer's ports through the same
            // `("_V", "_r", "_s", "")` infix set. The slot-wrapper Verilog
            // exposes stream ports as `{port}_s_din` / `{port}_s_dout` (33
            // bits for a 32-bit payload + "eot" flag); without probing the
            // `_s` infix, Rust falls back to the topology width and emits
            // a FIFO edge one bit too narrow.
            if let Some(mm) = state.module_map.get(producer_task_name) {
                if let Some(instances) = top.tasks.get(producer_task_name) {
                    if let Some(inst) = instances.get(producer.1 as usize) {
                        for (port_name, arg) in &inst.args {
                            if arg.arg == *fifo_name {
                                if let Some(w) = find_fifo_port_width(&mm.inner, port_name) {
                                    widths.insert(fifo_name.clone(), u64::from(w));
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        // Fallback to topology-based width
        if !widths.contains_key(fifo_name) {
            let topo_widths = collect_fifo_width(program);
            if let Some(&w) = topo_widths.get(fifo_name) {
                widths.insert(fifo_name.clone(), w);
            } else {
                widths.insert(fifo_name.clone(), 32);
            }
        }
    }
    widths
}

/// Port width: either a simple scalar width or MMAP (write, read) pair.
#[derive(Debug, Clone, Copy)]
pub enum PortWidth {
    Simple(u64),
    Mmap(u64, u64),
}

impl PortWidth {
    fn write_width(self) -> u64 {
        match self {
            Self::Simple(w) | Self::Mmap(w, _) => w,
        }
    }

    fn read_width(self) -> u64 {
        match self {
            Self::Simple(w) | Self::Mmap(_, w) => w,
        }
    }
}

/// Build the basic `ABGraph` with vertices for task instances and edges for FIFOs.
#[must_use]
pub fn get_basic_ab_graph(
    program: &Program,
    areas: &BTreeMap<String, Area>,
    fifo_widths: &BTreeMap<String, u64>,
) -> ABGraph {
    let top = &program.tasks[&program.top];
    let mut vertices = BTreeMap::new();
    let mut edges = Vec::new();

    // Create one vertex per task instance
    for (task_name, instances) in &top.tasks {
        let area = areas.get(task_name).copied().unwrap_or_default();
        for (idx, _inst) in instances.iter().enumerate() {
            let inst_name = format!("{task_name}_{idx}");
            vertices.insert(
                inst_name.clone(),
                ABVertex {
                    name: inst_name.clone(),
                    sub_cells: vec![inst_name],
                    area,
                    target_slot: None,
                    reserved_slot: None,
                },
            );
        }
    }

    // Create edges for FIFOs
    for (fifo_name, fifo) in &top.fifos {
        let Some(ref consumer) = fifo.consumed_by else {
            continue;
        };
        let Some(ref producer) = fifo.produced_by else {
            continue;
        };
        let consumer_inst = format!("{}_{}", consumer.0, consumer.1);
        let producer_inst = format!("{}_{}", producer.0, producer.1);
        let width = fifo_widths.get(fifo_name).copied().unwrap_or(32);

        edges.push(ABEdge {
            source_vertex: producer_inst,
            target_vertex: consumer_inst,
            width,
            index: edges.len(),
        });
    }

    ABGraph {
        vs: vertices.into_values().collect(),
        es: edges,
    }
}

/// Add port interface connections (dummy vertices + edges) for stream/mmap ports.
pub fn add_port_iface_connections(
    program: &Program,
    graph: &mut ABGraph,
    port_widths: &BTreeMap<String, PortWidth>,
    preassignments: &BTreeMap<String, String>,
) -> Result<(), FloorplanError> {
    let top = &program.tasks[&program.top];
    let mut vertex_names: BTreeMap<String, usize> = graph
        .vs
        .iter()
        .enumerate()
        .map(|(i, v)| (v.name.clone(), i))
        .collect();

    for port in &top.ports {
        let is_stream_or_mmap = matches!(
            port.cat,
            ArgCategory::Istream
                | ArgCategory::Ostream
                | ArgCategory::Istreams
                | ArgCategory::Ostreams
                | ArgCategory::Mmap
                | ArgCategory::AsyncMmap
                | ArgCategory::Immap
                | ArgCategory::Ommap
        );
        if !is_stream_or_mmap {
            continue;
        }

        let region = find_preassignment_region(&port.name, preassignments)?;
        let target_slot = region.map(|r| convert_region_format(&r));

        let dummy = ABVertex {
            name: port.name.clone(),
            sub_cells: vec![port.name.clone()],
            area: Area::default(),
            target_slot,
            reserved_slot: None,
        };

        let dummy_idx = graph.vs.len();
        let dummy_name = format!("{TAPA_PORT_PREFIX}{}", port.name);
        vertex_names.insert(dummy_name, dummy_idx);
        graph.vs.push(dummy);

        // Find connected task instance
        if let Some(connected_inst) = find_instance_for_port(top, &port.name) {
            let Some(&inst_idx) = vertex_names.get(&connected_inst) else {
                continue;
            };

            let pw = port_widths
                .get(&port.name)
                .copied()
                .unwrap_or_else(|| PortWidth::Simple(u64::from(port.width)));

            // Add edges based on port category
            let is_mmap = matches!(
                port.cat,
                ArgCategory::Mmap | ArgCategory::AsyncMmap | ArgCategory::Immap | ArgCategory::Ommap
            );

            if is_mmap || port.cat == ArgCategory::Istream || port.cat == ArgCategory::Istreams {
                graph.es.push(ABEdge {
                    source_vertex: graph.vs[dummy_idx].name.clone(),
                    target_vertex: graph.vs[inst_idx].name.clone(),
                    width: pw.read_width(),
                    index: graph.es.len(),
                });
            }

            if is_mmap || port.cat == ArgCategory::Ostream || port.cat == ArgCategory::Ostreams {
                graph.es.push(ABEdge {
                    source_vertex: graph.vs[inst_idx].name.clone(),
                    target_vertex: graph.vs[dummy_idx].name.clone(),
                    width: pw.write_width(),
                    index: graph.es.len(),
                });
            }
        }
    }

    Ok(())
}

/// Add scalar connections via a shared FSM vertex.
pub fn add_scalar_connections(
    program: &Program,
    graph: &mut ABGraph,
    port_widths: &BTreeMap<String, PortWidth>,
    fsm_name: &str,
) {
    let top = &program.tasks[&program.top];
    let vertex_names: BTreeMap<String, usize> = graph
        .vs
        .iter()
        .enumerate()
        .map(|(i, v)| (v.name.clone(), i))
        .collect();

    let mut fsm_added = false;

    for port in &top.ports {
        if port.cat != ArgCategory::Scalar {
            continue;
        }

        let Some(connected_inst) = find_instance_for_port(top, &port.name) else {
            continue;
        };
        let Some(&inst_idx) = vertex_names.get(&connected_inst) else {
            continue;
        };

        if !fsm_added {
            graph.vs.push(ABVertex {
                name: fsm_name.to_owned(),
                sub_cells: vec![fsm_name.to_owned()],
                area: Area::default(),
                target_slot: None,
                reserved_slot: None,
            });
            fsm_added = true;
        }

        let pw = port_widths
            .get(&port.name)
            .copied()
            .unwrap_or_else(|| PortWidth::Simple(u64::from(port.width)));

        graph.es.push(ABEdge {
            source_vertex: fsm_name.to_owned(),
            target_vertex: graph.vs[inst_idx].name.clone(),
            width: pw.write_width(),
            index: graph.es.len(),
        });
    }
}

/// Find the instance name connected to a top-level port by arg name.
fn find_instance_for_port(
    top: &tapa_topology::task::TaskDesign,
    port_name: &str,
) -> Option<String> {
    for (task_name, instances) in &top.tasks {
        for (idx, inst) in instances.iter().enumerate() {
            for arg in inst.args.values() {
                if arg.arg == port_name {
                    return Some(format!("{task_name}_{idx}"));
                }
            }
        }
    }
    None
}

/// Find a preassignment region for a port, checking for conflicts.
///
/// Uses full-match semantics (like Python `re.fullmatch`) by anchoring patterns
/// with `^` and `$`.
fn find_preassignment_region(
    port_name: &str,
    preassignments: &BTreeMap<String, String>,
) -> Result<Option<String>, FloorplanError> {
    let mut region: Option<String> = None;
    for (pattern, current_region) in preassignments {
        // Anchor the pattern for full-match semantics (Python re.fullmatch)
        let anchored = if pattern.starts_with('^') && pattern.ends_with('$') {
            pattern.clone()
        } else {
            format!("^(?:{pattern})$")
        };
        let re = Regex::new(&anchored).map_err(|e| {
            FloorplanError::InvalidDevice(format!("invalid regex pattern '{pattern}': {e}"))
        })?;
        if re.is_match(port_name) {
            if let Some(ref existing) = region {
                if existing != current_region {
                    return Err(FloorplanError::InvalidDevice(format!(
                        "port {port_name} matches multiple preassignment patterns: \
                         {existing} and {current_region}"
                    )));
                }
            }
            region = Some(current_region.clone());
        }
    }
    Ok(region)
}

/// Look up a FIFO data port's bit-width on a producer RTL module, trying
/// both `_din` (producer write side) and `_dout` (consumer read side on
/// pass-through wrappers). Delegates to `tapa_rtl::VerilogModule::get_port_of`,
/// which implements Python's `get_port_of` infix-lookup and singleton-array
/// fallback semantics.
fn find_fifo_port_width(module: &tapa_rtl::VerilogModule, port_name: &str) -> Option<u32> {
    for suffix in ["_din", "_dout"] {
        if let Some(port) = module.get_port_of(port_name, suffix) {
            let w = port.width.as_ref()?;
            let msb = parse_expr_as_u32(&w.msb)?;
            let lsb = parse_expr_as_u32(&w.lsb)?;
            return Some(msb.saturating_sub(lsb) + 1);
        }
    }
    None
}

/// Try to parse a simple expression (single numeric literal) as u32.
fn parse_expr_as_u32(expr: &[tapa_rtl::expression::Token]) -> Option<u32> {
    if expr.len() == 1 {
        expr[0].repr.parse::<u32>().ok()
    } else {
        None
    }
}

/// Convert a region format string (e.g., `SLOT_X0Y0:SLOT_X0Y0`) to slot name format.
fn convert_region_format(region: &str) -> String {
    if region.contains(':') {
        region.replace(':', "_TO_")
    } else {
        region.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_program() -> Program {
        serde_json::from_str(
            r#"{
                "top": "top_task",
                "target": "xilinx-hls",
                "tasks": {
                    "top_task": {
                        "level": "upper",
                        "code": "",
                        "target": "xilinx-hls",
                        "ports": [
                            {"cat": "istream", "name": "input_data", "type": "float", "width": 32},
                            {"cat": "ostream", "name": "output_data", "type": "float", "width": 32},
                            {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                        ],
                        "tasks": {
                            "producer": [{"args": {"out": {"arg": "fifo_0", "cat": "ostream"}, "n": {"arg": "n", "cat": "scalar"}}}],
                            "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}, "result": {"arg": "output_data", "cat": "ostream"}}}]
                        },
                        "fifos": {
                            "fifo_0": {
                                "depth": 16,
                                "consumed_by": ["consumer", 0],
                                "produced_by": ["producer", 0]
                            }
                        }
                    },
                    "producer": {
                        "level": "lower", "code": "", "target": "xilinx-hls",
                        "ports": [
                            {"cat": "ostream", "name": "out", "type": "float", "width": 32},
                            {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                        ],
                        "tasks": {}, "fifos": {}
                    },
                    "consumer": {
                        "level": "lower", "code": "", "target": "xilinx-hls",
                        "ports": [
                            {"cat": "istream", "name": "in_data", "type": "float", "width": 32},
                            {"cat": "ostream", "name": "result", "type": "float", "width": 32}
                        ],
                        "tasks": {}, "fifos": {}
                    }
                }
            }"#,
        )
        .expect("parse program")
    }

    #[test]
    fn collect_task_area_returns_entries() {
        let prog = make_program();
        let areas = collect_task_area(&prog);
        // Should have entries for producer and consumer
        assert!(areas.contains_key("producer"), "keys: {areas:?}");
        assert!(areas.contains_key("consumer"), "keys: {areas:?}");
    }

    #[test]
    fn collect_port_width_returns_entries() {
        let prog = make_program();
        let widths = collect_port_width(&prog);
        assert!(widths.contains_key("input_data"));
        assert!(widths.contains_key("output_data"));
        assert!(widths.contains_key("n"));
    }

    #[test]
    fn basic_ab_graph_vertices_and_edges() {
        let prog = make_program();
        let areas = collect_task_area(&prog);
        let mut fifo_widths = BTreeMap::new();
        fifo_widths.insert("fifo_0".into(), 32_u64);

        let graph = get_basic_ab_graph(&prog, &areas, &fifo_widths);

        // Should have 2 vertices (producer_0, consumer_0)
        assert_eq!(graph.vs.len(), 2, "vertices: {:?}", graph.vs.iter().map(|v| &v.name).collect::<Vec<_>>());
        // Should have 1 edge (fifo_0)
        assert_eq!(graph.es.len(), 1);
        assert_eq!(graph.es[0].width, 32);
    }

    #[test]
    fn add_port_iface_connections_adds_dummy_vertices() {
        let prog = make_program();
        let areas = collect_task_area(&prog);
        let fifo_widths = BTreeMap::from([("fifo_0".into(), 32_u64)]);
        let port_widths = collect_port_width(&prog);

        let mut graph = get_basic_ab_graph(&prog, &areas, &fifo_widths);
        let preassignments = BTreeMap::new();

        add_port_iface_connections(&prog, &mut graph, &port_widths, &preassignments).unwrap();

        // Should add dummy vertices for stream ports (input_data, output_data)
        assert!(
            graph.vs.len() > 2,
            "should have more than 2 vertices after port connections, got {}",
            graph.vs.len()
        );
    }

    #[test]
    fn add_scalar_connections_adds_fsm_vertex() {
        let prog = make_program();
        let areas = collect_task_area(&prog);
        let fifo_widths = BTreeMap::from([("fifo_0".into(), 32_u64)]);
        let port_widths = collect_port_width(&prog);

        let mut graph = get_basic_ab_graph(&prog, &areas, &fifo_widths);
        add_scalar_connections(&prog, &mut graph, &port_widths, "top_task_fsm");

        // Should have FSM vertex
        assert!(
            graph.find_vertex("top_task_fsm").is_some(),
            "should have FSM vertex"
        );
        // Should have scalar edges
        assert!(
            graph.es.len() > 1,
            "should have scalar edges, got {}",
            graph.es.len()
        );
    }

    #[test]
    fn preassignment_conflict_detected() {
        let mut preassignments = BTreeMap::new();
        preassignments.insert("input_.*".into(), "SLOT_X0Y0:SLOT_X0Y0".into());
        preassignments.insert("input_data".into(), "SLOT_X1Y1:SLOT_X1Y1".into());
        let result = find_preassignment_region("input_data", &preassignments);
        assert!(
            result.is_err(),
            "conflicting preassignments should error"
        );
    }

    #[test]
    fn preassignment_no_conflict() {
        let mut preassignments = BTreeMap::new();
        preassignments.insert("input_.*".into(), "SLOT_X0Y0:SLOT_X0Y0".into());
        let result = find_preassignment_region("input_data", &preassignments).unwrap();
        assert_eq!(result, Some("SLOT_X0Y0:SLOT_X0Y0".into()));
    }

    #[test]
    fn convert_region_format_colon_to_slot() {
        assert_eq!(
            convert_region_format("SLOT_X0Y0:SLOT_X0Y0"),
            "SLOT_X0Y0_TO_SLOT_X0Y0"
        );
    }

    #[test]
    fn collect_fifo_width_from_topology() {
        let prog = make_program();
        let widths = collect_fifo_width(&prog);
        assert!(widths.contains_key("fifo_0"), "keys: {widths:?}");
        // Producer port "out" has width 32
        assert_eq!(widths["fifo_0"], 32);
    }

    #[test]
    fn preassignment_fullmatch_rejects_partial() {
        // Pattern "input" should NOT match "input_data" with fullmatch
        let preassignments = BTreeMap::from([("input".into(), "SLOT_X0Y0:SLOT_X0Y0".into())]);
        let result = find_preassignment_region("input_data", &preassignments).unwrap();
        assert_eq!(result, None, "partial match should not assign");
    }

    #[test]
    fn preassignment_fullmatch_accepts_exact() {
        let preassignments = BTreeMap::from([("input_data".into(), "SLOT_X0Y0:SLOT_X0Y0".into())]);
        let result = find_preassignment_region("input_data", &preassignments).unwrap();
        assert_eq!(result, Some("SLOT_X0Y0:SLOT_X0Y0".into()));
    }

    #[test]
    fn get_top_level_ab_graph_works() {
        let prog = make_program();
        let preassignments = BTreeMap::new();
        let graph = get_top_level_ab_graph(&prog, &preassignments, "top_task_fsm").unwrap();
        // Should have instance vertices + port dummy vertices + FSM vertex
        assert!(graph.vs.len() >= 3, "should have at least 3 vertices, got {}", graph.vs.len());
        assert!(!graph.es.is_empty(), "should have edges");
    }

    #[test]
    fn abgraph_json_round_trip() {
        let prog = make_program();
        let areas = collect_task_area(&prog);
        let fifo_widths = BTreeMap::from([("fifo_0".into(), 32_u64)]);
        let graph = get_basic_ab_graph(&prog, &areas, &fifo_widths);

        let json = serde_json::to_string(&graph).unwrap();
        let graph2: ABGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(graph.vs.len(), graph2.vs.len());
        assert_eq!(graph.es.len(), graph2.es.len());
    }
}
