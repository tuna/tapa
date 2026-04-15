//! TAPA task graph schema — serde structs for `graph.json` (tapacc output).

pub mod graph;
pub mod interconnect;
pub mod instance;
pub mod port;
pub mod task;

mod error;

pub use error::ParseError;
pub use graph::Graph;
