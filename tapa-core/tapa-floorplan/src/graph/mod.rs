//! The placement graph built from a flattened task graph.

pub mod floor_graph;

pub use floor_graph::{
    fifo_area, AxiNet, FloorGraph, GraphError, MemoryInterface, PlacementEdge, Stream, Vertex,
};
