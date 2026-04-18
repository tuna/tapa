//! Floorplan graph rewrite: group task instances into slots.
//!
//! Ports `tapa/common/graph.py`: `get_floorplan_slot`, `get_floorplan_top`,
//! `get_floorplan_graph`, and their helper functions.
//!
//! Operates on `serde_json::Value` to match the Python dict manipulation
//! pattern, since graph.json has many fields that need flexible access.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::error::SlottingError;

/// Safe conversion of JSON u64 index to usize.
fn json_idx(val: u64) -> usize {
    usize::try_from(val).unwrap_or(0)
}

/// Generate a floorplanned graph by grouping instances into slots.
///
/// Takes the original graph JSON and a mapping from slot names to
/// the list of instance names assigned to each slot.
///
/// Returns the modified graph JSON with new slot task definitions
/// and a rewritten top-level task that instantiates slots.
pub fn get_floorplan_graph(
    graph: &Value,
    slot_to_insts: &BTreeMap<String, Vec<String>>,
) -> Result<Value, SlottingError> {
    let mut new_graph = graph.clone();
    let tasks = new_graph["tasks"]
        .as_object_mut()
        .ok_or_else(|| SlottingError::TreeSitter("graph missing 'tasks' field".into()))?;

    let top_name = graph["top"]
        .as_str()
        .ok_or_else(|| SlottingError::TreeSitter("graph missing 'top' field".into()))?
        .to_owned();

    // Build slot definitions
    let mut slot_defs: BTreeMap<String, Value> = BTreeMap::new();
    for (slot_name, insts) in slot_to_insts {
        let slot_def = build_floorplan_slot(graph, slot_name, insts, &top_name)?;
        tasks.insert(slot_name.clone(), slot_def.clone());
        slot_defs.insert(slot_name.clone(), slot_def);
    }

    // Build inst->slot mapping
    let inst_to_slot: BTreeMap<String, String> = slot_to_insts
        .iter()
        .flat_map(|(slot, insts)| insts.iter().map(move |inst| (inst.clone(), slot.clone())))
        .collect();

    // Rewrite top task
    let top_task = build_floorplan_top(graph, &slot_defs, &inst_to_slot, &top_name);
    tasks.insert(top_name, top_task);

    Ok(new_graph)
}

