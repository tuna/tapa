//! Coarse-grained floorplanning: cut enumeration and the placement ILP that
//! assigns every [`FloorGraph`](crate::graph::FloorGraph) vertex to a slot.

pub mod cut;

pub use cut::{find_cuts, Cut};
