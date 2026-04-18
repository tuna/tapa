//! Graph transforms ported from `tapa.common.graph.Graph`.
//!
//! - [`flatten`] — port of `Graph::get_flatten_graph`. Implements the
//!   single-level (vadd-shaped) case where every child of the top is
//!   already a leaf-level (`lower`) task. Multi-level hierarchies
//!   surface a typed [`TransformError::DeepHierarchyNotSupported`].
//! - [`apply_floorplan`] — port of `Graph::get_floorplan_graph`. The
//!   structural transform (slot tasks, port projection, FIFO
//!   retargeting, cross-slot FIFO update) is implemented, and the
//!   per-slot C++ wrapper is generated via `tapa_slotting::gen_slot_cpp`
//!   (tree-sitter-backed rewrite of the top task's `extern "C"` block).

mod floorplan_helpers;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::graph::Graph;
use crate::instance::{Arg, TaskInstance};
use crate::interconnect::{EndpointRef, InterconnectDefinition};
use crate::port::ArgCategory;
use crate::task::TaskLevel;

/// Error type for graph transforms.
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    /// The top task referenced by `graph.top` is not present in `tasks`.
    #[error("graph is missing the top task `{0}`")]
    MissingTop(String),

    /// The top task is `lower`; flatten/floorplan both require an upper top.
    #[error("top task `{0}` is a leaf; cannot transform")]
    TopIsLeaf(String),

    /// Flatten encountered a child task that is itself upper-level.
    /// Only the single-level (vadd-shaped) case is supported today.
    #[error(
        "flatten currently only supports single-level hierarchies; \
         child task `{0}` is `upper` and must be inlined first \
         (port `Graph::get_flatten_graph` recursive case to lift this)"
    )]
    DeepHierarchyNotSupported(String),

    /// A floorplan slot membership entry references an unknown instance.
    #[error("floorplan: instance `{0}` not found among top's leaf children")]
    UnknownFloorplanInstance(String),

    /// A slot listed in `slot_to_insts` collides with an existing task.
    #[error("floorplan: slot name `{0}` collides with an existing task")]
    SlotNameCollision(String),

    /// A child task referenced from the top's instantiations is missing
    /// from `graph.tasks` (needed to look up port types during slot C++
    /// generation).
    #[error("floorplan: child task definition `{0}` not found in graph.tasks")]
    UnknownChildTask(String),

    /// The slot wrapper C++ emitter (`tapa_slotting::gen_slot_cpp`)
    /// rejected the slot. Wraps the underlying slotting error.
    #[error("floorplan: slot C++ generation failed for `{slot}`: {source}")]
    SlotCppGeneration {
        slot: String,
        #[source]
        source: tapa_slotting::error::SlottingError,
    },

    /// JSON conversion failure (used by [`flatten_value`]).
    #[error("transform JSON failure: {0}")]
    Json(String),
}

