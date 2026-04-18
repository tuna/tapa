//! TAPA task graph schema — serde structs for `graph.json` (tapacc output)
//! and `design.json` (the topology bridge written by Python's
//! `tapa/steps/common.py::store_design`).

pub mod design;
pub mod graph;
pub mod interconnect;
pub mod instance;
pub mod port;
pub mod task;
pub mod transforms;

mod error;

pub use design::{Design, TaskTopology};
pub use error::ParseError;
pub use graph::Graph;
pub use transforms::{
    apply_floorplan, convert_region_format, flatten, region_to_slot_name, TransformError,
};
