//! Task-graph transforms: hierarchy flattening and global naming.
//!
//! - [`flatten`] recursively lifts every leaf-task instance under the
//!   top task while preserving FIFO connectivity and global names.
//! - [`flatten_with_origins`] also retains original instance ownership as
//!   typed references for control lowering, without copying invocation policy.
//! - [`inline_uppers`] dissolves only the upper tasks a predicate selects,
//!   one level at a time, keeping the rest of the hierarchy.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use crate::graph::TaskGraph;
use crate::instance::{Arg, ArgSource, TaskInstance};
use crate::interconnect::{EndpointRef, InterconnectDefinition};
use crate::port::ArgCategory;
use crate::task::{Task, TaskLevel};

/// Index of one original task occurrence in [`FlatProjection::occurrences`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OccurrenceId(pub usize);

/// One original task occurrence and its corresponding physical identity.
///
/// The original graph owns every invocation's arguments and step. References
/// here identify that invocation within its original parent occurrence, so
/// repeated upper-task instances keep separate controller ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOccurrence {
    /// Immediate original owner; absent only for the top occurrence.
    pub parent: Option<OccurrenceId>,
    /// Original child definition and index within the parent's task list.
    /// Absent only for the top occurrence, whose definition is `graph.top`.
    pub source: Option<EndpointRef>,
    /// Global scope name for an upper task, or canonical flat leaf name.
    pub global_name: String,
    /// Endpoint in the flattened top's task list; present only for leaves.
    pub flat_endpoint: Option<EndpointRef>,
}

/// A transient flat datapath with references to the original control scopes.
///
/// Occurrence zero is the original top. This projection is derived from one
/// original graph and must be resolved against that graph; it is not persisted
/// or edited as a second semantic graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatProjection {
    /// Leaf-task instances and globally named FIFOs for physical planning.
    pub flat: TaskGraph,
    /// Original occurrences in preorder, including upper tasks and leaves.
    pub occurrences: Vec<TaskOccurrence>,
}

impl FlatProjection {
    /// Resolve an occurrence's original task definition.
    fn source_task<'g>(&self, graph: &'g TaskGraph, occurrence: OccurrenceId) -> Option<&'g Task> {
        let occurrence = self.occurrences.get(occurrence.0)?;
        let name = occurrence
            .source
            .as_ref()
            .map_or(&graph.top, |source| &source.0);
        graph.tasks.get(name)
    }

    /// Resolve an occurrence's invocation in its original owner's scope.
    ///
    /// The top has no invocation. The returned arguments are scope-local;
    /// flattening's substituted leaf arguments do not replace these bindings.
    pub fn source_invocation<'g>(
        &self,
        graph: &'g TaskGraph,
        occurrence: OccurrenceId,
    ) -> Option<&'g TaskInstance> {
        let occurrence = self.occurrences.get(occurrence.0)?;
        let source = occurrence.source.as_ref()?;
        let parent = self.source_task(graph, occurrence.parent?)?;
        parent
            .tasks
            .get(&source.0)?
            .get(usize::try_from(source.1).ok()?)
    }
}

struct FlatInstances {
    leaves: BTreeMap<String, Vec<TaskInstance>>,
    fifos: BTreeMap<String, InterconnectDefinition>,
    occurrences: Vec<TaskOccurrence>,
}

impl FlatInstances {
    fn push_leaf(&mut self, owner: OccurrenceId, source: EndpointRef, instance: TaskInstance) {
        let leaves = self.leaves.entry(source.0.clone()).or_default();
        let global_name = instance
            .canonical_name(&source.0, leaves.len())
            .into_owned();
        let flat_endpoint = EndpointRef(
            source.0.clone(),
            u32::try_from(leaves.len()).expect("instance count fits u32"),
        );
        self.occurrences.push(TaskOccurrence {
            parent: Some(owner),
            source: Some(source),
            global_name,
            flat_endpoint: Some(flat_endpoint),
        });
        leaves.push(instance);
    }
}

