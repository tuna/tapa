//! Graph-dict to typed [`Design`] projection plus the
//! `--flatten-hierarchy` round-trip helper for `tapa analyze`.
//!
//! Mirrors the Python `Task.to_topology_dict` projection but drops
//! `vendor` and other tapacc-only keys, and provides
//! [`flatten_graph_value`] which round-trips a tapacc graph dict
//! through the typed [`Graph`] schema and re-serializes the result of
//! [`flatten`].

use indexmap::IndexMap;
use serde_json::Value;
use tapa_task_graph::{flatten, Design, Graph, TaskTopology, TransformError};

use crate::error::{CliError, Result};
use crate::state::value_to_indexmap;

/// Round-trip a tapacc graph dict through the typed [`Graph`] schema and
/// return the result of [`flatten`] re-serialized as `serde_json::Value`.
///
/// The CLI keeps the on-disk graph as a `Value` because the legacy
/// Python pipeline accepts a richer schema in some downstream stages,
/// but the transform itself is defined on the strict `Graph` type to
/// maximize Python parity.
pub(super) fn flatten_graph_value(graph: &Value) -> Result<Value> {
    let json = serde_json::to_string(graph)?;
    let typed = Graph::from_json(&json)?;
    let flat = flatten(&typed).map_err(|e| match e {
        TransformError::DeepHierarchyNotSupported(child) => CliError::InvalidArg(format!(
            "`--flatten-hierarchy` only supports single-level hierarchies for now; \
             child task `{child}` is itself an upper task. The native port covers \
             the vadd-shaped case; deeper graphs are pending.",
        )),
        other @ (TransformError::MissingTop(_)
        | TransformError::TopIsLeaf(_)
        | TransformError::UnknownFloorplanInstance(_)
        | TransformError::UnknownChildTask(_)
        | TransformError::SlotNameCollision(_)
        | TransformError::SlotCppGeneration { .. }
        | TransformError::Json(_)) => {
            CliError::InvalidArg(format!("flatten failed: {other}"))
        }
    })?;
    let out_json = flat.to_json()?;
    let value: Value = serde_json::from_str(&out_json)?;
    Ok(value)
}

/// True when the top task in `graph` is a leaf-level task.
pub(super) fn is_top_leaf(graph: &Value, top: &str) -> bool {
    graph
        .get("tasks")
        .and_then(|t| t.get(top))
        .and_then(|task| task.get("level"))
        .and_then(Value::as_str)
        .is_some_and(|level| level == "lower")
}

/// Project the tapacc graph dict into a typed [`Design`] suitable for
/// `<work_dir>/design.json`. Mirrors the Python `Task.to_topology_dict`
/// projection, but drops `vendor` and other tapacc-only keys.
pub(super) fn build_design(top: &str, target: &str, graph: &Value) -> Result<Design> {
    let tasks_obj = graph
        .get("tasks")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::InvalidArg(
            "tapacc graph is missing the `tasks` object".to_string(),
        ))?;

    let mut topology: IndexMap<String, TaskTopology> = IndexMap::new();
    for (name, task) in tasks_obj {
        topology.insert(name.clone(), task_to_topology(name, task));
    }

    Ok(Design {
        top: top.to_string(),
        target: target.to_string(),
        tasks: topology,
        slot_task_name_to_fp_region: None,
    })
}

