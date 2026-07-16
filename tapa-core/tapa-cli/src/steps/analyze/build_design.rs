//! Graph helpers for `tapa analyze`: the top-leaf check and the
//! `--flatten-hierarchy` round-trip through the typed [`TaskGraph`] schema.

use tapa_ir::{flatten, Graph, TaskLevel};

use crate::error::{CliError, Result};

/// Round-trip a task graph through [`flatten`], returning the flattened
/// [`Graph`]. Defined on the strict typed schema used by the CLI's graph
/// reader and writer.
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

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn is_top_leaf_detects_lower_level() {
        let g: Graph = serde_json::from_value(json!({"target": "xilinx-hls", "tasks": {"T": {"level": "lower", "code": "", "synth": "hls", "readable_name": "T", "ports": [], "tasks": {}, "fifos": {}}}, "top": "T"})).unwrap();
        assert!(is_top_leaf(&g, "T"));
        let g: Graph = serde_json::from_value(json!({"target": "xilinx-hls", "tasks": {"T": {"level": "upper", "code": "", "synth": "hls", "readable_name": "T", "ports": [], "tasks": {}, "fifos": {}}}, "top": "T"})).unwrap();
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
            "target": "xilinx-hls",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd",
                    "code": "void VecAdd() {}",
                    "level": "upper",
                    "synth": "hls",
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
                    "readable_name": "A",
                    "code": "void A() {}", "level": "lower",
                    "synth": "hls",
                    "ports": [
                        {"cat": "scalar", "name": "n",
                         "type": "uint64_t", "width": 64},
                        {"cat": "ostream", "name": "out",
                         "type": "float", "width": 32}
                    ]
                },
                "B": {
                    "readable_name": "B",
                    "code": "void B() {}", "level": "lower",
                    "synth": "hls",
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
            "target": "xilinx-hls",
            "tasks": {
                "Outer": {
                    "readable_name": "Outer",
                    "code": "", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {"Inner": [{"args": {}, "step": 0}]},
                    "fifos": {}
                },
                "Inner": {
                    "readable_name": "Inner",
                    "code": "", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {}, "fifos": {}
                }
            }
        }))
        .expect("valid graph");
        let out = flatten_graph_value(&raw).expect("recursive flatten ok");
        let top = out.tasks.get("Outer").expect("top survives");
        assert!(
            top.tasks.is_empty(),
            "Inner has no leaves, so the flattened top has an empty `tasks` map: {top:?}",
        );
    }
}
