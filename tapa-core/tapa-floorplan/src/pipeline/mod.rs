//! The latency-insensitive pipeline plan: turning a placement's cross-slot
//! channels into [`PipelineRoute`](tapa_ir::PipelineRoute) records for codegen.

pub mod plan;

pub use plan::{pipeline_reg_regions, plan_routes, PipelineError};