/// Build a slot task definition grouping the specified instances.
fn build_floorplan_slot(
    graph: &Value,
    slot_name: &str,
    task_inst_in_slot: &[String],
    top_name: &str,
) -> Result<Value, SlottingError> {
    let top_task = &graph["tasks"][top_name];
    let mut new_obj = top_task.clone();
    new_obj["level"] = json!("upper");

    let top_tasks = top_task["tasks"]
        .as_object()
        .ok_or_else(|| SlottingError::TreeSitter("top task missing 'tasks' field".into()))?;

    // Build instance mapping: top_index -> slot_index
    // Also track original top-level instance names for mmap port inference
    let mut new_tasks: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut top_to_slot_idx: BTreeMap<String, BTreeMap<usize, usize>> = BTreeMap::new();
    let mut original_inst_names: Vec<String> = Vec::new(); // original top-level names
    let inst_set: BTreeSet<&str> = task_inst_in_slot.iter().map(String::as_str).collect();

    for (task_name, insts_val) in top_tasks {
        let insts = insts_val.as_array().unwrap_or(&Vec::new()).clone();
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
            new_tasks.entry(task_name.clone()).or_default().push(new_inst);
            original_inst_names.push(inst_name);
        }
    }

    new_obj["tasks"] = json!(new_tasks);

    // Rewrite FIFOs
    let top_fifos = top_task["fifos"].as_object().cloned().unwrap_or_default();
    let (new_fifos, fifo_ports) =
        get_slot_fifos(&top_fifos, &top_to_slot_idx, &inst_set);
    new_obj["fifos"] = json!(new_fifos);

    // Build ports: scalar args + FIFO-connected ports + inferred mmap ports
    let mut new_ports: Vec<Value> = Vec::new();

    // Collect scalar args from instances in slot
    let mut scalar_args: BTreeSet<String> = BTreeSet::new();
    for insts in new_tasks.values() {
        for inst in insts {
            if let Some(args) = inst["args"].as_object() {
                for arg in args.values() {
                    if arg["cat"].as_str() == Some("scalar") {
                        if let Some(name) = arg["arg"].as_str() {
                            scalar_args.insert(name.to_owned());
                        }
                    }
                }
            }
        }
    }

    // Keep scalar ports from top task
    if let Some(ports) = top_task["ports"].as_array() {
        for port in ports {
            if let Some(name) = port["name"].as_str() {
                if scalar_args.contains(name) {
                    new_ports.push(port.clone());
                }
            }
        }
    }

    // Add FIFO-connected ports
    new_ports.extend(get_used_ports(graph, top_name, &new_tasks, &fifo_ports));

    // Add inferred mmap ports (using original top-level instance names)
    new_ports.extend(infer_mmap_ports_from_subtasks(graph, &new_tasks, &original_inst_names));

    new_obj["ports"] = json!(new_ports);

    // Generate slot C++ using gen_slot_cpp
    let top_code = top_task["code"].as_str().unwrap_or("");
    let top_task_name = top_name;
    let slot_ports: Vec<crate::SlotPort> = new_ports
        .iter()
        .filter_map(|p| {
            Some(crate::SlotPort {
                cat: p["cat"].as_str()?.to_owned(),
                name: p["name"].as_str()?.to_owned(),
                port_type: p["type"].as_str()?.to_owned(),
            })
        })
        .collect();

    let new_code = crate::gen_slot_cpp(slot_name, top_task_name, &slot_ports, top_code)?;
    new_obj["code"] = json!(new_code);

    Ok(new_obj)
}

/// Build the rewritten top-level task that instantiates slots.
fn build_floorplan_top(
    graph: &Value,
    slot_defs: &BTreeMap<String, Value>,
    inst_to_slot: &BTreeMap<String, String>,
    top_name: &str,
) -> Value {
    let top_task = &graph["tasks"][top_name];
    let mut new_top = top_task.clone();
    let top_tasks = top_task["tasks"].as_object().cloned().unwrap_or_default();

    // Build slot instances
    let new_insts = build_top_slot_insts(slot_defs, &top_tasks, inst_to_slot);
    new_top["tasks"] = json!(new_insts);

    // Collect in-slot internal FIFOs to exclude from top
    let mut in_slot_fifos: BTreeSet<String> = BTreeSet::new();
    for slot_def in slot_defs.values() {
        if let Some(fifos) = slot_def["fifos"].as_object() {
            for (name, fifo) in fifos {
                if fifo.get("depth").is_some() {
                    in_slot_fifos.insert(name.clone());
                }
            }
        }
    }

    // Update cross-slot FIFOs
    let top_fifos = top_task["fifos"].as_object().cloned().unwrap_or_default();
    let new_fifos = update_cross_slot_fifos(&top_fifos, &in_slot_fifos, inst_to_slot);
    new_top["fifos"] = json!(new_fifos);

    new_top
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Get instance name from task name and index: `{task}_{idx}`.
fn get_instance_name(task_name: &str, idx: usize) -> String {
    format!("{task_name}_{idx}")
}

/// Connect mmap args in a sub-instance to slot-level port names.
fn connect_subinst_mmap_to_slot_port(inst: &Value, inst_name: &str) -> Value {
    let mut new_inst = inst.clone();
    if let Some(args) = inst["args"].as_object() {
        let mut new_args = Map::new();
        for (port_name, arg) in args {
            let cat = arg["cat"].as_str().unwrap_or("");
            if cat == "mmap" || cat == "async_mmap" {
                new_args.insert(
                    port_name.clone(),
                    json!({
                        "arg": format!("{port_name}_{inst_name}"),
                        "cat": cat,
                    }),
                );
            } else {
                new_args.insert(port_name.clone(), arg.clone());
            }
        }
        new_inst["args"] = Value::Object(new_args);
    }
    new_inst
}

/// Get slot FIFOs: internal FIFOs stay, cross-slot become external.
fn get_slot_fifos(
    top_fifos: &Map<String, Value>,
    top_to_slot_idx: &BTreeMap<String, BTreeMap<usize, usize>>,
    inst_set: &BTreeSet<&str>,
) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut new_fifos: BTreeMap<String, Value> = BTreeMap::new();
    let mut fifo_ports: Vec<String> = Vec::new();

    for (name, fifo) in top_fifos {
        // Skip external FIFOs (no depth)
        if fifo.get("depth").is_none() {
            continue;
        }

        let src = fifo.get("consumed_by");
        let dst = fifo.get("produced_by");
        let src_in_slot = endpoint_in_set(src, inst_set);
        let dst_in_slot = endpoint_in_set(dst, inst_set);

        if src_in_slot && dst_in_slot {
            // Internal: update indices
            let updated = update_fifo_inst_idx(fifo, top_to_slot_idx);
            new_fifos.insert(name.clone(), updated);
        } else if src_in_slot {
            // Consumer in slot, producer outside -> external
            let updated_src = update_endpoint_idx(src, top_to_slot_idx);
            new_fifos.insert(name.clone(), json!({"consumed_by": updated_src}));
            fifo_ports.push(name.clone());
        } else if dst_in_slot {
            // Producer in slot, consumer outside -> external
            let updated_dst = update_endpoint_idx(dst, top_to_slot_idx);
            new_fifos.insert(name.clone(), json!({"produced_by": updated_dst}));
            fifo_ports.push(name.clone());
        }
    }

    (new_fifos, fifo_ports)
}

