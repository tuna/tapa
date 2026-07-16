//! TAPA intermediate representation — serde structs for the unified task
//! graph: the model `tapacc` emits, that `tapa analyze` persists into the
//! work dir's `tapa.json`, and that synth, codegen, and pack consume.

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

// The tapacc-output graph and the post-synthesis design are the same
// unified `TaskGraph` now. These aliases keep the producer/consumer call
// sites reading in terms of the role they handle without a second type.
pub use graph::TaskGraph as Design;
pub use graph::TaskGraph as Graph;
