//! The public error enums for plan orchestration: which subsystem failure a
//! [`plan`](crate::plan()) run (or XDC render) surfaced, mapped into one type
//! the CLI can match on.

use tapa_ir::MemoryBank;

use crate::device::select::SelectError;
use crate::graph::GraphError;
use crate::partition::ilp::IlpError;
use crate::pipeline::plan::PipelineError;
use crate::plan::PlanOptionsError;

/// Why [`plan`](crate::plan()) could not produce a floorplan.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// Planner options are invalid.
    #[error(transparent)]
    Options(#[from] PlanOptionsError),
    /// The work state has no resolved part number to select a device from.
    #[error("no part number in the work state; run `synth` first")]
    NoPartNum,
    /// The part number did not resolve to a device table.
    #[error(transparent)]
    Device(#[from] SelectError),
    /// Flattening the task graph failed.
    #[error(transparent)]
    Transform(#[from] tapa_ir::TransformError),
    /// Building the placement graph failed.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The floorplan ILP produced no placement.
    #[error(transparent)]
    Ilp(#[from] IlpError),
    /// The pipeline plan (routing) failed.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
    /// An external bank tag is absent or ambiguous in the selected device.
    #[error("memory bank `{bank}` maps to {matches} device slots; expected exactly one")]
    BankTag { bank: MemoryBank, matches: usize },
    /// Exact shell-interface locations require the platform used to build the
    /// device table.
    #[error("external-memory floorplanning requires platform `{expected}`; rerun synthesis with `--platform`")]
    PlatformRequired { expected: String },
    /// A recorded platform does not match the shell represented by the device
    /// table.
    #[error("platform `{platform}` does not match floorplan device platform `{expected}`")]
    PlatformMismatch { platform: String, expected: String },
    /// A shell-control anchor tag appears in more than one device slot.
    #[error("control anchor tag `{tag}` maps to {matches} device slots; expected at most one")]
    ControlTag { tag: &'static str, matches: usize },
}

/// Why a floorplan's pblock XDC could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum RenderXdcError {
    /// The result's device did not resolve to a device table.
    #[error(transparent)]
    Device(#[from] SelectError),
    /// The persisted result is malformed; emitting Tcl would fail open.
    #[error(transparent)]
    Xdc(#[from] crate::xdc::XdcError),
}
