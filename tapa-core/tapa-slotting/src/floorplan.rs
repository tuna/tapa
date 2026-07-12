//! Floorplan graph rewrite: group task instances into slots.
//!
//! Implements: `get_floorplan_slot`, `get_floorplan_top`,
//! `get_floorplan_graph`, and their helper functions.
//!
//! Operates on typed [`tapa_task_graph::Graph`] to eliminate ad-hoc
//! `serde_json::Value` dict manipulation.

use std::collections::{BTreeMap, BTreeSet};

use tapa_task_graph::{
    Arg, ArgCategory, EndpointRef, Graph, InterconnectDefinition, Port, TaskDefinition,
    TaskInstance, TaskLevel,
};

use crate::error::SlottingError;

/// Generate a floorplanned graph by grouping instances into slots.
///
/// Takes the original typed graph and a mapping from slot names to
/// the list of instance names assigned to each slot.
///
/// Returns the modified graph with new slot task definitions
/// and a rewritten top-level task that instantiates slots.
pub fn get_floorplan_graph(
    graph: &Graph,
    slot_to_insts: &BTreeMap<String, Vec<String>>,
) -> Result<Graph, SlottingError> {
    let mut new_graph = graph.clone();
    let top_name = &graph.top;

    let top_task = new_graph
        .tasks
        .get(top_name)
        .ok_or_else(|| SlottingError::MissingGraphField(format!("top task `{top_name}`")))?;

    // Floorplan requires an upper-level top.
    if top_task.level == TaskLevel::Lower {
        return Err(SlottingError::TopIsLeaf(top_name.clone()));
    }

    // Build the set of known instance names under the top task.
    let mut known_inst_names = BTreeSet::new();
    for (def_name, insts) in &top_task.tasks {
        for idx in 0..insts.len() {
            known_inst_names.insert(format!("{def_name}_{idx}"));
        }
    }

    // Every top-level instance must be assigned to exactly one non-empty slot.
    let mut assigned_inst_names = BTreeSet::new();
    for (slot_name, inst_names) in slot_to_insts {
        if inst_names.is_empty() {
            return Err(SlottingError::EmptyFloorplanSlot(slot_name.clone()));
        }
        for inst_name in inst_names {
            if !known_inst_names.contains(inst_name) {
                return Err(SlottingError::UnknownFloorplanInstance(inst_name.clone()));
            }
            if !assigned_inst_names.insert(inst_name.clone()) {
                return Err(SlottingError::DuplicateFloorplanInstance(inst_name.clone()));
            }
        }
    }
    if let Some(unassigned) = known_inst_names.difference(&assigned_inst_names).next() {
        return Err(SlottingError::UnassignedFloorplanInstance(
            unassigned.clone(),
        ));
    }

    // Validate slot names do not collide with existing tasks.
    for slot_name in slot_to_insts.keys() {
        if new_graph.tasks.contains_key(slot_name) {
            return Err(SlottingError::SlotNameCollision(slot_name.clone()));
        }
    }

    // Build slot definitions
    let mut slot_defs: BTreeMap<String, TaskDefinition> = BTreeMap::new();
    for (slot_name, insts) in slot_to_insts {
        let slot_def = build_floorplan_slot(graph, slot_name, insts, top_name)?;
        new_graph.tasks.insert(slot_name.clone(), slot_def.clone());
        slot_defs.insert(slot_name.clone(), slot_def);
    }

    // Build inst->slot mapping
    let inst_to_slot: BTreeMap<String, String> = slot_to_insts
        .iter()
        .flat_map(|(slot, insts)| insts.iter().map(move |inst| (inst.clone(), slot.clone())))
        .collect();

    // Rewrite top task
    let top_task = build_floorplan_top(graph, &slot_defs, &inst_to_slot, top_name);
    new_graph.tasks.insert(top_name.clone(), top_task);

    Ok(new_graph)
}

