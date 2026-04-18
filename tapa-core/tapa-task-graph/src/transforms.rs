//! Topology transforms over `Graph` — `get_flatten_graph` and
//! `get_floorplan_graph` ports of `tapa.common.graph.Graph`.
//!
//! These are the long-pole helpers blocking native composites from
//! running end-to-end on the vadd fixture. The full Python orchestration
//! involves multi-level upper-task inlining + FIFO renaming + slot
//! synthesis; the Rust port lands incrementally. For now we expose the
//! API surface so callers can wire against it, and surface a typed
//! [`TransformError`] when the input shape exceeds what's supported.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::graph::Graph;

/// Errors surfaced by `flatten` / `apply_floorplan`.
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    /// The graph nests upper tasks beyond what the current native
    /// transform supports. Carries the offending child name so the
    /// caller can build a precise diagnostic.
    #[error(
        "deep hierarchy not supported by the native flatten port: \
         child task `{0}` is itself an upper task. Port \
         `tapa.common.graph.Graph::get_flatten_graph` to lift the limit."
    )]
    DeepHierarchyNotSupported(String),

    /// `apply_floorplan` was given a slot mapping referencing tasks
    /// not present in the graph.
    #[error("floorplan references unknown task instance `{0}`")]
    UnknownInstance(String),

    /// Floorplan-graph transform is not yet ported; carries the
    /// number of slot groups requested for diagnostics.
    #[error(
        "floorplan-graph transform not yet supported by the native port \
         (got {0} slot groups); port \
         `tapa.common.graph.Graph::get_floorplan_graph` to enable it."
    )]
    FloorplanNotSupported(usize),

    /// Anything else (typically a malformed graph dict).
    #[error("transform failed: {0}")]
    Other(String),
}

/// Flatten the upper-task hierarchy.
///
/// Mirrors `tapa.common.graph.Graph::get_flatten_graph`. The full
/// transform inlines every upper task under the top so the result has
/// only leaf-level instantiations. The Rust port currently returns
/// `Ok(graph.clone())` for graphs already at depth ≤1 (e.g. the vadd
/// fixture: top is the only upper task). Deeper nesting surfaces
/// [`TransformError::DeepHierarchyNotSupported`] so callers can fall
/// back to the Python implementation.
pub fn flatten(graph: &Graph) -> Result<Graph, TransformError> {
    for task in graph.tasks.values() {
        for child_name in task.tasks.keys() {
            if let Some(child) = graph.tasks.get(child_name) {
                if !child.tasks.is_empty() {
                    return Err(TransformError::DeepHierarchyNotSupported(
                        child_name.clone(),
                    ));
                }
            }
        }
    }
    Ok(graph.clone())
}

/// Apply a floorplan-derived slot mapping.
///
/// Mirrors `tapa.common.graph.Graph::get_floorplan_graph`. Returns
/// the rewritten graph + the `slot_task_name_to_fp_region` mapping
/// the caller persists into `design.json`. The full transform
/// synthesizes a slot task per region group and rewires connections;
/// the Rust port currently surfaces
/// [`TransformError::FloorplanNotSupported`] so callers can fall
/// back to the Python implementation.
pub fn apply_floorplan(
    _graph: &Graph,
    slot_to_insts: &BTreeMap<String, Vec<String>>,
) -> Result<(Graph, BTreeMap<String, String>), TransformError> {
    Err(TransformError::FloorplanNotSupported(slot_to_insts.len()))
}

/// Mirror `tapa.common.floorplan.convert_region_format`: turns
/// `"SLOT_X0Y0:SLOT_X0Y0"` into `"SLOT_X0Y0_SLOT_X0Y0"`.
pub fn region_to_slot_name(region: &str) -> String {
    region.replace(':', "_")
}

/// Compatibility alias used by `floorplan.rs` ports.
pub fn convert_region_format(region: &str) -> String {
    region_to_slot_name(region)
}

/// Convenience for callers operating on `serde_json::Value` graphs
/// (e.g. `analyze` which stores its mid-pipeline state as a `Value`).
pub fn flatten_value(value: &Value) -> Result<Value, TransformError> {
    let graph: Graph = serde_json::from_value(value.clone())
        .map_err(|e| TransformError::Other(e.to_string()))?;
    let flattened = flatten(&graph)?;
    serde_json::to_value(&flattened).map_err(|e| TransformError::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use crate::task::{TaskDefinition, TaskLevel};

    fn empty_graph(top: &str) -> Graph {
        let mut tasks = std::collections::BTreeMap::new();
        tasks.insert(
            top.to_string(),
            TaskDefinition {
                code: String::new(),
                level: TaskLevel::Upper,
                target: "hls".into(),
                vendor: String::new(),
                ports: Vec::new(),
                tasks: std::collections::BTreeMap::new(),
                fifos: std::collections::BTreeMap::new(),
            },
        );
        Graph {
            cflags: Vec::new(),
            tasks,
            top: top.to_string(),
        }
    }

    #[test]
    fn flatten_passes_through_single_level() {
        let g = empty_graph("Top");
        let flat = flatten(&g).expect("single-level flatten succeeds");
        assert_eq!(flat.top, "Top");
    }

    #[test]
    fn region_to_slot_name_normalizes_colons() {
        assert_eq!(
            region_to_slot_name("SLOT_X0Y0:SLOT_X0Y0"),
            "SLOT_X0Y0_SLOT_X0Y0",
        );
    }

    #[test]
    fn apply_floorplan_signals_not_yet_implemented() {
        let g = empty_graph("Top");
        let mapping = std::collections::BTreeMap::new();
        let err = apply_floorplan(&g, &mapping).expect_err("not yet implemented");
        assert!(matches!(err, TransformError::FloorplanNotSupported(0)));
    }
}
