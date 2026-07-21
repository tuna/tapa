//! Inter-slot routing: candidate path enumeration and the routing ILP that
//! picks a path per cross-slot net.

pub mod ilp;
pub mod paths;

pub use ilp::{route_nets, slot_tag, RouteError, RouteNet};
pub use paths::{enumerate_paths, Cell};