/// Build a fresh [`Graph`] with all leaf-task instances re-parented under
/// the top task.
///
/// Mirrors `Graph::get_flatten_graph`. Implements the single-level case
/// where every child of the top is already a leaf (`lower`). Deeper
/// nesting surfaces [`TransformError::DeepHierarchyNotSupported`].
pub fn flatten(graph: &Graph) -> Result<Graph, TransformError> {
    let top_name = &graph.top;
    let top_def = graph
        .tasks
        .get(top_name)
        .ok_or_else(|| TransformError::MissingTop(top_name.clone()))?;
    if top_def.level == TaskLevel::Lower {
        return Err(TransformError::TopIsLeaf(top_name.clone()));
    }

    // Recursively inline every upper descendant: leaf instantiations
    // float up to the top, FIFOs that live inside nested upper tasks
    // are rewritten to their global names (`<fifo>_<inst_path>_<top>`)
    // and stitched back together against the collected leaves.
    // Mirrors Python's `Graph.get_flatten_graph` +
    // `TaskInstance.get_leaf_tasks_insts` + `recursive_get_interconnect_insts`.
    let mut leaves: BTreeMap<String, Vec<TaskInstance>> = BTreeMap::new();
    let mut fifos: BTreeMap<String, InterconnectDefinition> = BTreeMap::new();
    collect_leaves_recursive(
        graph,
        top_name,
        top_name,
        &BTreeMap::new(),
        &mut leaves,
        &mut fifos,
    )?;

    // Post-pass: now that every instantiation has its final argument
    // bindings, fill in each FIFO's `consumed_by` / `produced_by`
    // against the flattened leaf set. This mirrors the Python side
    // which recomputes endpoints after `interconnect_global_name=True`
    // args have been produced.
    let fifo_names: Vec<String> = fifos.keys().cloned().collect();
    for fifo_name in fifo_names {
        let had_consumer = fifos
            .get(&fifo_name)
            .and_then(|f| f.consumed_by.as_ref())
            .is_some();
        let had_producer = fifos
            .get(&fifo_name)
            .and_then(|f| f.produced_by.as_ref())
            .is_some();
        if had_consumer {
            let consumed = find_endpoint(&leaves, &fifo_name, EndpointRole::Consumer);
            if let Some(entry) = fifos.get_mut(&fifo_name) {
                entry.consumed_by = consumed;
            }
        }
        if had_producer {
            let produced = find_endpoint(&leaves, &fifo_name, EndpointRole::Producer);
            if let Some(entry) = fifos.get_mut(&fifo_name) {
                entry.produced_by = produced;
            }
        }
    }

    let mut new_tasks = BTreeMap::new();
    for child_name in leaves.keys() {
        if let Some(def) = graph.tasks.get(child_name) {
            new_tasks.insert(child_name.clone(), def.clone());
        }
    }
    let mut new_top_def = top_def.clone();
    new_top_def.tasks = leaves;
    new_top_def.fifos = fifos;
    new_tasks.insert(top_name.clone(), new_top_def);

    Ok(Graph {
        cflags: graph.cflags.clone(),
        tasks: new_tasks,
        top: top_name.clone(),
    })
}