/// Build a slot task definition grouping the specified instances.
fn build_floorplan_slot(
    graph: &Graph,
    slot_name: &str,
    task_inst_in_slot: &[String],
    top_name: &str,
) -> Result<TaskDefinition, SlottingError> {
    let top_task = &graph.tasks[top_name];
    let mut slot_def = top_task.clone();
    slot_def.level = TaskLevel::Upper;

    // Build instance mapping: top_index -> slot_index
    // Also track original top-level instance names for mmap port inference
    let mut new_tasks: BTreeMap<String, Vec<TaskInstance>> = BTreeMap::new();
    let mut top_to_slot_idx: BTreeMap<String, BTreeMap<usize, usize>> = BTreeMap::new();
    let mut original_inst_names: Vec<String> = Vec::new();
    let inst_set: BTreeSet<&str> = task_inst_in_slot.iter().map(String::as_str).collect();

    for (task_name, insts) in &top_task.tasks {
        for (top_idx, inst) in insts.iter().enumerate() {
            let inst_name = get_instance_name(task_name, top_idx);
            if !inst_set.contains(inst_name.as_str()) {
                continue;
            }
            let slot_idx = new_tasks.entry(task_name.clone()).or_default().len();
            top_to_slot_idx
                .entry(task_name.clone())
                .or_default()
                .insert(top_idx, slot_idx);
            // Connect mmap ports to slot-level ports using ORIGINAL instance name
            let new_inst = connect_subinst_mmap_to_slot_port(inst, &inst_name);
            new_tasks
                .entry(task_name.clone())
                .or_default()
                .push(new_inst);
            original_inst_names.push(inst_name);
        }
    }

    slot_def.tasks = new_tasks;

    // Rewrite FIFOs
    let (new_fifos, fifo_ports) = get_slot_fifos(&top_task.fifos, &top_to_slot_idx, &inst_set);
    slot_def.fifos = new_fifos;

    // Build ports: scalar args + FIFO-connected ports + inferred mmap ports
    let mut new_ports: Vec<Port> = Vec::new();

    // Collect scalar args from instances in slot
    let mut scalar_args: BTreeSet<String> = BTreeSet::new();
    for insts in slot_def.tasks.values() {
        for inst in insts {
            for arg in inst.args.values() {
                if arg.cat == ArgCategory::Scalar {
                    scalar_args.insert(arg.arg.clone());
                }
            }
        }
    }

    // Keep scalar ports from top task
    for port in &top_task.ports {
        if scalar_args.contains(&port.name) {
            new_ports.push(port.clone());
        }
    }

    // Add FIFO-connected ports
    new_ports.extend(get_used_ports(
        graph,
        top_name,
        &slot_def.tasks,
        &fifo_ports,
    ));

    // Add inferred mmap ports (using original top-level instance names)
    new_ports.extend(infer_mmap_ports_from_subtasks(
        graph,
        &slot_def.tasks,
        &original_inst_names,
    ));

    // Deduplicate by name
    let mut seen = BTreeSet::new();
    new_ports.retain(|p| seen.insert(p.name.clone()));

    slot_def.ports = new_ports;

    // Generate slot C++ using gen_slot_cpp
    let top_code = &top_task.code;
    let top_task_name = top_name;
    let slot_ports: Vec<crate::SlotPort> = slot_def
        .ports
        .iter()
        .map(|p| crate::SlotPort {
            cat: p.cat.as_str().to_owned(),
            name: p.name.clone(),
            port_type: p.ctype.clone(),
        })
        .collect();

    let new_code = crate::gen_slot_cpp(slot_name, top_task_name, &slot_ports, top_code)?;
    slot_def.code = new_code;

    Ok(slot_def)
}

/// Build the rewritten top-level task that instantiates slots.
fn build_floorplan_top(
    graph: &Graph,
    slot_defs: &BTreeMap<String, TaskDefinition>,
    inst_to_slot: &BTreeMap<String, String>,
    top_name: &str,
) -> TaskDefinition {
    let top_task = &graph.tasks[top_name];
    let mut new_top = top_task.clone();

    // Build slot instances
    let new_insts = build_top_slot_insts(slot_defs, &top_task.tasks, inst_to_slot);
    new_top.tasks = new_insts;

    // Collect in-slot internal FIFOs to exclude from top
    let mut in_slot_fifos: BTreeSet<String> = BTreeSet::new();
    for slot_def in slot_defs.values() {
        for (name, fifo) in &slot_def.fifos {
            if fifo.depth.is_some() {
                in_slot_fifos.insert(name.clone());
            }
        }
    }

    // Update cross-slot FIFOs
    let new_fifos = update_cross_slot_fifos(&top_task.fifos, &in_slot_fifos, inst_to_slot);
    new_top.fifos = new_fifos;

    new_top
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Get instance name from task name and index: `{task}_{idx}`.
fn get_instance_name(task_name: &str, idx: usize) -> String {
    format!("{task_name}_{idx}")
}

fn is_mmap_category(cat: ArgCategory) -> bool {
    matches!(
        cat,
        ArgCategory::Mmap | ArgCategory::AsyncMmap | ArgCategory::Immap | ArgCategory::Ommap
    )
}

/// Connect mmap args in a sub-instance to slot-level port names.
fn connect_subinst_mmap_to_slot_port(inst: &TaskInstance, inst_name: &str) -> TaskInstance {
    let mut new_inst = inst.clone();
    let mut new_args = BTreeMap::new();
    for (port_name, arg) in &inst.args {
        if is_mmap_category(arg.cat) {
            new_args.insert(
                port_name.clone(),
                Arg {
                    arg: format!("{port_name}_{inst_name}"),
                    cat: arg.cat,
                },
            );
        } else {
            new_args.insert(port_name.clone(), arg.clone());
        }
    }
    new_inst.args = new_args;
    new_inst
}

/// Get slot FIFOs: internal FIFOs stay, cross-slot become external.
fn get_slot_fifos(
    top_fifos: &BTreeMap<String, InterconnectDefinition>,
    top_to_slot_idx: &BTreeMap<String, BTreeMap<usize, usize>>,
    inst_set: &BTreeSet<&str>,
) -> (BTreeMap<String, InterconnectDefinition>, Vec<String>) {
    let mut new_fifos: BTreeMap<String, InterconnectDefinition> = BTreeMap::new();
    let mut fifo_ports: Vec<String> = Vec::new();

    for (name, fifo) in top_fifos {
        // Skip external FIFOs (no depth)
        if fifo.depth.is_none() {
            continue;
        }

        let src_in_slot = endpoint_in_set(fifo.consumed_by.as_ref(), inst_set);
        let dst_in_slot = endpoint_in_set(fifo.produced_by.as_ref(), inst_set);

        if src_in_slot && dst_in_slot {
            // Internal: update indices
            let updated = update_fifo_inst_idx(fifo, top_to_slot_idx);
            new_fifos.insert(name.clone(), updated);
        } else if src_in_slot {
            // Consumer in slot, producer outside -> external
            let updated_src = update_endpoint_idx(fifo.consumed_by.as_ref(), top_to_slot_idx);
            new_fifos.insert(
                name.clone(),
                InterconnectDefinition {
                    depth: None,
                    consumed_by: updated_src,
                    produced_by: None,
                },
            );
            fifo_ports.push(name.clone());
        } else if dst_in_slot {
            // Producer in slot, consumer outside -> external
            let updated_dst = update_endpoint_idx(fifo.produced_by.as_ref(), top_to_slot_idx);
            new_fifos.insert(
                name.clone(),
                InterconnectDefinition {
                    depth: None,
                    consumed_by: None,
                    produced_by: updated_dst,
                },
            );
            fifo_ports.push(name.clone());
        }
    }

    (new_fifos, fifo_ports)
}

/// Check if a FIFO endpoint is in the instance set.
fn endpoint_in_set(endpoint: Option<&EndpointRef>, inst_set: &BTreeSet<&str>) -> bool {
    endpoint.is_some_and(|ep| {
        let inst_name = get_instance_name(&ep.0, ep.1 as usize);
        inst_set.contains(inst_name.as_str())
    })
}

/// Update FIFO endpoint indices from top to slot.
fn update_endpoint_idx(
    endpoint: Option<&EndpointRef>,
    idx_map: &BTreeMap<String, BTreeMap<usize, usize>>,
) -> Option<EndpointRef> {
    endpoint.and_then(|ep| {
        let name = &ep.0;
        let top_idx = ep.1 as usize;
        idx_map
            .get(name)
            .and_then(|m| m.get(&top_idx))
            .map(|&slot_idx| {
                EndpointRef(
                    name.clone(),
                    u32::try_from(slot_idx).expect("slot index fits in u32"),
                )
            })
    })
}

/// Update FIFO instance indices from top to slot.
fn update_fifo_inst_idx(
    fifo: &InterconnectDefinition,
    idx_map: &BTreeMap<String, BTreeMap<usize, usize>>,
) -> InterconnectDefinition {
    let mut result = fifo.clone();
    result.consumed_by = fifo
        .consumed_by
        .as_ref()
        .and_then(|ep| update_endpoint_idx(Some(ep), idx_map));
    result.produced_by = fifo
        .produced_by
        .as_ref()
        .and_then(|ep| update_endpoint_idx(Some(ep), idx_map));
    result
}

fn task_port_matches_instance_port(task_port: &str, instance_port: &str) -> bool {
    if task_port == instance_port {
        return true;
    }
    instance_port
        .strip_prefix(task_port)
        .and_then(|suffix| suffix.strip_prefix('['))
        .and_then(|index| index.strip_suffix(']'))
        .is_some_and(|index| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()))
}

