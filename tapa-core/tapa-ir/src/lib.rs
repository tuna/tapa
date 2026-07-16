//! TAPA intermediate representation — serde structs for the unified task
//! graph serialized as both `graph.json` (tapacc output) and `design.json`
//! (the design model consumed by synth, codegen, and pack).

pub mod graph;
pub mod instance;
pub mod interconnect;
pub mod port;
pub mod synth_target;
pub mod target;
pub mod task;
pub mod transforms;

mod error;

pub use error::ParseError;
pub use graph::TaskGraph;
pub use instance::{Arg, TaskInstance};
pub use interconnect::{EndpointRef, InterconnectDefinition};
pub use port::{ArgCategory, Port};
pub use synth_target::SynthTarget;
pub use target::Target;
pub use task::{Task, TaskLevel};
pub use transforms::{flatten, TransformError};

// `graph.json` and `design.json` are the same unified `TaskGraph` now.
// These aliases keep the producer/consumer call sites reading in terms of
// the artifact they handle without a second type.
pub use graph::TaskGraph as Design;
pub use graph::TaskGraph as Graph;
