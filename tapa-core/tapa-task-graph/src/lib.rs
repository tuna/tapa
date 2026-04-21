//! TAPA task graph schema — serde structs for `graph.json` (tapacc output)
//! and `design.json` (the topology bridge written by current
//! the implementation).

pub mod design;
pub mod graph;
pub mod instance;
pub mod interconnect;
pub mod port;
pub mod task;
pub mod transforms;

mod error;

pub use design::{Design, TaskTopology};
pub use error::ParseError;
pub use graph::Graph;
pub use instance::{Arg, TaskInstance};
pub use interconnect::{EndpointRef, InterconnectDefinition};
pub use port::{ArgCategory, Port};
pub use task::{TaskDefinition, TaskLevel};
pub use transforms::{flatten, TransformError};
