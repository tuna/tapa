//! The placement graph built from a flattened task graph.

pub mod floor_graph;

pub use floor_graph::{fifo_area, FloorGraph, GraphError, PlacementEdge, Stream, Vertex};
