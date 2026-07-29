//! Coarse-grained floorplanning: cut enumeration and the placement ILP that
//! assigns every [`FloorGraph`](crate::graph::FloorGraph) vertex to a slot.

pub mod cut;
pub mod ilp;

pub use cut::{find_cuts_for_regions, Cut};
pub use ilp::{select_strategy, Assignment, IlpError, PartitionStrategy};
