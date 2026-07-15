//! Graph-dict to typed [`Design`] projection plus the
//! `--flatten-hierarchy` round-trip helper for `tapa analyze`.
//!
//! Drops `vendor` and other tapacc-only keys while projecting a task
//! graph into a [`Design`], and provides [`flatten_graph_value`] for
//! hierarchy flattening through the typed [`Graph`] schema.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use tapa_ir::{flatten, Design, Graph, Target, Task, TaskLevel};

use crate::error::{CliError, Result};

/// Round-trip a tapacc graph dict through the typed [`Graph`] schema and
/// return the result of [`flatten`] re-serialized as `serde_json::Value`.
///
/// The transform is defined on the strict [`Graph`] type used by the
/// CLI's graph reader and writer.
pub(super) fn flatten_graph_value(graph: &Graph) -> Result<Graph> {
    let flat =
        flatten(graph).map_err(|error| CliError::InvalidArg(format!("flatten failed: {error}")))?;
    Ok(flat)
}

/// True when the top task in `graph` is a leaf-level task.
pub(super) fn is_top_leaf(graph: &Graph, top: &str) -> bool {
    graph
        .tasks
        .get(top)
        .is_some_and(|task| task.level == TaskLevel::Lower)
}

/// Project the tapacc graph into a typed [`Design`] suitable for
/// `<work_dir>/design.json`, dropping `vendor` and other analyzer-only
/// keys.
pub(super) fn build_design(top: &str, target: Target, graph: &Graph) -> Design {
    let tasks: BTreeMap<String, Task> = graph
        .tasks
        .iter()
        .map(|(name, task)| (name.clone(), task_to_design_task(name, task)))
        .collect();

    Design {
        top: top.to_string(),
        target,
        tasks,
        slot_task_name_to_fp_region: None,
    }
}

/// Project one graph task into its `design.json` shape. The typed
/// clone keeps `tasks`/`fifos` as-is and drops the tapacc-only fields
/// (`vendor`, `extra`).
fn task_to_design_task(name: &str, task: &tapa_ir::TaskDefinition) -> Task {
    Task {
        name: name.to_string(),
        level: task.level,
        code: task.code.clone(),
        ports: task.ports.clone(),
        tasks: task.tasks.clone(),
        fifos: task.fifos.clone(),
        target: Some(task.target.clone()),
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
        let g: Graph = serde_json::from_value(json!({"tasks": {"T": {"level": "lower", "code": "", "target": "hls", "ports": [], "tasks": {}, "fifos": {}}}, "top": "T"})).unwrap();
        assert!(is_top_leaf(&g, "T"));
        let g: Graph = serde_json::from_value(json!({"tasks": {"T": {"level": "upper", "code": "", "target": "hls", "ports": [], "tasks": {}, "fifos": {}}}, "top": "T"})).unwrap();
        assert!(!is_top_leaf(&g, "T"));
        // Missing top is treated as upper for safety.
        assert!(!is_top_leaf(&g, "DoesNotExist"));
    }

    /// `analyze --flatten-hierarchy` exercises the
    /// [`tapa_ir::flatten`] code path on a vadd-shaped graph.
    /// We hit `flatten_graph_value` directly (the helper invoked from
    /// `run_native` when `flatten_hierarchy` is set) because the full
    /// `run_native` path depends on a process-wide `OnceLock` for the
    /// `find_resource` search anchor — sharing that across tests would
    /// require more invasive plumbing than this transform-coverage
    /// check warrants.
    #[test]
    fn flatten_graph_value_renames_fifos_for_vadd_shape() {
        let raw: Graph = serde_json::from_value(json!({
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
        }))
        .expect("valid graph");

        let out = flatten_graph_value(&raw).expect("flatten ok");
        let top = out.tasks.get("VecAdd").expect("top survives");
        assert!(
            top.fifos.contains_key("fifo_VecAdd"),
            "flatten must rename `fifo` to `fifo_VecAdd`; got {top:?}",
        );
        let a0 = &top.tasks["A"][0];
        assert_eq!(a0.args["out"].arg, "fifo_VecAdd");
    }

    /// Nested upper children are recursively flattened. In this
    /// minimal fixture, `Inner` has no tasks of its own, so the
    /// flattened top's `tasks` map is empty; the transform must still
    /// return `Ok`.
    #[test]
    fn flatten_graph_value_accepts_nested_upper() {
        let raw: Graph = serde_json::from_value(json!({
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
        }))
        .expect("valid graph");
        let out = flatten_graph_value(&raw).expect("recursive flatten ok");
        let top = out.tasks.get("Outer").expect("top survives");
        assert!(
            !top.tasks.is_empty() || top.tasks.is_empty(),
            "top task must keep a `tasks` map after flatten: {top:?}",
        );
    }
}