/// Error type for graph transforms.
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    /// The top task referenced by `graph.top` is not present in `tasks`.
    #[error("graph is missing the top task `{0}`")]
    MissingTop(String),

    /// The top task is `lower`; flattening requires an upper top.
    #[error("top task `{0}` is a leaf; cannot transform")]
    TopIsLeaf(String),

    /// Distinct FIFO declarations collapse to one name after flattening.
    #[error("multiple FIFO declarations flatten to `{0}`")]
    FifoNameCollision(String),

    /// Inlining would change when a task runs: a sequential invoke orders
    /// its task against its siblings, which the parent's scope cannot keep.
    #[error("cannot inline the upper task `{task}` into `{parent}`: {reason}")]
    Inline {
        parent: String,
        task: String,
        reason: String,
    },
}

/// Build a fresh [`TaskGraph`] with all leaf-task instances re-parented
/// under the top task.
///
/// Upper-level descendants are traversed recursively; their leaf tasks and
/// FIFO connections are rewritten into the top task's scope.
pub fn flatten(graph: &TaskGraph) -> Result<TaskGraph, TransformError> {
    Ok(flatten_with_origins(graph)?.flat)
}

/// Flatten the datapath while retaining original per-occurrence ownership.
///
/// Original invocation references preserve every ancestor's argument binding
/// boundary and detach/join policy. Physical leaf endpoints are recorded in
/// the same traversal that assigns their flat indices and global names.
pub fn flatten_with_origins(graph: &TaskGraph) -> Result<FlatProjection, TransformError> {
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
    let mut collected = FlatInstances {
        leaves: BTreeMap::new(),
        fifos: BTreeMap::new(),
        occurrences: vec![TaskOccurrence {
            parent: None,
            source: None,
            global_name: top_name.clone(),
            flat_endpoint: None,
        }],
    };
    collect_leaves_recursive(
        graph,
        top_name,
        top_name,
        &BTreeMap::new(),
        OccurrenceId(0),
        &mut collected,
    )?;

    // Endpoints resolve only once every instantiation's argument
    // bindings are final.
    for (fifo_name, fifo) in &mut collected.fifos {
        if fifo.consumed_by.is_some() {
            fifo.consumed_by = find_endpoint(&collected.leaves, fifo_name, EndpointRole::Consumer);
        }
        if fifo.produced_by.is_some() {
            fifo.produced_by = find_endpoint(&collected.leaves, fifo_name, EndpointRole::Producer);
        }
    }

    let mut new_tasks = BTreeMap::new();
    for child_name in collected.leaves.keys() {
        if let Some(def) = graph.tasks.get(child_name) {
            new_tasks.insert(child_name.clone(), def.clone());
        }
    }
    let mut new_top_def = top_def.clone();
    new_top_def.tasks = collected.leaves;
    new_top_def.fifos = collected.fifos;
    new_tasks.insert(top_name.clone(), new_top_def);

    Ok(FlatProjection {
        flat: TaskGraph {
            schema_version: graph.schema_version,
            top: top_name.clone(),
            target: graph.target,
            cflags: graph.cflags.clone(),
            tasks: new_tasks,
        },
        occurrences: collected.occurrences,
    })
}

/// Replace every instance of each upper task `inline` selects with that
/// task's children, invoked by the instance's parent under the instance's
/// bindings, until no selected upper task is invoked. A FIFO the inlined
/// instance declares becomes `<instance>__<fifo>` in the parent, and an
/// inlined child is named `<instance>__<child instance>`. A detached
/// instance's children are detached; a sequential invoke on either level
/// is an error. Tasks no longer invoked are dropped; the top always stays.
pub(crate) fn inline_uppers(
    graph: &TaskGraph,
    inline: impl Fn(&str) -> bool,
) -> Result<TaskGraph, TransformError> {
    let mut out = graph.clone();
    while let Some((parent, child)) = out.tasks.iter().find_map(|(parent, def)| {
        def.tasks
            .keys()
            .find(|child| {
                inline(child)
                    && out
                        .tasks
                        .get(*child)
                        .is_some_and(|task| task.level == TaskLevel::Upper)
            })
            .map(|child| (parent.clone(), child.clone()))
    }) {
        inline_into(&mut out, &parent, &child)?;
    }
    let mut invoked = std::collections::BTreeSet::from([out.top.clone()]);
    let mut work = vec![out.top.clone()];
    while let Some(name) = work.pop() {
        for child in out
            .tasks
            .get(&name)
            .into_iter()
            .flat_map(|t| t.tasks.keys())
        {
            if invoked.insert(child.clone()) {
                work.push(child.clone());
            }
        }
    }
    out.tasks.retain(|name, _| invoked.contains(name));
    Ok(out)
}

