//! Public planner options and their validation.

use tapa_ir::PipelineScheme;

use crate::device::model::DEFAULT_USAGE_LIMIT;
use crate::partition::PartitionStrategy;

/// Options controlling a [`plan`](crate::plan()) run. Defaults match the CLI's defaults.
#[derive(Debug, Clone, Copy)]
pub struct PlanOptions {
    /// Base per-slot utilization target; raised on infeasibility.
    pub usage_limit: f64,
    /// ILP wall-clock limit, in seconds.
    pub max_seconds: u64,
    /// CBC worker threads. `1` keeps the solve deterministic.
    pub threads: u32,
    /// Placement schedule selection.
    pub partition_strategy: PartitionStrategy,
    /// How pipeline registers are distributed across each crossing's route.
    pub scheme: PipelineScheme,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            usage_limit: DEFAULT_USAGE_LIMIT,
            max_seconds: 600,
            threads: 1,
            partition_strategy: PartitionStrategy::Auto,
            scheme: PipelineScheme::Double,
        }
    }
}

impl PlanOptions {
    /// Validate public planner inputs before device lookup or solver launch.
    pub fn validate(&self) -> Result<(), PlanOptionsError> {
        if !self.usage_limit.is_finite() || self.usage_limit <= 0.0 || self.usage_limit > 1.0 {
            return Err(PlanOptionsError::UsageLimit(self.usage_limit));
        }
        if self.max_seconds == 0 {
            return Err(PlanOptionsError::MaxSeconds);
        }
        if self.threads == 0 {
            return Err(PlanOptionsError::Threads);
        }
        Ok(())
    }
}

/// Invalid values in [`PlanOptions`].
#[derive(Debug, thiserror::Error)]
pub enum PlanOptionsError {
    /// The utilization target is not a finite fraction in `(0, 1]`.
    #[error("usage limit must be finite and in the range (0, 1], got {0}")]
    UsageLimit(f64),
    /// A zero-second limit can expose an unverified LP relaxation.
    #[error("max seconds must be greater than zero")]
    MaxSeconds,
    /// CBC requires at least one worker thread.
    #[error("solver thread count must be greater than zero")]
    Threads,
}