/// Find ports connected to FIFO endpoints.
fn get_used_ports(
    graph: &Graph,
    _top_name: &str,
    new_tasks: &BTreeMap<String, Vec<TaskInstance>>,
    fifo_ports: &[String],
) -> Vec<Port> {
    let fifo_set: BTreeSet<&str> = fifo_ports.iter().map(String::as_str).collect();
    let mut new_ports = Vec::new();

    for (task_name, insts) in new_tasks {
        let task_ports = graph
            .tasks
            .get(task_name)
            .map_or(&[] as &[tapa_task_graph::port::Port], |t| {
                t.ports.as_slice()
            });
        for inst in insts {
            for (port_name, arg) in &inst.args {
                if is_mmap_category(arg.cat) {
                    continue;
                }
                let arg_name = &arg.arg;
                if !fifo_set.contains(arg_name.as_str()) {
                    continue;
                }
                // Find matching port in task definition by port_name (arg key).
                for port in task_ports {
                    if task_port_matches_instance_port(&port.name, port_name) {
                        let mut new_port = port.clone();
                        new_port.name.clone_from(arg_name);
                        new_ports.push(new_port);
                        break;
                    }
                }
            }
        }
    }

    new_ports
}

/// Infer mmap ports from child instance definitions.
///
/// Uses the original top-level instance names (not slot-local indices)
/// to match the port names created by `connect_subinst_mmap_to_slot_port`.
fn infer_mmap_ports_from_subtasks(
    graph: &Graph,
    new_tasks: &BTreeMap<String, Vec<TaskInstance>>,
    original_inst_names: &[String],
) -> Vec<Port> {
    let mut ports = Vec::new();
    let mut name_iter = original_inst_names.iter();
    for (task_name, insts) in new_tasks {
        let task_ports = graph
            .tasks
            .get(task_name)
            .map_or(&[] as &[tapa_task_graph::port::Port], |t| {
                t.ports.as_slice()
            });
        for _inst in insts {
            // Use original top-level instance name, NOT slot-local index
            let inst_name = match name_iter.next() {
                Some(name) => name.clone(),
                None => task_name.clone(),
            };
            for port in task_ports {
                if is_mmap_category(port.cat) {
                    ports.push(Port {
                        cat: port.cat,
                        name: format!("{}_{}", port.name, inst_name),
                        ctype: port.ctype.clone(),
                        width: port.width,
                        chan_count: port.chan_count,
                        chan_size: port.chan_size,
                    });
                }
            }
        }
    }
    ports
}

