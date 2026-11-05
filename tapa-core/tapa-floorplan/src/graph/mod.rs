//! The placement graph built from a flattened task graph.

mod build;
pub mod floor_graph;
mod query;
mod validate;

// The public graph types keep their `graph::floor_graph` home; the
// build/validate/query split is internal.
pub use floor_graph::{
    AxiNet, ControlInterface, ControlNet, FloorGraph, GraphError, MemoryInterface, PlacementEdge,
    Stream, Vertex,
};

/// Every child instance of `top` in declaration order, as `(definition name,
/// canonical name, instance index, instance)`.
///
/// The one pass over `top.tasks` shared by graph construction and validation,
/// so every pass resolves canonical names and `u32` indices identically.
fn child_instances(
    top: &tapa_ir::Task,
) -> impl Iterator<Item = (&String, String, u32, &tapa_ir::TaskInstance)> + '_ {
    top.tasks.iter().flat_map(|(definition, instances)| {
        instances.iter().enumerate().map(move |(index, instance)| {
            (
                definition,
                instance.canonical_name(definition, index).into_owned(),
                u32::try_from(index).expect("instance count fits u32"),
                instance,
            )
        })
    })
}
