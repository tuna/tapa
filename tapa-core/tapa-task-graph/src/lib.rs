//! TAPA task graph schema — serde structs for `graph.json` (tapacc output)
//! and `design.json` (the topology bridge written by Python's
//! `tapa/steps/common.py::store_design`).

pub mod design;
pub mod graph;
pub mod interconnect;
pub mod instance;
pub mod port;
pub mod task;

mod error;

pub use design::{Design, TaskTopology};
pub use error::ParseError;
pub use graph::Graph;
