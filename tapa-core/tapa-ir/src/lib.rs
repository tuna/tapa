//! TAPA intermediate representation — serde structs for the unified task
//! graph: the model `tapacc` emits, that `tapa analyze` persists into the
//! work dir's `tapa.json`, and that synth, codegen, and pack consume.
//!
//! The state file wrapping that graph ([`WorkState`], the root of
//! `tapa.json`) lives here too: `tapa pack` copies the file verbatim into
//! the `.zip` archive, where `frt-cosim` — in a different Cargo workspace —
//! parses it back with these same types.

pub mod clock;
pub mod connectivity;
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

pub use clock::{ClockPeriod, ClockPeriodError};
pub use connectivity::{
    DuplicateMemoryEndpoint, MemoryBank, MemoryBinding, MemoryBindings, MemoryEndpoint, MemoryKind,
};
pub use error::ParseError;
pub use floorplan::{
    async_mmap_bridge_instance_name, axi_pipeline_instance_name, control_pipeline_instance_name,
    floorplanned_fifo_storage_depth, global_controller_instance_name,
    local_controller_instance_name, Area, AxiChannel, AxiChannelWidths, AxiEndpoint,
    ControlChannel, Coor, FloorplanResult, PipelineRoute, PipelineScheme, RoutedChannel,
};
pub use graph::TaskGraph;
pub use instance::{Arg, ArgSource, TaskInstance, WireValue};
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