/// Recursive hierarchy-flattening helper. Walks from `task_name`
/// (starting at the top) and collects every leaf instantiation it
/// encounters into `leaves`, rewriting args according to FIFO
/// remapping at this level and the `arg_bindings` handed down from
/// the parent. FIFOs at this scope are renamed to their global form
/// and added to `fifos`. Recurses into upper-level children.
///
/// `scope_path` is the `parent.global_name` for this call site: the
/// top-level call uses `top_name`, and each upper-child instantiation
/// descends with `<inst_name>_<scope_path>`. Matches Python's
/// `Base._generate_global_name_without_sub` chain.
///
/// `arg_bindings` maps *this task's port names* to the globally-
/// resolved arg names in the caller's scope — that's how leaf
/// instances deep in the tree pick up their ancestor's FIFO /
/// external-port bindings.
fn collect_leaves_recursive(
    graph: &Graph,
    task_name: &str,
    scope_path: &str,
    arg_bindings: &BTreeMap<String, String>,
    leaves: &mut BTreeMap<String, Vec<TaskInstance>>,
    fifos: &mut BTreeMap<String, InterconnectDefinition>,
) -> Result<(), TransformError> {
    let def = graph
        .tasks
        .get(task_name)
        .ok_or_else(|| TransformError::MissingTop(task_name.to_string()))?;

    // FIFOs declared at this scope get renamed to their global form.
    // Top-level FIFOs match the legacy single-level shape
    // (`<name>_<top>`); nested FIFOs additionally embed the ancestor
    // instance path (`<name>_<inst_0>_..._<top>`).
    let mut fifo_global_map: BTreeMap<String, String> = BTreeMap::new();
    for fifo_name in def.fifos.keys() {
        let global = format!("{fifo_name}_{scope_path}");
        fifo_global_map.insert(fifo_name.clone(), global);
    }

    for (child_def_name, instances) in &def.tasks {
        for (idx, inst) in instances.iter().enumerate() {
            // Resolve every arg: first check if it names a local
            // FIFO (→ global form), then check the parent binding
            // (→ promoted arg), else leave as-is (scalar or
            // external port that keeps its name per Python's
            // `ExternalPort.global_name = name` rule).
            let mut resolved_args: BTreeMap<String, Arg> = BTreeMap::new();
            for (port_name, arg) in &inst.args {
                let resolved = fifo_global_map
                    .get(&arg.arg)
                    .cloned()
                    .or_else(|| arg_bindings.get(&arg.arg).cloned())
                    .unwrap_or_else(|| arg.arg.clone());
                resolved_args.insert(
                    port_name.clone(),
                    Arg {
                        arg: resolved,
                        cat: arg.cat,
                    },
                );
            }

            let child_def = graph
                .tasks
                .get(child_def_name)
                .ok_or_else(|| TransformError::MissingTop(child_def_name.clone()))?;
            if child_def.level == TaskLevel::Lower {
                leaves
                    .entry(child_def_name.clone())
                    .or_default()
                    .push(TaskInstance {
                        args: resolved_args,
                        step: inst.step,
                    });
            } else {
                // Upper child → descend. Its port bindings become
                // the `arg_bindings` the recursion uses to resolve
                // its own sub-instances' args. Its scope path is
                // prepended with this instance's name.
                let inst_name = format!("{child_def_name}_{idx}");
                let child_scope = format!("{inst_name}_{scope_path}");
                let child_bindings: BTreeMap<String, String> = resolved_args
                    .iter()
                    .map(|(p, a)| (p.clone(), a.arg.clone()))
                    .collect();
                collect_leaves_recursive(
                    graph,
                    child_def_name,
                    &child_scope,
                    &child_bindings,
                    leaves,
                    fifos,
                )?;
            }
        }
    }

    // Register this scope's FIFOs now that recursion filled `leaves`.
    // We can't set `consumed_by` / `produced_by` yet because a FIFO
    // introduced at this scope may only acquire its endpoints once
    // deeper recursion adds the leaves that reference it; the caller
    // (`flatten`) runs a final pass to fill them in.
    for (local_name, global_name) in &fifo_global_map {
        let fifo_def = def.fifos.get(local_name).expect("local fifo present");
        fifos.insert(
            global_name.clone(),
            InterconnectDefinition {
                depth: fifo_def.depth,
                // Keep the original `Some(_)` presence so the
                // endpoint-fill pass knows which side to resolve.
                // Actual endpoint is overwritten there.
                consumed_by: fifo_def.consumed_by.clone(),
                produced_by: fifo_def.produced_by.clone(),
            },
        );
    }

    Ok(())
}

