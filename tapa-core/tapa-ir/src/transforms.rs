//! Task-graph transforms: hierarchy flattening and global naming.
//!
//! - [`flatten`] recursively lifts every leaf-task instance under the
//!   top task while preserving FIFO connectivity and global names.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use crate::graph::TaskGraph;
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
}

/// Build a fresh [`TaskGraph`] with all leaf-task instances re-parented
/// under the top task.
///
/// Upper-level descendants are traversed recursively; their leaf tasks and
/// FIFO connections are rewritten into the top task's scope.
pub fn flatten(graph: &TaskGraph) -> Result<TaskGraph, TransformError> {
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
    // against the flattened leaf set after all globally named arguments have
    // been produced.
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

    Ok(TaskGraph {
        top: top_name.clone(),
        target: graph.target,
        cflags: graph.cflags.clone(),
        tasks: new_tasks,
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
/// descends with `<inst_name>_<scope_path>`.
///
/// `arg_bindings` maps *this task's port names* to the globally-
/// resolved arg names in the caller's scope — that's how leaf
/// instances deep in the tree pick up their ancestor's FIFO /
/// external-port bindings.
fn collect_leaves_recursive(
    graph: &TaskGraph,
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
    // Top-level FIFOs match the single-level shape
    // (`<name>_<top>`); nested FIFOs additionally embed the ancestor
    // instance path (`<name>_<inst_0>_..._<top>`).
    let mut fifo_global_map: BTreeMap<String, String> = BTreeMap::new();
    let is_top_scope = scope_path == task_name && arg_bindings.is_empty();
    for (fifo_name, fifo_def) in &def.fifos {
        if !should_materialize_fifo(fifo_def, is_top_scope) {
            continue;
        }
        let global = format!("{fifo_name}_{scope_path}");
        fifo_global_map.insert(fifo_name.clone(), global);
    }

    for (child_def_name, instances) in &def.tasks {
        for (idx, inst) in instances.iter().enumerate() {
            // Resolve every arg: first check if it names a local
            // FIFO (→ global form), then check the parent binding
            // (→ promoted arg), else leave as-is (scalar or
            // external port that keeps its name per current
            // `ExternalPort.global_name = name` rule).
            let mut resolved_args: BTreeMap<String, Arg> = BTreeMap::new();
            for (port_name, arg) in &inst.args {
                let resolved = resolve_scoped_arg(&arg.arg, &fifo_global_map, arg_bindings);
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
                        name: inst.name.clone(),
                        args: resolved_args,
                        step: inst.step,
                    });
            } else {
                // Upper child → descend. Its port bindings become
                // the `arg_bindings` the recursion uses to resolve
                // its own sub-instances' args. Its scope path is
                // prepended with this instance's name.
                let inst_name = inst.canonical_name(child_def_name, idx);
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

fn should_materialize_fifo(fifo: &InterconnectDefinition, is_top_scope: bool) -> bool {
    is_top_scope || fifo.depth.is_some()
}

fn resolve_scoped_arg(
    local_name: &str,
    fifo_global_map: &BTreeMap<String, String>,
    arg_bindings: &BTreeMap<String, String>,
) -> String {
    if let Some(global) = fifo_global_map.get(local_name) {
        return global.clone();
    }
    if let Some(global) = arg_bindings.get(local_name) {
        return global.clone();
    }

    if let Some((base, idx, suffix)) = split_array_index(local_name) {
        if suffix.is_empty() {
            if let Some(bound) = arg_bindings.get(base) {
                return offset_array_binding(bound, idx);
            }
            if let Some(bound) = fifo_global_map.get(base) {
                return offset_array_binding(bound, idx);
            }
        }
    }

    local_name.to_owned()
}

fn split_array_index(name: &str) -> Option<(&str, u32, &str)> {
    let open = name.find('[')?;
    let close_rel = name[open + 1..].find(']')?;
    let close = open + 1 + close_rel;
    let idx = name[open + 1..close].parse().ok()?;
    Some((&name[..open], idx, &name[close + 1..]))
}

fn offset_array_binding(bound: &str, offset: u32) -> String {
    if let Some((base, base_idx, suffix)) = split_array_index(bound) {
        return format!("{base}[{}]{suffix}", base_idx + offset);
    }
    format!("{bound}[{offset}]")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointRole {
    Consumer,
    Producer,
}

impl EndpointRole {
    fn matches(self, cat: ArgCategory) -> bool {
        match self {
            // current: `arg["cat"].startswith("is")` → istream/istreams.
            Self::Consumer => cat.is_input_stream(),
            // current: `arg["cat"].startswith("os")` → ostream/ostreams.
            Self::Producer => cat.is_output_stream(),
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