/// Check if a FIFO endpoint is in the instance set.
fn endpoint_in_set(endpoint: Option<&Value>, inst_set: &BTreeSet<&str>) -> bool {
    endpoint.is_some_and(|ep| {
        if let Some(arr) = ep.as_array() {
            if arr.len() >= 2 {
                let name = arr[0].as_str().unwrap_or("");
                let idx = arr[1].as_u64().map_or(0, json_idx);
                return inst_set.contains(get_instance_name(name, idx).as_str());
            }
        }
        false
    })
}

/// Update FIFO endpoint indices from top to slot.
fn update_endpoint_idx(
    endpoint: Option<&Value>,
    idx_map: &BTreeMap<String, BTreeMap<usize, usize>>,
) -> Value {
    if let Some(ep) = endpoint {
        if let Some(arr) = ep.as_array() {
            if arr.len() >= 2 {
                let name = arr[0].as_str().unwrap_or("");
                let top_idx = arr[1].as_u64().map_or(0, json_idx);
                if let Some(slot_idx) = idx_map.get(name).and_then(|m| m.get(&top_idx)) {
                    return json!([name, *slot_idx]);
                }
            }
        }
    }
    json!(null)
}

/// Update FIFO instance indices from top to slot.
fn update_fifo_inst_idx(
    fifo: &Value,
    idx_map: &BTreeMap<String, BTreeMap<usize, usize>>,
) -> Value {
    let mut result = fifo.clone();
    if let Some(consumed) = fifo.get("consumed_by") {
        result["consumed_by"] = update_endpoint_idx(Some(consumed), idx_map);
    }
    if let Some(produced) = fifo.get("produced_by") {
        result["produced_by"] = update_endpoint_idx(Some(produced), idx_map);
    }
    result
}