/// Wrap groups of leaf instances under synthetic per-slot upper tasks.
///
/// Mirrors `Graph::get_floorplan_graph`. Returns the rewritten graph
/// plus a `slot_task_name_to_fp_region` echo map keyed by slot name; the
/// CLI is expected to pair these keys with whatever region map it
/// computed via [`convert_region_format`].
///
/// Each slot task's `code` is generated by `tapa_slotting::gen_slot_cpp`,
/// which rewrites the top task's `extern "C"` block into a slot wrapper
/// with matching HLS pragmas.
pub fn apply_floorplan(
    graph: &Graph,
    slot_to_insts: &BTreeMap<String, Vec<String>>,
) -> Result<(Graph, BTreeMap<String, String>), TransformError> {
    let top_name = &graph.top;
    let top_def = graph
        .tasks
        .get(top_name)
        .ok_or_else(|| TransformError::MissingTop(top_name.clone()))?;
    if top_def.level == TaskLevel::Lower {
        return Err(TransformError::TopIsLeaf(top_name.clone()));
    }

    let mut inst_name_to_pos: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for (def_name, insts) in &top_def.tasks {
        for (idx, _inst) in insts.iter().enumerate() {
            inst_name_to_pos.insert(format!("{def_name}_{idx}"), (def_name.clone(), idx));
        }
    }

    for inst_name in slot_to_insts.values().flatten() {
        if !inst_name_to_pos.contains_key(inst_name) {
            return Err(TransformError::UnknownFloorplanInstance(inst_name.clone()));
        }
    }

    let mut inst_to_slot: BTreeMap<String, String> = BTreeMap::new();
    for (slot_name, insts) in slot_to_insts {
        if graph.tasks.contains_key(slot_name) {
            return Err(TransformError::SlotNameCollision(slot_name.clone()));
        }
        for inst in insts {
            inst_to_slot.insert(inst.clone(), slot_name.clone());
        }
    }

    let mut slot_defs = BTreeMap::new();
    for (slot_name, insts_in_slot) in slot_to_insts {
        let slot_def = floorplan_helpers::build_slot_def(
            top_def,
            top_name,
            slot_name,
            insts_in_slot,
            &inst_name_to_pos,
            &graph.tasks,
        )?;
        slot_defs.insert(slot_name.clone(), slot_def);
    }

    let new_top_tasks =
        floorplan_helpers::build_top_slot_instantiations(&slot_defs, top_def, &inst_to_slot);
    let in_slot_fifo_names: BTreeSet<String> = slot_defs
        .values()
        .flat_map(|d| d.fifos.iter())
        .filter(|(_, f)| f.depth.is_some())
        .map(|(n, _)| n.clone())
        .collect();
    let new_top_fifos = floorplan_helpers::update_cross_slot_fifos(
        &top_def.fifos,
        &in_slot_fifo_names,
        &inst_to_slot,
    );

    let mut new_tasks = graph.tasks.clone();
    for (slot_name, slot_def) in &slot_defs {
        new_tasks.insert(slot_name.clone(), slot_def.clone());
    }
    let mut new_top_def = top_def.clone();
    new_top_def.tasks = new_top_tasks;
    new_top_def.fifos = new_top_fifos;
    new_tasks.insert(top_name.clone(), new_top_def);

    let new_graph = Graph {
        cflags: graph.cflags.clone(),
        tasks: new_tasks,
        top: top_name.clone(),
    };
    let region_map: BTreeMap<String, String> = slot_to_insts
        .keys()
        .map(|n| (n.clone(), n.clone()))
        .collect();
    Ok((new_graph, region_map))
}

/// Convert a floorplan region string from Python's `"x:y"` form to the
/// canonical `"x_TO_y"` form used by `slot_task_name_to_fp_region`.
///
/// Mirrors `tapa.common.floorplan.convert_region_format`.
#[must_use]
pub fn convert_region_format(region: &str) -> String {
    region.replace(':', "_TO_")
}

/// Compute the slot name from a Python-style region by replacing `:`
/// with `_` (mirrors `slot_name = "_".join(region.split(":"))`).
#[must_use]
pub fn region_to_slot_name(region: &str) -> String {
    region.replace(':', "_")
}

/// Convenience wrapper: round-trip a `serde_json::Value` graph through
/// the typed [`Graph`] schema and apply [`flatten`].
pub fn flatten_value(value: &Value) -> Result<Value, TransformError> {
    let typed: Graph = serde_json::from_value(value.clone())
        .map_err(|e| TransformError::Json(e.to_string()))?;
    let flat = flatten(&typed)?;
    serde_json::to_value(&flat).map_err(|e| TransformError::Json(e.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointRole {
    Consumer,
    Producer,
}

impl EndpointRole {
    fn matches(self, cat: ArgCategory) -> bool {
        match self {
            // Python: `arg["cat"].startswith("is")` → istream/istreams.
            Self::Consumer => matches!(cat, ArgCategory::Istream | ArgCategory::Istreams),
            // Python: `arg["cat"].startswith("os")` → ostream/ostreams.
            Self::Producer => matches!(cat, ArgCategory::Ostream | ArgCategory::Ostreams),
        }
    }
}

fn find_endpoint(
    instantiations: &BTreeMap<String, Vec<TaskInstance>>,
    fifo_global: &str,
    role: EndpointRole,
) -> Option<EndpointRef> {
    for (task_name, insts) in instantiations {
        for (idx, inst) in insts.iter().enumerate() {
            for arg in inst.args.values() {
                if arg.arg == fifo_global && role.matches(arg.cat) {
                    let idx_u32 = u32::try_from(idx).unwrap_or(u32::MAX);
                    return Some(EndpointRef(task_name.clone(), idx_u32));
                }
            }
        }
    }
    None
}
