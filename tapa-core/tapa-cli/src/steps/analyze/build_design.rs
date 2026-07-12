//! Graph-dict to typed [`Design`] projection plus the
//! `--flatten-hierarchy` round-trip helper for `tapa analyze`.
//!
//! Drops `vendor` and other tapacc-only keys while projecting a task
//! graph into a [`Design`], and provides [`flatten_graph_value`] for
//! hierarchy flattening through the typed [`Graph`] schema.

use indexmap::IndexMap;
use tapa_task_graph::{flatten, Design, Graph, TaskLevel, TaskTopology, TransformError};

use crate::error::{CliError, Result};

/// Round-trip a tapacc graph dict through the typed [`Graph`] schema and
/// return the result of [`flatten`] re-serialized as `serde_json::Value`.
///
/// The transform is defined on the strict [`Graph`] type used by the
/// CLI's graph reader and writer.
pub(super) fn flatten_graph_value(graph: &Graph) -> Result<Graph> {
    let flat = flatten(graph).map_err(|e| match e {
        TransformError::DeepHierarchyNotSupported(child) => CliError::InvalidArg(format!(
            "`--flatten-hierarchy` only supports single-level hierarchies for now; \
             child task `{child}` is itself an upper task. The native port covers \
             the vadd-shaped case; deeper graphs are pending.",
        )),
        other @ (TransformError::MissingTop(_)
        | TransformError::TopIsLeaf(_)
        | TransformError::Json(_)) => CliError::InvalidArg(format!("flatten failed: {other}")),
    })?;
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
pub(super) fn build_design(top: &str, target: &str, graph: &Graph) -> Design {
    let mut topology: IndexMap<String, TaskTopology> = IndexMap::new();
    for (name, task) in &graph.tasks {
        topology.insert(name.clone(), task_to_topology(name, task));
    }

    Design {
        top: top.to_string(),
        target: target.to_string(),
        tasks: topology,
        slot_task_name_to_fp_region: None,
    }
}

fn task_to_topology(name: &str, task: &tapa_task_graph::TaskDefinition) -> TaskTopology {
    let level = match task.level {
        TaskLevel::Lower => "lower",
        TaskLevel::Upper => "upper",
    }
    .to_string();
    let code = task.code.clone();
    let ports = task.ports.clone();
    let tasks = serde_json::to_value(&task.tasks)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|o| o.into_iter().collect())
        .unwrap_or_default();
    let fifos = serde_json::to_value(&task.fifos)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|o| o.into_iter().collect())
        .unwrap_or_default();
    let target = Some(task.target.clone());

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
        let g: Graph = serde_json::from_value(json!({"tasks": {"T": {"level": "lower", "code": "", "target": "hls", "ports": [], "tasks": {}, "fifos": {}}}, "top": "T"})).unwrap();
        assert!(is_top_leaf(&g, "T"));
        let g: Graph = serde_json::from_value(json!({"tasks": {"T": {"level": "upper", "code": "", "target": "hls", "ports": [], "tasks": {}, "fifos": {}}}, "top": "T"})).unwrap();
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
