//! TAPA intermediate representation — serde structs for `graph.json`
//! (tapacc output) and `design.json` (the design model consumed by
//! synth, codegen, and pack).

pub mod design;
pub mod graph;
pub mod instance;
pub mod interconnect;
pub mod port;
pub mod synth_target;
pub mod target;
pub mod task;
pub mod transforms;

mod error;

pub use design::{Design, Task};
pub use error::ParseError;
pub use graph::Graph;
pub use instance::{Arg, TaskInstance};
pub use interconnect::{EndpointRef, InterconnectDefinition};
pub use port::{ArgCategory, Port};
pub use synth_target::SynthTarget;
pub use target::Target;
pub use task::{TaskDefinition, TaskLevel};
pub use transforms::{flatten, TransformError};