/// Build slot instances for the rewritten top task.
fn build_top_slot_insts(
    slot_defs: &BTreeMap<String, TaskDefinition>,
    top_tasks: &BTreeMap<String, Vec<TaskInstance>>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<TaskInstance>> {
    let mut new_top_insts: BTreeMap<String, Vec<TaskInstance>> = BTreeMap::new();

    for (slot_name, slot_def) in slot_defs {
        let slot_ports: BTreeMap<String, &Port> =
            slot_def.ports.iter().map(|p| (p.name.clone(), p)).collect();

        let slot_subtasks = &slot_def.tasks;

        let mut args = BTreeMap::new();
        for (port_name, port) in &slot_ports {
            if is_mmap_category(port.cat) {
                continue;
            }
            let formatted = port_name.replace('[', "_").replace(']', "");
            let inferred_cat = infer_arg_cat_from_subinst(port_name, slot_subtasks);
            args.insert(
                formatted,
                Arg {
                    arg: port_name.clone(),
                    cat: inferred_cat,
                },
            );
        }

        // Add mmap port args
        let mmap_args = get_slot_inst_mmap_port_args(slot_name, top_tasks, inst_to_slot);
        for (k, v) in mmap_args {
            args.insert(k, v);
        }

        new_top_insts
            .entry(slot_name.clone())
            .or_default()
            .push(TaskInstance {
                name: None,
                args,
                step: 0,
            });
    }

    new_top_insts
}

/// Infer port category from child instances.
fn infer_arg_cat_from_subinst(
    port_name: &str,
    tasks: &BTreeMap<String, Vec<TaskInstance>>,
) -> ArgCategory {
    for insts in tasks.values() {
        for inst in insts {
            for arg in inst.args.values() {
                if arg.arg == port_name {
                    return arg.cat;
                }
            }
        }
    }
    ArgCategory::Scalar
}

/// Update cross-slot FIFOs: remap endpoints to slot instances.
fn update_cross_slot_fifos(
    top_fifos: &BTreeMap<String, InterconnectDefinition>,
    in_slot_fifos: &BTreeSet<String>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, InterconnectDefinition> {
    let mut new_fifos = BTreeMap::new();
    for (name, fifo) in top_fifos {
        if in_slot_fifos.contains(name) {
            continue;
        }
        let mut updated = fifo.clone();
        if let Some(consumed) = &fifo.consumed_by {
            updated.consumed_by = Some(remap_endpoint_to_slot(consumed, inst_to_slot));
        }
        if let Some(produced) = &fifo.produced_by {
            updated.produced_by = Some(remap_endpoint_to_slot(produced, inst_to_slot));
        }
        new_fifos.insert(name.clone(), updated);
    }
    new_fifos
}

/// Remap a FIFO endpoint from (task, idx) to (slot, 0).
fn remap_endpoint_to_slot(
    endpoint: &EndpointRef,
    inst_to_slot: &BTreeMap<String, String>,
) -> EndpointRef {
    let inst_name = get_instance_name(&endpoint.0, endpoint.1 as usize);
    if let Some(slot) = inst_to_slot.get(&inst_name) {
        EndpointRef(slot.clone(), 0)
    } else {
        endpoint.clone()
    }
}

/// Get mmap port args for a slot instance in the rewritten top.
fn get_slot_inst_mmap_port_args(
    slot_name: &str,
    top_tasks: &BTreeMap<String, Vec<TaskInstance>>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, Arg> {
    let mut args = BTreeMap::new();
    for (task_name, insts) in top_tasks {
        for (idx, inst) in insts.iter().enumerate() {
            let inst_name = get_instance_name(task_name, idx);
            if inst_to_slot.get(&inst_name).map(String::as_str) != Some(slot_name) {
                continue;
            }
            for (port_name, arg) in &inst.args {
                if is_mmap_category(arg.cat) {
                    let slot_port_name = format!("{port_name}_{inst_name}");
                    args.insert(slot_port_name, arg.clone());
                }
            }
        }
    }
    args
}

/// Convert a floorplan region string from `"x:y"` form to the
/// canonical `"x_TO_y"` form used by `slot_task_name_to_fp_region`.
#[must_use]
pub fn convert_region_format(region: &str) -> String {
    region.replace(':', "_TO_")
}

/// Compute the slot name from a region by replacing `:`
/// with `_` (mirrors `slot_name = "_".join(region.split(":"))`).
#[must_use]
pub fn region_to_slot_name(region: &str) -> String {
    region.replace(':', "_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph_from_value(value: serde_json::Value) -> Graph {
        serde_json::from_value(value).expect("valid graph")
    }

    fn sample_graph() -> serde_json::Value {
        json!({
            "top": "top_func",
            "tasks": {
                "top_func": {
                    "level": "upper",
                    "code": "extern \"C\" {\nvoid top_func(int a) { /* body */ }\n}  // extern \"C\"\n",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "size", "type": "int", "width": 32},
                        {"cat": "istream", "name": "in_data", "type": "float", "width": 32}
                    ],
                    "tasks": {
                        "producer": [
                            {"args": {"data_out": {"arg": "fifo_0", "cat": "ostream"}, "n": {"arg": "size", "cat": "scalar"}}, "step": 0}
                        ],
                        "consumer": [
                            {"args": {"data_in": {"arg": "fifo_0", "cat": "istream"}, "n": {"arg": "size", "cat": "scalar"}}, "step": 1}
                        ]
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
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "data_out", "type": "float", "width": 32},
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "data_in", "type": "float", "width": 32},
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        })
    }

    #[test]
    fn floorplan_graph_creates_slot_task() {
        let graph = graph_from_value(sample_graph());
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();

        // Should have the slot task
        assert!(
            result.tasks.contains_key("SLOT_X0Y0_TO_SLOT_X1Y1"),
            "slot task should exist"
        );
        // Slot task should be upper level
        assert_eq!(
            result.tasks["SLOT_X0Y0_TO_SLOT_X1Y1"].level,
            TaskLevel::Upper
        );
    }

    #[test]
    fn floorplan_graph_internal_fifo_stays() {
        let graph = graph_from_value(sample_graph());
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot_fifos = &result.tasks["SLOT_X0Y0_TO_SLOT_X1Y1"].fifos;
        // fifo_0 should stay internal (both endpoints in slot)
        assert!(
            slot_fifos.contains_key("fifo_0"),
            "internal FIFO should be preserved in slot"
        );
        assert!(
            slot_fifos["fifo_0"].depth.is_some(),
            "internal FIFO should keep depth"
        );
    }

    #[test]
    fn floorplan_graph_top_task_rewritten() {
        let graph = graph_from_value(sample_graph());
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let top = &result.tasks["top_func"];

        // Top task should now instantiate the slot
        assert!(
            top.tasks.contains_key("SLOT_X0Y0_TO_SLOT_X1Y1"),
            "top should instantiate slot, got tasks: {:?}",
            top.tasks.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn instance_name_format() {
        assert_eq!(get_instance_name("producer", 0), "producer_0");
        assert_eq!(get_instance_name("task", 5), "task_5");
    }

    #[test]
    fn task_port_matching_accepts_only_exact_or_array_elements() {
        assert!(task_port_matches_instance_port("in", "in"));
        assert!(task_port_matches_instance_port("in", "in[12]"));
        assert!(!task_port_matches_instance_port("in", "input"));
        assert!(!task_port_matches_instance_port("in", "in[0]_suffix"));
        assert!(!task_port_matches_instance_port("in", "in[]"));
    }

    #[test]
    fn floorplan_mmap_ports_use_original_instance_names() {
        // Test with nonzero instance index and mmap ports
        let graph = graph_from_value(json!({
            "top": "top_func",
            "tasks": {
                "top_func": {
                    "level": "upper",
                    "code": "extern \"C\" {\nvoid top_func(int a) {}\n}  // extern \"C\"\n",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "mem", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "worker": [
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}, "step": 0},
                            {"args": {"data": {"arg": "mem", "cat": "mmap"}}, "step": 0}
                        ]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "mmap", "name": "data", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        // Put worker_1 in the slot under test and worker_0 in a different slot.
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["worker_1".to_owned()],
        );
        slot_to_insts.insert("OTHER_SLOT".to_owned(), vec!["worker_0".to_owned()]);

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot = &result.tasks["SLOT_X0Y0_TO_SLOT_X1Y1"];
        let slot_ports = &slot.ports;

        // The mmap port should use the original instance name "worker_1",
        // NOT the slot-local index "worker_0"
        let mmap_ports: Vec<&str> = slot_ports
            .iter()
            .filter(|p| p.cat == ArgCategory::Mmap)
            .map(|p| p.name.as_str())
            .collect();

        assert!(
            mmap_ports.iter().any(|n| n.contains("worker_1")),
            "mmap port should use original instance name worker_1, got: {mmap_ports:?}"
        );
        assert!(
            !mmap_ports.iter().any(|n| n.contains("worker_0")),
            "mmap port should NOT use slot-local index worker_0, got: {mmap_ports:?}"
        );
    }

    #[test]
    fn floorplan_preserves_directional_mmap_ports() {
        let graph = graph_from_value(json!({
            "top": "top_func",
            "tasks": {
                "top_func": {
                    "level": "upper",
                    "code": "extern \"C\" {\nvoid top_func() {}\n}  // extern \"C\"\n",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "immap", "name": "src", "type": "uint64_t", "width": 64},
                        {"cat": "ommap", "name": "dst", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "worker": [{"args": {
                            "input_mem": {"arg": "src", "cat": "immap"},
                            "output_mem": {"arg": "dst", "cat": "ommap"}
                        }, "step": 0}]
                    },
                    "fifos": {}
                },
                "worker": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "immap", "name": "input_mem", "type": "uint64_t", "width": 64},
                        {"cat": "ommap", "name": "output_mem", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let slot_name = "SLOT_X0Y0_TO_SLOT_X0Y0";
        let slot_to_insts = BTreeMap::from([(slot_name.to_owned(), vec!["worker_0".to_owned()])]);

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot = &result.tasks[slot_name];
        assert!(slot
            .ports
            .iter()
            .any(|p| p.name == "input_mem_worker_0" && p.cat == ArgCategory::Immap));
        assert!(slot
            .ports
            .iter()
            .any(|p| p.name == "output_mem_worker_0" && p.cat == ArgCategory::Ommap));

        let worker = &slot.tasks["worker"][0];
        assert_eq!(worker.args["input_mem"].arg, "input_mem_worker_0");
        assert_eq!(worker.args["output_mem"].arg, "output_mem_worker_0");

        let slot_inst = &result.tasks["top_func"].tasks[slot_name][0];
        assert_eq!(slot_inst.args["input_mem_worker_0"].arg, "src");
        assert_eq!(slot_inst.args["input_mem_worker_0"].cat, ArgCategory::Immap);
        assert_eq!(slot_inst.args["output_mem_worker_0"].arg, "dst");
        assert_eq!(
            slot_inst.args["output_mem_worker_0"].cat,
            ArgCategory::Ommap
        );
    }

    /// Build a graph with two slots where a FIFO crosses the slot boundary.
    /// The cross-slot FIFO endpoints in the rewritten top should reference
    /// slot names, not the original task names.
    #[test]
    fn test_floorplan_cross_slot_fifo_remapping() {
        let graph = graph_from_value(json!({
            "top": "top_func",
            "tasks": {
                "top_func": {
                    "level": "upper",
                    "code": "extern \"C\" {\nvoid top_func(int a) { /* body */ }\n}  // extern \"C\"\n",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "size", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "producer": [
                            {"args": {"data_out": {"arg": "cross_fifo", "cat": "ostream"}, "n": {"arg": "size", "cat": "scalar"}}, "step": 0}
                        ],
                        "consumer": [
                            {"args": {"data_in": {"arg": "cross_fifo", "cat": "istream"}, "n": {"arg": "size", "cat": "scalar"}}, "step": 1}
                        ]
                    },
                    "fifos": {
                        "cross_fifo": {
                            "depth": 32,
                            "consumed_by": ["consumer", 0],
                            "produced_by": ["producer", 0]
                        }
                    }
                },
                "producer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "data_out", "type": "float", "width": 32},
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "data_in", "type": "float", "width": 32},
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        // Put producer and consumer in different slots
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert("SLOT_A".to_owned(), vec!["producer_0".to_owned()]);
        slot_to_insts.insert("SLOT_B".to_owned(), vec!["consumer_0".to_owned()]);

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let top = &result.tasks["top_func"];

        // The cross-slot FIFO should still exist in top-level
        assert!(
            top.fifos.contains_key("cross_fifo"),
            "cross-slot FIFO should remain in top, got keys: {:?}",
            top.fifos.keys().collect::<Vec<_>>()
        );

        let cross = &top.fifos["cross_fifo"];

        // Endpoints should reference slot names, not original task names
        let consumed = cross
            .consumed_by
            .as_ref()
            .expect("consumed_by should be present");
        let produced = cross
            .produced_by
            .as_ref()
            .expect("produced_by should be present");

        assert_eq!(
            consumed.0, "SLOT_B",
            "consumed_by should reference SLOT_B, got: {consumed:?}"
        );
        assert_eq!(
            produced.0, "SLOT_A",
            "produced_by should reference SLOT_A, got: {produced:?}"
        );

        // Neither slot should contain the cross-slot FIFO as internal (with depth)
        let fifos_a = &result.tasks["SLOT_A"].fifos;
        let fifos_b = &result.tasks["SLOT_B"].fifos;

        // SLOT_A has the producer side (produced_by endpoint); it should NOT have depth
        if let Some(fifo) = fifos_a.get("cross_fifo") {
            assert!(
                fifo.depth.is_none(),
                "cross-slot FIFO in SLOT_A should not have depth"
            );
        }
        if let Some(fifo) = fifos_b.get("cross_fifo") {
            assert!(
                fifo.depth.is_none(),
                "cross-slot FIFO in SLOT_B should not have depth"
            );
        }
    }

    #[test]
    fn floorplan_fifo_port_matching_rejects_plain_prefixes() {
        let graph = graph_from_value(json!({
            "top": "top_func",
            "tasks": {
                "top_func": {
                    "level": "upper",
                    "code": "extern \"C\" {\nvoid top_func() {}\n}  // extern \"C\"\n",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {
                        "consumer": [{"args": {
                            "input": {"arg": "cross_fifo", "cat": "istream"}
                        }, "step": 0}],
                        "producer": [{"args": {
                            "output": {"arg": "cross_fifo", "cat": "ostream"}
                        }, "step": 0}]
                    },
                    "fifos": {
                        "cross_fifo": {
                            "depth": 2,
                            "consumed_by": ["consumer", 0],
                            "produced_by": ["producer", 0]
                        }
                    }
                },
                "consumer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "in", "type": "uint8_t", "width": 8},
                        {"cat": "istream", "name": "input", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "producer": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "ostream", "name": "output", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));
        let slot_to_insts = BTreeMap::from([
            ("SLOT_A".to_owned(), vec!["producer_0".to_owned()]),
            ("SLOT_B".to_owned(), vec!["consumer_0".to_owned()]),
        ]);

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let port = result.tasks["SLOT_B"]
            .ports
            .iter()
            .find(|port| port.name == "cross_fifo")
            .expect("consumer slot exposes cross-slot FIFO");
        assert_eq!(port.ctype, "uint64_t");
        assert_eq!(port.width, 64);
    }

    /// Verify that multiple slots are created and the top task instantiates both.
    #[test]
    #[allow(clippy::too_many_lines, reason = "complex test fixture setup")]
    fn test_floorplan_multiple_slots() {
        let graph = graph_from_value(json!({
            "top": "top_func",
            "tasks": {
                "top_func": {
                    "level": "upper",
                    "code": "extern \"C\" {\nvoid top_func(int a) { /* body */ }\n}  // extern \"C\"\n",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "task_a": [
                            {"args": {"n": {"arg": "n", "cat": "scalar"}, "out": {"arg": "f0", "cat": "ostream"}}, "step": 0}
                        ],
                        "task_b": [
                            {"args": {"n": {"arg": "n", "cat": "scalar"}, "in0": {"arg": "f0", "cat": "istream"}, "out": {"arg": "f1", "cat": "ostream"}}, "step": 1}
                        ],
                        "task_c": [
                            {"args": {"n": {"arg": "n", "cat": "scalar"}, "in0": {"arg": "f1", "cat": "istream"}}, "step": 2}
                        ]
                    },
                    "fifos": {
                        "f0": {"depth": 8, "consumed_by": ["task_b", 0], "produced_by": ["task_a", 0]},
                        "f1": {"depth": 8, "consumed_by": ["task_c", 0], "produced_by": ["task_b", 0]}
                    }
                },
                "task_a": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32},
                        {"cat": "ostream", "name": "out", "type": "float", "width": 32}
                    ],
                    "tasks": {}, "fifos": {}
                },
                "task_b": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32},
                        {"cat": "istream", "name": "in0", "type": "float", "width": 32},
                        {"cat": "ostream", "name": "out", "type": "float", "width": 32}
                    ],
                    "tasks": {}, "fifos": {}
                },
                "task_c": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32},
                        {"cat": "istream", "name": "in0", "type": "float", "width": 32}
                    ],
                    "tasks": {}, "fifos": {}
                }
            }
        }));

        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_LEFT".to_owned(),
            vec!["task_a_0".to_owned(), "task_b_0".to_owned()],
        );
        slot_to_insts.insert("SLOT_RIGHT".to_owned(), vec!["task_c_0".to_owned()]);

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();

        // Both slot tasks should exist
        assert!(
            result.tasks.contains_key("SLOT_LEFT"),
            "SLOT_LEFT task should exist"
        );
        assert!(
            result.tasks.contains_key("SLOT_RIGHT"),
            "SLOT_RIGHT task should exist"
        );

        // Both should be upper level
        assert_eq!(
            result.tasks["SLOT_LEFT"].level,
            TaskLevel::Upper,
            "SLOT_LEFT should be upper"
        );
        assert_eq!(
            result.tasks["SLOT_RIGHT"].level,
            TaskLevel::Upper,
            "SLOT_RIGHT should be upper"
        );

        // Top task should instantiate both slots
        let top = &result.tasks["top_func"];
        assert!(
            top.tasks.contains_key("SLOT_LEFT"),
            "top should instantiate SLOT_LEFT"
        );
        assert!(
            top.tasks.contains_key("SLOT_RIGHT"),
            "top should instantiate SLOT_RIGHT"
        );

        // SLOT_LEFT should contain task_a and task_b
        let left_tasks = &result.tasks["SLOT_LEFT"].tasks;
        assert!(
            left_tasks.contains_key("task_a"),
            "SLOT_LEFT should contain task_a, got: {:?}",
            left_tasks.keys().collect::<Vec<_>>()
        );
        assert!(
            left_tasks.contains_key("task_b"),
            "SLOT_LEFT should contain task_b, got: {:?}",
            left_tasks.keys().collect::<Vec<_>>()
        );

        // SLOT_RIGHT should contain task_c
        let right_tasks = &result.tasks["SLOT_RIGHT"].tasks;
        assert!(
            right_tasks.contains_key("task_c"),
            "SLOT_RIGHT should contain task_c, got: {:?}",
            right_tasks.keys().collect::<Vec<_>>()
        );
    }

    /// Verify that scalar ports used by instances in a slot are preserved
    /// in the slot's port list.
    #[test]
    fn test_floorplan_scalar_port_preservation() {
        let graph = graph_from_value(sample_graph());
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot = &result.tasks["SLOT_X0Y0_TO_SLOT_X1Y1"];
        let slot_ports = &slot.ports;

        // The "size" scalar port is used by both producer and consumer
        let scalar_port_names: Vec<&str> = slot_ports
            .iter()
            .filter(|p| p.cat == ArgCategory::Scalar)
            .map(|p| p.name.as_str())
            .collect();

        assert!(
            scalar_port_names.contains(&"size"),
            "slot should preserve scalar port 'size', got: {scalar_port_names:?}"
        );

        // Verify the scalar port retains its type and width
        let size_port = slot_ports
            .iter()
            .find(|p| p.name == "size")
            .expect("size port should exist");
        assert_eq!(size_port.ctype, "int", "size port should retain type 'int'");
        assert_eq!(size_port.width, 32, "size port should retain width 32");
    }

    /// Reject floorplans that reference an unknown instance name.
    #[test]
    fn test_floorplan_rejects_unknown_instance() {
        let graph = graph_from_value(sample_graph());
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert("SLOT".to_owned(), vec!["NoSuch_0".to_owned()]);
        let err = get_floorplan_graph(&graph, &slot_to_insts).expect_err("must reject");
        assert!(
            matches!(err, SlottingError::UnknownFloorplanInstance(_)),
            "expected UnknownFloorplanInstance, got: {err:?}"
        );
    }

    #[test]
    fn test_floorplan_rejects_unassigned_instance() {
        let graph = graph_from_value(sample_graph());
        let slot_to_insts = BTreeMap::from([("SLOT".to_owned(), vec!["producer_0".to_owned()])]);

        let err = get_floorplan_graph(&graph, &slot_to_insts).expect_err("must reject");
        assert!(
            matches!(
                err,
                SlottingError::UnassignedFloorplanInstance(ref name) if name == "consumer_0"
            ),
            "expected unassigned consumer_0, got: {err:?}"
        );
    }

    #[test]
    fn test_floorplan_rejects_duplicate_instance_assignment() {
        let graph = graph_from_value(sample_graph());
        let slot_to_insts = BTreeMap::from([
            (
                "SLOT_A".to_owned(),
                vec!["producer_0".to_owned(), "consumer_0".to_owned()],
            ),
            ("SLOT_B".to_owned(), vec!["producer_0".to_owned()]),
        ]);

        let err = get_floorplan_graph(&graph, &slot_to_insts).expect_err("must reject");
        assert!(
            matches!(
                err,
                SlottingError::DuplicateFloorplanInstance(ref name) if name == "producer_0"
            ),
            "expected duplicate producer_0, got: {err:?}"
        );
    }

    /// Snapshot the slot wrapper C++ and a compact form of the rewritten
    /// graph so regressions in `gen_slot_cpp` wiring or port plumbing show
    /// up as diff noise on this test.
    #[test]
    #[allow(clippy::too_many_lines, reason = "snapshot test for slot C++ wrapper")]
    fn test_floorplan_emits_slot_cpp_wrapper() {
        let graph = graph_from_value(json!({
            "top": "VecAdd",
            "tasks": {
                "VecAdd": {
                    "code": "extern \"C\" {\nvoid VecAdd(uint64_t n);\n}  // extern \"C\"\n\nextern \"C\" {\nvoid VecAdd(uint64_t n) { /* top body */ }\n}  // extern \"C\"\n",
                    "level": "upper",
                    "target": "hls",
                    "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "A": [{"args": {
                            "n": {"arg": "n", "cat": "scalar"},
                            "out": {"arg": "fifo", "cat": "ostream"}
                        }, "step": 0}],
                        "B": [{"args": {
                            "n": {"arg": "n", "cat": "scalar"},
                            "in": {"arg": "fifo", "cat": "istream"}
                        }, "step": 0}]
                    },
                    "fifos": {
                        "fifo": {
                            "depth": 2,
                            "consumed_by": ["B", 0],
                            "produced_by": ["A", 0]
                        }
                    }
                },
                "A": {
                    "code": "void A() {}",
                    "level": "lower",
                    "target": "hls",
                    "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64},
                        {"cat": "ostream", "name": "out", "type": "float", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                },
                "B": {
                    "code": "void B() {}",
                    "level": "lower",
                    "target": "hls",
                    "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64},
                        {"cat": "istream", "name": "in", "type": "float", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }));

        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert("SLOT_X0Y0".to_owned(), vec!["A_0".to_owned()]);
        slot_to_insts.insert("SLOT_X0Y1".to_owned(), vec!["B_0".to_owned()]);
        let result = get_floorplan_graph(&graph, &slot_to_insts).expect("apply");

        // --- Slot wrapper code snapshot ---------------------------------
        let slot_a = &result.tasks["SLOT_X0Y0"];
        let code_a = &slot_a.code;
        assert!(
            code_a.contains("void SLOT_X0Y0("),
            "slot wrapper must declare SLOT_X0Y0 fn; got:\n{code_a}",
        );
        assert!(
            code_a.contains("uint64_t n"),
            "slot wrapper must forward the scalar `n`; got:\n{code_a}",
        );
        assert!(
            code_a.contains("tapa::ostream<float>&"),
            "slot A must expose an ostream port (FIFO into SLOT_X0Y1); got:\n{code_a}",
        );
        assert!(
            code_a.contains("#pragma HLS interface ap_fifo"),
            "slot wrapper must stamp HLS fifo pragma; got:\n{code_a}",
        );
        assert!(
            !code_a.contains("TODO(rust-port)"),
            "slot code must no longer carry the TODO placeholder; got:\n{code_a}",
        );

        let slot_b = &result.tasks["SLOT_X0Y1"];
        let code_b = &slot_b.code;
        assert!(
            code_b.contains("void SLOT_X0Y1("),
            "slot wrapper must declare SLOT_X0Y1 fn; got:\n{code_b}",
        );
        assert!(
            code_b.contains("tapa::istream<float>&"),
            "slot B must expose an istream port (FIFO out of SLOT_X0Y0); got:\n{code_b}",
        );

        // --- Port-type plumbing snapshot --------------------------------
        // Regression: pre-fix, FIFO/mmap ports were emitted with empty
        // `ctype` + `width=0`, which makes HLS wrappers uncompilable.
        let slot_a_ports = &slot_a.ports;
        let fifo_port = slot_a_ports
            .iter()
            .find(|p| p.name == "fifo")
            .expect("slot A must carry the bridged FIFO port");
        assert_eq!(
            fifo_port.ctype, "float",
            "FIFO port type must come from A.out"
        );
        assert_eq!(fifo_port.width, 32, "FIFO port width must come from A.out");

        // --- Design.json snapshot via serde round-trip ------------------
        assert!(
            result.tasks.contains_key("SLOT_X0Y0"),
            "rewritten graph must carry SLOT_X0Y0",
        );
        assert!(
            result.tasks.contains_key("SLOT_X0Y1"),
            "rewritten graph must carry SLOT_X0Y1",
        );
        let top_tasks = &result.tasks["VecAdd"].tasks;
        assert!(
            top_tasks.contains_key("SLOT_X0Y0") && top_tasks.contains_key("SLOT_X0Y1"),
            "top must instantiate both slots; got keys: {:?}",
            top_tasks.keys().collect::<Vec<_>>(),
        );
    }

    #[test]
    fn region_format_helpers() {
        assert_eq!(
            region_to_slot_name("SLOT_X0Y0:SLOT_X0Y1"),
            "SLOT_X0Y0_SLOT_X0Y1",
        );
        assert_eq!(
            convert_region_format("SLOT_X0Y0:SLOT_X0Y1"),
            "SLOT_X0Y0_TO_SLOT_X0Y1",
        );
        assert_eq!(convert_region_format("solo"), "solo");
    }

    #[test]
    fn test_floorplan_empty_slot_rejected() {
        let graph = graph_from_value(sample_graph());
        let slot_to_insts = BTreeMap::from([("EMPTY_SLOT".to_owned(), vec![])]);

        let err = get_floorplan_graph(&graph, &slot_to_insts).expect_err("must reject");
        assert!(
            matches!(
                err,
                SlottingError::EmptyFloorplanSlot(ref name) if name == "EMPTY_SLOT"
            ),
            "expected EmptyFloorplanSlot, got: {err:?}"
        );
    }

    /// Malformed graph JSON should surface a precise typed error via
    /// `serde_path_to_error`.
    #[test]
    fn test_malformed_graph_rejects_missing_tasks() {
        let json = r#"{"top": "T"}"#;
        let err = Graph::from_json(json).expect_err("must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("tasks"),
            "error should mention tasks path, got: {msg}"
        );
    }

    #[test]
    fn test_malformed_graph_rejects_bad_level() {
        let json = r#"{"top": "T", "tasks": {"T": {"level": "unknown", "code": "", "target": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#;
        let err = Graph::from_json(json).expect_err("must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("level") || msg.contains("unknown"),
            "error should mention level, got: {msg}"
        );
    }
}
