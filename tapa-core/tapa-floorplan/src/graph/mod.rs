//! The placement graph built from a flattened task graph.

mod build;
pub mod floor_graph;
mod query;
mod validate;

// floor_graph remains the home of the public graph types so their canonical
// path (`tapa_floorplan::graph::floor_graph::*`) — and thus the recorded
// public API — is unchanged by the build/validate/query split.
pub use floor_graph::{
    fifo_area, AxiNet, ControlInterface, ControlNet, FloorGraph, GraphError, MemoryInterface,
    PlacementEdge, Stream, Vertex,
};
