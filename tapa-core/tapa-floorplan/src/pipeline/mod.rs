//! The latency-insensitive pipeline plan: turning a placement's cross-slot
//! channels into [`Crossing`](tapa_ir::Crossing) records for codegen.

pub mod plan;

pub use plan::{pipeline_level, plan_crossings, reg_regions, PipelineError};