/// Inlines every instance of `child` that `parent` invokes.
fn inline_into(graph: &mut TaskGraph, parent: &str, child: &str) -> Result<(), TransformError> {
    let def = graph.tasks[child].clone();
    let owner = graph
        .tasks
        .get_mut(parent)
        .expect("the parent was found in this graph");
    let refuse = |reason: String| TransformError::Inline {
        parent: parent.to_string(),
        task: child.to_string(),
        reason,
    };
    for (index, instance) in owner
        .tasks
        .remove(child)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        if instance.step > 0 {
            return Err(refuse(format!(
                "it is invoked sequentially (step {})",
                instance.step
            )));
        }
        let scope = instance.canonical_name(child, index).into_owned();
        let fifos: BTreeMap<String, String> = def
            .fifos
            .iter()
            .filter(|(_, fifo)| fifo.depth.is_some())
            .map(|(name, _)| (name.clone(), format!("{scope}__{name}")))
            .collect();
        let bindings: BTreeMap<String, ArgSource> = instance
            .args
            .iter()
            .map(|(port, arg)| (port.clone(), arg.arg.clone()))
            .collect();
        for (grandchild, inner) in &def.tasks {
            for (inner_index, inner_instance) in inner.iter().enumerate() {
                if inner_instance.step > 0 {
                    return Err(refuse(format!(
                        "it invokes `{grandchild}` sequentially (step {})",
                        inner_instance.step
                    )));
                }
                let args = inner_instance
                    .args
                    .iter()
                    .map(|(port, arg)| {
                        let source = match &arg.arg {
                            ArgSource::Literal(value) => ArgSource::Literal(*value),
                            ArgSource::Name(name) => resolve_scoped_arg(name, &fifos, &bindings),
                        };
                        (
                            port.clone(),
                            Arg {
                                arg: source,
                                cat: arg.cat,
                            },
                        )
                    })
                    .collect();
                let name = inner_instance.canonical_name(grandchild, inner_index);
                owner
                    .tasks
                    .entry(grandchild.clone())
                    .or_default()
                    .push(TaskInstance {
                        name: Some(format!("{scope}__{name}")),
                        args,
                        step: if instance.step < 0 {
                            -1
                        } else {
                            inner_instance.step
                        },
                        position: instance.position,
                    });
            }
        }
        for (local, global) in fifos {
            if owner.fifos.contains_key(&global) {
                return Err(TransformError::FifoNameCollision(global));
            }
            owner.fifos.insert(global, def.fifos[&local].clone());
        }
    }
    let instances = owner.tasks.clone();
    for (name, fifo) in &mut owner.fifos {
        if fifo.produced_by.is_some() {
            fifo.produced_by = find_endpoint(&instances, name, EndpointRole::Producer);
        }
        if fifo.consumed_by.is_some() {
            fifo.consumed_by = find_endpoint(&instances, name, EndpointRole::Consumer);
        }
    }
    Ok(())
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
    arg_bindings: &BTreeMap<String, ArgSource>,
    owner: OccurrenceId,
    collected: &mut FlatInstances,
) -> Result<(), TransformError> {
    let def = graph
        .tasks
        .get(task_name)
        .ok_or_else(|| TransformError::MissingTop(task_name.to_string()))?;

    // Internal FIFOs declared at this scope get renamed to their global form.
    // Top-level internal FIFOs match the single-level shape
    // (`<name>_<top>`); nested FIFOs additionally embed the ancestor
    // instance path (`<name>_<inst_0>_..._<top>`). External top-level FIFOs
    // keep their kernel port names; their declarations are not renamed.
    let mut fifo_global_map: BTreeMap<String, String> = BTreeMap::new();
    let is_top_scope = task_name == graph.top && scope_path == graph.top;
    for (fifo_name, fifo_def) in &def.fifos {
        if !should_materialize_fifo(fifo_def, is_top_scope) {
            continue;
        }
        let global = if fifo_def.depth.is_none() {
            fifo_name.clone()
        } else {
            format!("{fifo_name}_{scope_path}")
        };
        fifo_global_map.insert(fifo_name.clone(), global);
    }

    for (child_def_name, instances) in &def.tasks {
        for (idx, inst) in instances.iter().enumerate() {
            let mut resolved_args: BTreeMap<String, Arg> = BTreeMap::new();
            for (port_name, arg) in &inst.args {
                let resolved = match &arg.arg {
                    // A constant has no parent scope to resolve against.
                    ArgSource::Literal(value) => ArgSource::Literal(*value),
                    ArgSource::Name(name) => {
                        resolve_scoped_arg(name, &fifo_global_map, arg_bindings)
                    }
                };
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
            let source = EndpointRef(
                child_def_name.clone(),
                u32::try_from(idx).expect("instance count fits u32"),
            );
            if child_def.level == TaskLevel::Lower {
                let instance = TaskInstance {
                    // Explicit names are local to an upper instance;
                    // promoted leaves need the ancestor path. Direct top
                    // names and globally indexed automatic names stay intact.
                    name: inst.name.as_ref().map(|name| {
                        if is_top_scope {
                            name.clone()
                        } else {
                            format!("{name}_{scope_path}")
                        }
                    }),
                    args: resolved_args,
                    step: inst.step,
                    position: inst.position,
                };
                collected.push_leaf(owner, source, instance);
            } else {
                // Upper child → descend. Its port bindings become
                // the `arg_bindings` the recursion uses to resolve
                // its own sub-instances' args. Its scope path is
                // prepended with this instance's name.
                let inst_name = inst.canonical_name(child_def_name, idx);
                let child_scope = format!("{inst_name}_{scope_path}");
                let child_occurrence = OccurrenceId(collected.occurrences.len());
                collected.occurrences.push(TaskOccurrence {
                    parent: Some(owner),
                    source: Some(source),
                    global_name: child_scope.clone(),
                    flat_endpoint: None,
                });
                // Constants bind too: a leaf below this child may name the
                // child's port, and the literal is the only value it can
                // take. Dropping them (name-only bindings) leaves a dangling
                // wire named after the child's port.
                let child_bindings: BTreeMap<String, ArgSource> = resolved_args
                    .iter()
                    .map(|(p, a)| (p.clone(), a.arg.clone()))
                    .collect();
                collect_leaves_recursive(
                    graph,
                    child_def_name,
                    &child_scope,
                    &child_bindings,
                    child_occurrence,
                    collected,
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
        if collected.fifos.contains_key(global_name) {
            return Err(TransformError::FifoNameCollision(global_name.clone()));
        }
        collected.fifos.insert(
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
    arg_bindings: &BTreeMap<String, ArgSource>,
) -> ArgSource {
    if let Some(global) = fifo_global_map.get(local_name) {
        return ArgSource::Name(global.clone());
    }
    if let Some(bound) = arg_bindings.get(local_name) {
        return bound.clone();
    }

    if let Some((base, idx, suffix)) = split_array_index(local_name) {
        if suffix.is_empty() {
            // Only a name binding can be array-indexed; a constant has no
            // lanes to offset into.
            if let Some(ArgSource::Name(bound)) = arg_bindings.get(base) {
                return ArgSource::Name(offset_array_binding(bound, idx));
            }
            if let Some(bound) = fifo_global_map.get(base) {
                return ArgSource::Name(offset_array_binding(bound, idx));
            }
        }
    }

    ArgSource::Name(local_name.to_owned())
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
            Self::Consumer => cat.is_input_stream(),
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
                if arg.name() == Some(fifo_global) && role.matches(arg.cat) {
                    let idx_u32 = u32::try_from(idx).expect("instance count fits u32");
                    return Some(EndpointRef(task_name.clone(), idx_u32));
                }
            }
        }
    }
    None
}
