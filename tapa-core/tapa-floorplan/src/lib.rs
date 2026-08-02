//! Coarse-grained floorplanning and latency-insensitive pipeline planning for
//! TAPA dataflow designs on multi-die AMD FPGAs.
//!
//! The planner assigns each flattened task cluster to a physical *slot* on a
//! rows×cols grid (rows = SLRs) by solving a wire-crossing-minimizing ILP under
//! per-slot resource and per-boundary wire-capacity constraints, then plans
//! register pipelining for every channel that crosses a slot boundary. Internal
//! FIFO storage is charged to and co-located with its destination cluster.
//! Its output is a [`tapa_ir::FloorplanResult`], the plain-data contract
//! codegen consumes.
//!
//! Module map:
//! - [`device`] — device model (`Area`/`Coor`/`Slot`/`Device`), embedded
//!   per-part JSON tables, and `part_num → Device` selection.
//! - `solver` — an `LpModel` + CPLEX-LP writer + `Solver` trait, with a first
//!   backend that spawns the external `cbc` binary.
//! - `graph`/`partition` — the `FloorGraph` and the floorplan ILP.
//! - `route`/`pipeline` — inter-slot routing and the pipeline plan.
//! - `plan` — planner options and the prepare/solve/finish orchestration,
//!   including the exact-cap DSE policy; `error` maps subsystem failures into
//!   the public error enums.
//! - `xdc` — pblock/anchor XDC emission from a `FloorplanResult`.

pub mod device;
pub mod dse;
pub mod graph;
pub mod partition;
pub mod pipeline;
pub mod route;
pub mod solver;
pub mod xdc;

mod error;
mod plan;

pub use crate::error::{PlanError, RenderXdcError};
pub use crate::graph::{ControlInterface, MemoryInterface};
pub use crate::partition::PartitionStrategy;
pub use crate::plan::{
    plan, plan_with_inputs, render_xdc, PlanInputs, PlanOptions, PlanOptionsError,
};

/// Model-fingerprint instrumentation consumed by
/// `tests/golden_model_fingerprints.rs`; not a public contract.
#[doc(hidden)]
pub use crate::plan::fingerprint::fingerprint_plan_models_json;

pub(crate) use crate::plan::{
    plan_with_inputs_at_usage_limit_and_caps, ExactDseResourceCaps, EXACT_DSE_CAP_SCALE,
    MULTILEVEL_BLOCK_RESOURCE_MARGIN_UNITS,
};

/// Widening conversion of the small exact integers the formulations scale
/// into `f64` solver coefficients.
///
/// Every converted value is a physical count or grid coordinate far below
/// 2^53, so the cast is exact.
pub(crate) trait ExactInt: Copy {
    /// The value widened to `f64`.
    fn as_f64(self) -> f64;
}

#[allow(
    clippy::cast_precision_loss,
    reason = "converted values are physical counts and grid coordinates far below 2^53"
)]
impl ExactInt for u64 {
    fn as_f64(self) -> f64 {
        self as f64
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "converted values are physical counts and grid coordinates far below 2^53"
)]
impl ExactInt for i64 {
    fn as_f64(self) -> f64 {
        self as f64
    }
}
