//! TAPA intermediate representation — serde structs for the unified task
//! graph: the model `tapacc` emits, that `tapa analyze` persists into the
//! work dir's `tapa.json`, and that synth, codegen, and pack consume.
//!
//! The state file wrapping that graph ([`WorkState`], the root of
//! `tapa.json`) lives here too: `tapa pack` copies the file verbatim into
//! the `.zip` archive, where `frt-cosim` — in a different Cargo workspace —
//! parses it back with these same types.

pub mod floorplan;
pub mod graph;
pub mod instance;
pub mod interconnect;
pub mod port;
pub mod synth_target;
pub mod target;
pub mod task;
pub mod transforms;
pub mod work_state;

mod error;

pub use error::ParseError;
pub use floorplan::{Area, Crossing, CrossingKind, FloorplanResult, PipelineScheme};
pub use graph::TaskGraph;
pub use instance::{Arg, TaskInstance};
pub use interconnect::{EndpointRef, InterconnectDefinition};
pub use port::{ArgCategory, Port};
pub use synth_target::SynthTarget;
pub use target::Target;
pub use task::{Task, TaskLevel};
pub use transforms::{flatten, TransformError};
pub use work_state::{FlowSettings, WorkState};

// The tapacc-output graph and the post-synthesis design are the same
// unified `TaskGraph` now. These aliases keep the producer/consumer call
// sites reading in terms of the role they handle without a second type.
pub use graph::TaskGraph as Design;
pub use graph::TaskGraph as Graph;