fn task_to_topology(name: &str, task: &Value) -> TaskTopology {
    let level = task
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("lower")
        .to_string();
    let code = task
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let ports = task
        .get("ports")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|p| serde_json::from_value(p.clone()).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tasks = value_to_indexmap(task.get("tasks"));
    let fifos = value_to_indexmap(task.get("fifos"));
    let target = task
        .get("target")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    TaskTopology {
        name: name.to_string(),
        level,
        code,
        ports,
        tasks,
        fifos,
        target,
        is_slot: false,
        self_area: IndexMap::new(),
        total_area: IndexMap::new(),
        clock_period: "0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn is_top_leaf_detects_lower_level() {
        let g = json!({"tasks": {"T": {"level": "lower"}}, "top": "T"});
        assert!(is_top_leaf(&g, "T"));
        let g = json!({"tasks": {"T": {"level": "upper"}}, "top": "T"});
        assert!(!is_top_leaf(&g, "T"));
        // Missing top is treated as upper for safety.
        assert!(!is_top_leaf(&g, "DoesNotExist"));
    }

    /// `analyze --flatten-hierarchy` exercises the
    /// [`tapa_task_graph::flatten`] code path on a vadd-shaped graph.
    /// We hit `flatten_graph_value` directly (the helper invoked from
    /// `run_native` when `flatten_hierarchy` is set) because the full
    /// `run_native` path depends on a process-wide `OnceLock` for the
    /// `find_resource` search anchor — sharing that across tests would
    /// require more invasive plumbing than this transform-coverage
    /// check warrants.
    #[test]
    fn flatten_graph_value_renames_fifos_for_vadd_shape() {
        let raw = json!({
            "cflags": [],
            "top": "VecAdd",
            "tasks": {
                "VecAdd": {
                    "code": "void VecAdd() {}",
                    "level": "upper",
                    "target": "hls",
                    "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64}
                    ],
                    "tasks": {
                        "A": [{"step": 0, "args": {
                            "n": {"arg": "n", "cat": "scalar"},
                            "out": {"arg": "fifo", "cat": "ostream"}
                        }}],
                        "B": [{"step": 0, "args": {
                            "n": {"arg": "n", "cat": "scalar"},
                            "in": {"arg": "fifo", "cat": "istream"}
                        }}]
                    },
                    "fifos": {
                        "fifo": {"depth": 2, "consumed_by": ["B", 0],
                                 "produced_by": ["A", 0]}
                    }
                },
                "A": {
                    "code": "void A() {}", "level": "lower",
                    "target": "hls", "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64},
                        {"cat": "ostream", "name": "out",
                         "type": "float", "width": 32}
                    ]
                },
                "B": {
                    "code": "void B() {}", "level": "lower",
                    "target": "hls", "vendor": "xilinx",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64},
                        {"cat": "istream", "name": "in",
                         "type": "float", "width": 32}
                    ]
                }
            }
        });

        let out = flatten_graph_value(&raw).expect("flatten ok");
        let top = out["tasks"]["VecAdd"]
            .as_object()
            .expect("top survives");
        assert!(
            top["fifos"].get("fifo_VecAdd").is_some(),
            "flatten must rename `fifo` to `fifo_VecAdd`; got {top:?}",
        );
        let a0 = &top["tasks"]["A"][0]["args"]["out"]["arg"];
        assert_eq!(a0, &json!("fifo_VecAdd"));
    }

    /// Regression: nested upper children used to surface
    /// `DeepHierarchyNotSupported`. Python's
    /// `Graph.get_flatten_graph` recursively collects every leaf
    /// under the top, so the Rust port now mirrors that — a deeply
    /// nested design must round-trip cleanly (no panic, no typed
    /// `InvalidArg`). For the minimal fixture below, Inner has no
    /// tasks of its own, so the flattened top's `tasks` map is
    /// empty; the important contract is that `flatten_graph_value`
    /// returns `Ok`.
    #[test]
    fn flatten_graph_value_accepts_nested_upper() {
        let raw = json!({
            "cflags": [],
            "top": "Outer",
            "tasks": {
                "Outer": {
                    "code": "", "level": "upper", "target": "hls",
                    "vendor": "xilinx", "ports": [],
                    "tasks": {"Inner": [{"args": {}, "step": 0}]},
                    "fifos": {}
                },
                "Inner": {
                    "code": "", "level": "upper", "target": "hls",
                    "vendor": "xilinx", "ports": [],
                    "tasks": {}, "fifos": {}
                }
            }
        });
        let out = flatten_graph_value(&raw).expect("recursive flatten ok");
        let top = out["tasks"]["Outer"].as_object().expect("top survives");
        assert!(
            top.get("tasks").and_then(|v| v.as_object()).is_some(),
            "top task must keep a `tasks` map after flatten: {top:?}",
        );
    }
}