/// Find ports connected to FIFO endpoints.
fn get_used_ports(
    graph: &Value,
    _top_name: &str,
    new_tasks: &BTreeMap<String, Vec<Value>>,
    fifo_ports: &[String],
) -> Vec<Value> {
    let fifo_set: BTreeSet<&str> = fifo_ports.iter().map(String::as_str).collect();
    let mut new_ports = Vec::new();

    for (task_name, insts) in new_tasks {
        let task_ports = &graph["tasks"][task_name.as_str()]["ports"];
        for inst in insts {
            if let Some(args) = inst["args"].as_object() {
                for arg in args.values() {
                    let cat = arg["cat"].as_str().unwrap_or("");
                    if cat == "mmap" || cat == "async_mmap" {
                        continue;
                    }
                    let arg_name = arg["arg"].as_str().unwrap_or("");
                    if !fifo_set.contains(arg_name) {
                        continue;
                    }
                    // Find matching port in task definition
                    if let Some(ports) = task_ports.as_array() {
                        for port in ports {
                            if port["name"].as_str() == Some(arg_name)
                                || port["name"].as_str().is_some_and(|n| {
                                    // Match without array index
                                    arg_name.starts_with(n)
                                })
                            {
                                let mut new_port = port.clone();
                                new_port["name"] = json!(arg_name);
                                new_ports.push(new_port);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Deduplicate by name
    let mut seen = BTreeSet::new();
    new_ports.retain(|p| {
        let name = p["name"].as_str().unwrap_or("").to_owned();
        seen.insert(name)
    });

    new_ports
}

/// Infer mmap ports from child instance definitions.
///
/// Uses the original top-level instance names (not slot-local indices)
/// to match the port names created by `connect_subinst_mmap_to_slot_port`.
fn infer_mmap_ports_from_subtasks(
    graph: &Value,
    new_tasks: &BTreeMap<String, Vec<Value>>,
    original_inst_names: &[String],
) -> Vec<Value> {
    let mut ports = Vec::new();
    let mut name_iter = original_inst_names.iter();
    for (task_name, insts) in new_tasks {
        let task_ports = &graph["tasks"][task_name.as_str()]["ports"];
        if let Some(port_array) = task_ports.as_array() {
            for _inst in insts {
                // Use original top-level instance name, NOT slot-local index
                let inst_name = match name_iter.next() {
                    Some(name) => name.clone(),
                    None => task_name.clone(),
                };
                for port in port_array {
                    let cat = port["cat"].as_str().unwrap_or("");
                    if cat == "mmap" || cat == "async_mmap" {
                        let port_name = port["name"].as_str().unwrap_or("");
                        ports.push(json!({
                            "cat": cat,
                            "name": format!("{port_name}_{inst_name}"),
                            "type": port["type"],
                            "width": port["width"],
                        }));
                    }
                }
            }
        }
    }
    ports
}

/// Build slot instances for the rewritten top task.
fn build_top_slot_insts(
    slot_defs: &BTreeMap<String, Value>,
    top_tasks: &Map<String, Value>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<Value>> {
    let mut new_top_insts: BTreeMap<String, Vec<Value>> = BTreeMap::new();

    for (slot_name, slot_def) in slot_defs {
        let slot_ports: BTreeMap<String, Value> = slot_def["ports"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        let name = p["name"].as_str()?.to_owned();
                        Some((name, p.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let slot_subtasks = slot_def["tasks"].as_object();

        let mut args = Map::new();
        for (port_name, port) in &slot_ports {
            let cat = port["cat"].as_str().unwrap_or("");
            if cat == "mmap" || cat == "hmap" || cat == "async_mmap" {
                continue;
            }
            let formatted = port_name.replace('[', "_").replace(']', "");
            let empty_map = Map::new();
            let inferred_cat =
                infer_arg_cat_from_subinst(port_name, slot_subtasks.unwrap_or(&empty_map));
            args.insert(
                formatted,
                json!({
                    "arg": port_name,
                    "cat": inferred_cat,
                }),
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
            .push(json!({"args": args, "step": 0}));
    }

    new_top_insts
}

/// Infer port category from child instances.
fn infer_arg_cat_from_subinst(port_name: &str, tasks: &Map<String, Value>) -> String {
    for (_task_name, insts_val) in tasks {
        if let Some(insts) = insts_val.as_array() {
            for inst in insts {
                if let Some(args) = inst["args"].as_object() {
                    for arg in args.values() {
                        if arg["arg"].as_str() == Some(port_name) {
                            return arg["cat"].as_str().unwrap_or("scalar").to_owned();
                        }
                    }
                }
            }
        }
    }
    "scalar".to_owned()
}

/// Update cross-slot FIFOs: remap endpoints to slot instances.
fn update_cross_slot_fifos(
    top_fifos: &Map<String, Value>,
    in_slot_fifos: &BTreeSet<String>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, Value> {
    let mut new_fifos = BTreeMap::new();
    for (name, fifo) in top_fifos {
        if in_slot_fifos.contains(name) {
            continue;
        }
        let mut updated = fifo.clone();
        if let Some(consumed) = fifo.get("consumed_by") {
            updated["consumed_by"] = remap_endpoint_to_slot(consumed, inst_to_slot);
        }
        if let Some(produced) = fifo.get("produced_by") {
            updated["produced_by"] = remap_endpoint_to_slot(produced, inst_to_slot);
        }
        new_fifos.insert(name.clone(), updated);
    }
    new_fifos
}

/// Remap a FIFO endpoint from (task, idx) to (slot, 0).
fn remap_endpoint_to_slot(
    endpoint: &Value,
    inst_to_slot: &BTreeMap<String, String>,
) -> Value {
    if let Some(arr) = endpoint.as_array() {
        if arr.len() >= 2 {
            let task_name = arr[0].as_str().unwrap_or("");
            let idx = arr[1].as_u64().map_or(0, json_idx);
            let inst_name = get_instance_name(task_name, idx);
            if let Some(slot) = inst_to_slot.get(&inst_name) {
                return json!([slot, 0]);
            }
        }
    }
    endpoint.clone()
}

/// Get mmap port args for a slot instance in the rewritten top.
fn get_slot_inst_mmap_port_args(
    slot_name: &str,
    top_tasks: &Map<String, Value>,
    inst_to_slot: &BTreeMap<String, String>,
) -> BTreeMap<String, Value> {
    let mut args = BTreeMap::new();
    for (task_name, insts_val) in top_tasks {
        if let Some(insts) = insts_val.as_array() {
            for (idx, inst) in insts.iter().enumerate() {
                let inst_name = get_instance_name(task_name, idx);
                if inst_to_slot.get(&inst_name).map(String::as_str) != Some(slot_name) {
                    continue;
                }
                if let Some(inst_args) = inst["args"].as_object() {
                    for (port_name, arg) in inst_args {
                        let cat = arg["cat"].as_str().unwrap_or("");
                        if cat == "mmap" || cat == "async_mmap" {
                            let slot_port_name = format!("{port_name}_{inst_name}");
                            args.insert(
                                slot_port_name,
                                json!({
                                    "arg": arg["arg"],
                                    "cat": cat,
                                }),
                            );
                        }
                    }
                }
            }
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> Value {
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
        let graph = sample_graph();
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();

        // Should have the slot task
        assert!(
            result["tasks"]["SLOT_X0Y0_TO_SLOT_X1Y1"].is_object(),
            "slot task should exist"
        );
        // Slot task should be upper level
        assert_eq!(
            result["tasks"]["SLOT_X0Y0_TO_SLOT_X1Y1"]["level"],
            "upper"
        );
    }

    #[test]
    fn floorplan_graph_internal_fifo_stays() {
        let graph = sample_graph();
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot_fifos = &result["tasks"]["SLOT_X0Y0_TO_SLOT_X1Y1"]["fifos"];
        // fifo_0 should stay internal (both endpoints in slot)
        assert!(
            slot_fifos["fifo_0"].is_object(),
            "internal FIFO should be preserved in slot"
        );
        assert!(
            slot_fifos["fifo_0"]["depth"].is_number(),
            "internal FIFO should keep depth"
        );
    }

    #[test]
    fn floorplan_graph_top_task_rewritten() {
        let graph = sample_graph();
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let top = &result["tasks"]["top_func"];

        // Top task should now instantiate the slot
        assert!(
            top["tasks"]["SLOT_X0Y0_TO_SLOT_X1Y1"].is_array(),
            "top should instantiate slot, got: {}",
            serde_json::to_string_pretty(top).unwrap()
        );
    }

    #[test]
    fn instance_name_format() {
        assert_eq!(get_instance_name("producer", 0), "producer_0");
        assert_eq!(get_instance_name("task", 5), "task_5");
    }

    #[test]
    fn floorplan_mmap_ports_use_original_instance_names() {
        // Test with nonzero instance index and mmap ports
        let graph = json!({
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
        });

        // Put only worker_1 (not worker_0) in the slot
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["worker_1".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot = &result["tasks"]["SLOT_X0Y0_TO_SLOT_X1Y1"];
        let slot_ports = slot["ports"].as_array().unwrap();

        // The mmap port should use the original instance name "worker_1",
        // NOT the slot-local index "worker_0"
        let mmap_ports: Vec<&str> = slot_ports
            .iter()
            .filter(|p| p["cat"].as_str() == Some("mmap"))
            .filter_map(|p| p["name"].as_str())
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

    /// Build a graph with two slots where a FIFO crosses the slot boundary.
    /// The cross-slot FIFO endpoints in the rewritten top should reference
    /// slot names, not the original task names.
    #[test]
    fn test_floorplan_cross_slot_fifo_remapping() {
        let graph = json!({
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
        });

        // Put producer and consumer in different slots
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_A".to_owned(),
            vec!["producer_0".to_owned()],
        );
        slot_to_insts.insert(
            "SLOT_B".to_owned(),
            vec!["consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let top = &result["tasks"]["top_func"];
        let top_fifos = top["fifos"].as_object().expect("top should have fifos");

        // The cross-slot FIFO should still exist in top-level
        assert!(
            top_fifos.contains_key("cross_fifo"),
            "cross-slot FIFO should remain in top, got keys: {:?}",
            top_fifos.keys().collect::<Vec<_>>()
        );

        let cross = &top_fifos["cross_fifo"];

        // Endpoints should reference slot names, not original task names
        let consumed = cross["consumed_by"].as_array().expect("consumed_by should be array");
        let produced = cross["produced_by"].as_array().expect("produced_by should be array");

        assert_eq!(
            consumed[0].as_str().unwrap(), "SLOT_B",
            "consumed_by should reference SLOT_B, got: {consumed:?}"
        );
        assert_eq!(
            produced[0].as_str().unwrap(), "SLOT_A",
            "produced_by should reference SLOT_A, got: {produced:?}"
        );

        // Neither slot should contain the cross-slot FIFO as internal (with depth)
        let fifos_a = result["tasks"]["SLOT_A"]["fifos"].as_object();
        let fifos_b = result["tasks"]["SLOT_B"]["fifos"].as_object();

        // SLOT_A has the producer side (produced_by endpoint); it should NOT have depth
        if let Some(fifos) = fifos_a {
            if let Some(fifo) = fifos.get("cross_fifo") {
                assert!(
                    fifo.get("depth").is_none(),
                    "cross-slot FIFO in SLOT_A should not have depth"
                );
            }
        }
        if let Some(fifos) = fifos_b {
            if let Some(fifo) = fifos.get("cross_fifo") {
                assert!(
                    fifo.get("depth").is_none(),
                    "cross-slot FIFO in SLOT_B should not have depth"
                );
            }
        }
    }

    /// Verify that multiple slots are created and the top task instantiates both.
    #[test]
    #[allow(clippy::too_many_lines, reason = "complex test fixture setup")]
    fn test_floorplan_multiple_slots() {
        let graph = json!({
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
        });

        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_LEFT".to_owned(),
            vec!["task_a_0".to_owned(), "task_b_0".to_owned()],
        );
        slot_to_insts.insert(
            "SLOT_RIGHT".to_owned(),
            vec!["task_c_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();

        // Both slot tasks should exist
        assert!(
            result["tasks"]["SLOT_LEFT"].is_object(),
            "SLOT_LEFT task should exist"
        );
        assert!(
            result["tasks"]["SLOT_RIGHT"].is_object(),
            "SLOT_RIGHT task should exist"
        );

        // Both should be upper level
        assert_eq!(result["tasks"]["SLOT_LEFT"]["level"], "upper", "SLOT_LEFT should be upper");
        assert_eq!(result["tasks"]["SLOT_RIGHT"]["level"], "upper", "SLOT_RIGHT should be upper");

        // Top task should instantiate both slots
        let top = &result["tasks"]["top_func"];
        assert!(
            top["tasks"]["SLOT_LEFT"].is_array(),
            "top should instantiate SLOT_LEFT"
        );
        assert!(
            top["tasks"]["SLOT_RIGHT"].is_array(),
            "top should instantiate SLOT_RIGHT"
        );

        // SLOT_LEFT should contain task_a and task_b
        let left_tasks = result["tasks"]["SLOT_LEFT"]["tasks"]
            .as_object()
            .expect("SLOT_LEFT should have tasks");
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
        let right_tasks = result["tasks"]["SLOT_RIGHT"]["tasks"]
            .as_object()
            .expect("SLOT_RIGHT should have tasks");
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
        let graph = sample_graph();
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "SLOT_X0Y0_TO_SLOT_X1Y1".to_owned(),
            vec!["producer_0".to_owned(), "consumer_0".to_owned()],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts).unwrap();
        let slot = &result["tasks"]["SLOT_X0Y0_TO_SLOT_X1Y1"];
        let slot_ports = slot["ports"]
            .as_array()
            .expect("slot should have ports");

        // The "size" scalar port is used by both producer and consumer
        let scalar_port_names: Vec<&str> = slot_ports
            .iter()
            .filter(|p| p["cat"].as_str() == Some("scalar"))
            .filter_map(|p| p["name"].as_str())
            .collect();

        assert!(
            scalar_port_names.contains(&"size"),
            "slot should preserve scalar port 'size', got: {scalar_port_names:?}"
        );

        // Verify the scalar port retains its type and width
        let size_port = slot_ports
            .iter()
            .find(|p| p["name"].as_str() == Some("size"))
            .expect("size port should exist");
        assert_eq!(
            size_port["type"].as_str().unwrap(), "int",
            "size port should retain type 'int'"
        );
        assert_eq!(
            size_port["width"].as_u64().unwrap(), 32,
            "size port should retain width 32"
        );
    }

    /// An empty slot instance list should produce an error or result in
    /// no meaningful slot task being created.
    #[test]
    fn test_floorplan_empty_slot_rejected() {
        let graph = sample_graph();
        let mut slot_to_insts = BTreeMap::new();
        slot_to_insts.insert(
            "EMPTY_SLOT".to_owned(),
            vec![],
        );

        let result = get_floorplan_graph(&graph, &slot_to_insts);

        match result {
            Err(_) => {
                // An error is acceptable for empty slots
            }
            Ok(value) => {
                // If no error, the empty slot should have no child tasks
                let slot = &value["tasks"]["EMPTY_SLOT"];
                let tasks = slot["tasks"].as_object();
                if let Some(t) = tasks {
                    assert!(
                        t.is_empty(),
                        "empty slot should have no child tasks, got: {:?}",
                        t.keys().collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}
