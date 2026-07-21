//! Deterministic utilization-cap exploration around the exact-cap planner.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::{FloorplanResult, WorkState};

use crate::device::model::{Coor, Device, Resource};
use crate::device::select::select_device;
use crate::partition::ilp::IlpError;
use crate::pipeline::plan::PipelineError;
use crate::route::ilp::RouteError;
use crate::{plan_with_inputs_at_usage_limit, PlanError, PlanInputs, PlanOptions};

const CAP_SCALE: u32 = 1_000_000_000;
const ADAPTIVE_MARGIN: u32 = CAP_SCALE / 100;

/// Bounds and spacing for a utilization-cap sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DseOptions {
    /// Lowest cap that may be attempted.
    pub min: f64,
    /// First and highest cap to attempt.
    pub max: f64,
    /// Nominal decrease between attempts.
    pub step: f64,
}

impl Default for DseOptions {
    fn default() -> Self {
        Self {
            min: 0.55,
            max: 0.9,
            step: 0.03,
        }
    }
}

impl DseOptions {
    /// Validate the sweep range before launching a solver.
    pub fn validate(&self) -> Result<(), DseError> {
        ValidatedOptions::try_from(*self).map(|_| ())
    }
}

/// One attempted utilization cap, in descending attempt order.
#[derive(Debug, Clone, PartialEq)]
pub enum DseCandidate {
    /// A unique placement was found at this exact cap.
    Feasible {
        /// Exact cap passed to the planner.
        usage_limit: f64,
        /// Largest realized resource fraction over every slot and resource.
        max_utilization: f64,
        /// Complete placement, routing, and pipeline plan.
        floorplan: FloorplanResult,
    },
    /// The exact-cap problem was proven infeasible or exceeded its resource cap.
    Infeasible {
        /// Exact cap passed to the planner.
        usage_limit: f64,
    },
}

/// Why design-space exploration could not complete its sweep.
#[derive(Debug, thiserror::Error)]
pub enum DseError {
    /// The requested range cannot form a meaningful utilization sweep.
    #[error("invalid DSE options: {0}")]
    InvalidOptions(String),
    /// An exact-cap planning attempt failed without rejecting the candidate.
    #[error("planning DSE candidate at usage limit {usage_limit} failed: {source}")]
    Plan {
        /// Exact cap passed to the failed planner invocation.
        usage_limit: f64,
        /// Underlying planning failure.
        #[source]
        source: PlanError,
    },
    /// A successful planner result could not be measured against its device.
    #[error("invalid floorplan result: {0}")]
    InvalidFloorplan(String),
}

/// Explore exact utilization caps from `options.max` toward `options.min`.
///
/// Rejected attempts remain in the returned sequence. Proven placement
/// infeasibility terminates the descending sweep, while post-placement routing
/// and pipeline-capacity rejections continue because a tighter cap may change
/// the placement. Solver timeouts, missing incumbents, malformed results, and
/// other planning errors abort the sweep. A repeated region assignment
/// terminates exploration before the redundant candidate is returned.
pub fn explore(
    state: &WorkState,
    plan_options: &PlanOptions,
    inputs: &PlanInputs,
    options: &DseOptions,
) -> Result<Vec<DseCandidate>, DseError> {
    let options = ValidatedOptions::try_from(*options)?;
    let attempts = sweep_with(options, |usage_limit| {
        let exact_options = PlanOptions {
            usage_limit,
            ..*plan_options
        };
        match plan_with_inputs_at_usage_limit(state, &exact_options, inputs) {
            Ok(floorplan) => {
                let device = select_device(&floorplan.device).map_err(|source| DseError::Plan {
                    usage_limit,
                    source: PlanError::from(source),
                })?;
                let max_utilization = maximum_realized_utilization(&floorplan, &device)?;
                Ok(Attempt::Feasible {
                    regions: floorplan.regions.clone(),
                    max_utilization,
                    value: floorplan,
                })
            }
            Err(error) => match rejection_kind(&error) {
                Some(kind) => Ok(Attempt::Infeasible { kind }),
                None => Err(DseError::Plan {
                    usage_limit,
                    source: error,
                }),
            },
        }
    })?;

    Ok(attempts
        .into_iter()
        .map(|attempt| match attempt.value {
            Some((floorplan, max_utilization)) => DseCandidate::Feasible {
                usage_limit: cap_value(attempt.cap),
                max_utilization,
                floorplan,
            },
            None => DseCandidate::Infeasible {
                usage_limit: cap_value(attempt.cap),
            },
        })
        .collect())
}

#[derive(Debug, Clone, Copy)]
struct ValidatedOptions {
    min: u32,
    max: u32,
    step: u32,
}

impl TryFrom<DseOptions> for ValidatedOptions {
    type Error = DseError;

    fn try_from(options: DseOptions) -> Result<Self, Self::Error> {
        validate_fraction("minimum", options.min)?;
        validate_fraction("maximum", options.max)?;
        validate_fraction("step", options.step)?;
        if options.min > options.max {
            return Err(DseError::InvalidOptions(format!(
                "minimum {} exceeds maximum {}",
                options.min, options.max
            )));
        }

        Ok(Self {
            min: cap_units("minimum", options.min)?,
            max: cap_units("maximum", options.max)?,
            step: cap_units("step", options.step)?,
        })
    }
}

fn validate_fraction(name: &str, value: f64) -> Result<(), DseError> {
    if !value.is_finite() || value <= 0.0 || value > 1.0 {
        return Err(DseError::InvalidOptions(format!(
            "{name} must be finite and in the range (0, 1], got {value}"
        )));
    }
    Ok(())
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated fractions are rounded onto the bounded fixed-point cap grid"
)]
fn cap_units(name: &str, value: f64) -> Result<u32, DseError> {
    let units = (value * f64::from(CAP_SCALE)).round() as u32;
    if units == 0 {
        return Err(DseError::InvalidOptions(format!(
            "{name} is below the supported utilization resolution of {}",
            1.0 / f64::from(CAP_SCALE)
        )));
    }
    Ok(units)
}

fn cap_value(units: u32) -> f64 {
    f64::from(units) / f64::from(CAP_SCALE)
}

enum Attempt<T> {
    Feasible {
        value: T,
        regions: BTreeMap<String, String>,
        max_utilization: f64,
    },
    Infeasible {
        kind: RejectionKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectionKind {
    /// Placement itself is infeasible, so every tighter cap is redundant.
    Terminal,
    /// A tighter placement may route or account for pipelines differently.
    Retryable,
}

#[derive(Debug)]
struct Swept<T> {
    cap: u32,
    value: Option<(T, f64)>,
}

fn sweep_with<T, E>(
    options: ValidatedOptions,
    mut attempt: impl FnMut(f64) -> Result<Attempt<T>, E>,
) -> Result<Vec<Swept<T>>, E> {
    let mut cap = options.max;
    let mut visited = BTreeSet::new();
    let mut candidates = Vec::new();

    while cap >= options.min {
        let next_cap = match attempt(cap_value(cap))? {
            Attempt::Feasible {
                value,
                regions,
                max_utilization,
            } => {
                if !visited.insert(regions) {
                    break;
                }
                candidates.push(Swept {
                    cap,
                    value: Some((value, max_utilization)),
                });
                let nominal = cap.saturating_sub(options.step);
                let adaptive = utilization_units(max_utilization).saturating_sub(ADAPTIVE_MARGIN);
                nominal.min(adaptive)
            }
            Attempt::Infeasible { kind } => {
                candidates.push(Swept { cap, value: None });
                if kind == RejectionKind::Terminal {
                    break;
                }
                cap.saturating_sub(options.step)
            }
        };
        if next_cap >= cap {
            break;
        }
        cap = next_cap;
    }

    Ok(candidates)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "realized utilization is bounded and conservatively floored onto the cap grid"
)]
fn utilization_units(utilization: f64) -> u32 {
    debug_assert!(
        utilization.is_finite() && (0.0..=1.0).contains(&utilization),
        "realized utilization must be a finite fraction"
    );
    (utilization.clamp(0.0, 1.0) * f64::from(CAP_SCALE)).floor() as u32
}

fn rejection_kind(error: &PlanError) -> Option<RejectionKind> {
    match error {
        PlanError::Ilp(IlpError::Infeasible(_) | IlpError::NoCandidates { .. }) => {
            Some(RejectionKind::Terminal)
        }
        PlanError::Pipeline(
            PipelineError::Route(RouteError::Infeasible | RouteError::CapacityExceeded { .. })
            | PipelineError::RealizedCapacity { .. },
        ) => Some(RejectionKind::Retryable),
        PlanError::Options(_)
        | PlanError::NoPartNum
        | PlanError::Device(_)
        | PlanError::Transform(_)
        | PlanError::Graph(_)
        | PlanError::Ilp(_)
        | PlanError::Pipeline(_)
        | PlanError::BankTag { .. }
        | PlanError::PlatformRequired { .. }
        | PlanError::PlatformMismatch { .. } => None,
    }
}

fn maximum_realized_utilization(
    floorplan: &FloorplanResult,
    device: &Device,
) -> Result<f64, DseError> {
    let mut maximum = 0.0_f64;
    for (region, usage) in &floorplan.slot_usage {
        let coor = Coor::from_region_name(region)
            .filter(|coor| coor.width() == 1 && coor.height() == 1)
            .ok_or_else(|| {
                DseError::InvalidFloorplan(format!("usage key `{region}` is not an atomic slot"))
            })?;
        let slot = device.slot(coor.dl_x, coor.dl_y).ok_or_else(|| {
            DseError::InvalidFloorplan(format!(
                "usage key `{region}` is outside device `{}`",
                device.key
            ))
        })?;
        for resource in Resource::ALL {
            let used = resource.amount(usage);
            let available = resource.amount(&slot.area);
            if available == 0 {
                if used != 0 {
                    return Err(DseError::InvalidFloorplan(format!(
                        "{region} uses {used} {} with zero available",
                        resource.name()
                    )));
                }
                continue;
            }
            maximum = maximum.max(resource_ratio(used, available));
        }
    }
    if maximum > 1.0 {
        return Err(DseError::InvalidFloorplan(format!(
            "maximum realized utilization {maximum} exceeds device capacity"
        )));
    }
    Ok(maximum)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "device resource counts are far below the exact integer range of f64"
)]
fn resource_ratio(used: u64, available: u64) -> f64 {
    used as f64 / available as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::LpStatus;
    use tapa_ir::Area;

    fn options(min: f64, max: f64, step: f64) -> ValidatedOptions {
        ValidatedOptions::try_from(DseOptions { min, max, step }).expect("valid options")
    }

    #[test]
    fn options_reject_invalid_bounds_and_unrepresentable_steps() {
        for value in [0.0, -0.1, 1.01, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                DseOptions {
                    min: value,
                    ..DseOptions::default()
                }
                .validate(),
                Err(DseError::InvalidOptions(_))
            ));
        }
        assert!(matches!(
            DseOptions {
                min: 0.8,
                max: 0.7,
                step: 0.1,
            }
            .validate(),
            Err(DseError::InvalidOptions(_))
        ));
        assert!(matches!(
            DseOptions {
                step: 0.000_000_000_1,
                ..DseOptions::default()
            }
            .validate(),
            Err(DseError::InvalidOptions(_))
        ));
    }

    #[test]
    fn sweep_is_descending_without_accumulated_float_drift() {
        let mut called = Vec::new();
        let candidates = sweep_with(options(0.6, 0.9, 0.1), |cap| {
            called.push(cap);
            Ok::<_, ()>(Attempt::<()>::Infeasible {
                kind: RejectionKind::Retryable,
            })
        })
        .expect("sweep");

        assert_eq!(called, vec![0.9, 0.8, 0.7, 0.6]);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| cap_value(candidate.cap))
                .collect::<Vec<_>>(),
            called
        );
        assert!(candidates.iter().all(|candidate| candidate.value.is_none()));
    }

    #[test]
    fn realized_utilization_skips_loose_caps_only_when_useful() {
        let mut called = Vec::new();
        let candidates = sweep_with(options(0.5, 0.9, 0.05), |cap| {
            called.push(cap);
            Ok::<_, ()>(Attempt::Feasible {
                value: (),
                regions: BTreeMap::from([("task".to_string(), "slot-a".to_string())]),
                max_utilization: 0.72,
            })
        })
        .expect("sweep");

        assert_eq!(called, vec![0.9, 0.71]);
        assert_eq!(candidates.len(), 1, "the duplicate is not returned");

        let mut called = Vec::new();
        let _ = sweep_with(options(0.7, 0.9, 0.1), |cap| {
            called.push(cap);
            Ok::<_, ()>(Attempt::Feasible {
                value: (),
                regions: BTreeMap::from([("task".to_string(), format!("slot-{cap}"))]),
                max_utilization: 0.89,
            })
        })
        .expect("sweep");
        assert_eq!(called, vec![0.9, 0.8, 0.7]);
    }

    #[test]
    fn duplicate_placement_stops_before_redundant_candidate() {
        let mut call = 0;
        let candidates = sweep_with(options(0.5, 0.8, 0.1), |_| {
            call += 1;
            let slot = if call == 2 { "slot-b" } else { "slot-a" };
            Ok::<_, ()>(Attempt::Feasible {
                value: call,
                regions: BTreeMap::from([("task".to_string(), slot.to_string())]),
                max_utilization: 0.8,
            })
        })
        .expect("sweep");

        assert_eq!(call, 3);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].value, Some((1, 0.8)));
        assert_eq!(candidates[1].value, Some((2, 0.8)));
    }

    #[test]
    fn retryable_rejections_are_recorded_and_callback_errors_abort() {
        let mut call = 0;
        let result = sweep_with(options(0.6, 0.8, 0.1), |_| {
            call += 1;
            if call == 3 {
                return Err("timeout");
            }
            Ok(Attempt::<()>::Infeasible {
                kind: RejectionKind::Retryable,
            })
        });
        assert_eq!(result.expect_err("third attempt must abort"), "timeout");
        assert_eq!(call, 3);
    }

    #[test]
    fn terminal_rejection_stops_after_recording_the_proof() {
        let mut call = 0;
        let candidates = sweep_with(options(0.5, 0.8, 0.1), |_| {
            call += 1;
            Ok::<_, ()>(Attempt::<()>::Infeasible {
                kind: RejectionKind::Terminal,
            })
        })
        .expect("sweep");

        assert_eq!(call, 1);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].value.is_none());
    }

    #[test]
    fn rejection_classification_distinguishes_monotone_placement_failures() {
        assert_eq!(
            rejection_kind(&PlanError::Ilp(IlpError::Infeasible(0.7))),
            Some(RejectionKind::Terminal)
        );
        assert_eq!(
            rejection_kind(&PlanError::Ilp(IlpError::NoCandidates {
                vertex: "task".to_string()
            })),
            Some(RejectionKind::Terminal)
        );
        assert_eq!(
            rejection_kind(&PlanError::Pipeline(PipelineError::Route(
                RouteError::Infeasible
            ))),
            Some(RejectionKind::Retryable)
        );
        assert_eq!(
            rejection_kind(&PlanError::Pipeline(PipelineError::Route(
                RouteError::CapacityExceeded { utilization: 1.1 }
            ))),
            Some(RejectionKind::Retryable)
        );
        assert_eq!(
            rejection_kind(&PlanError::Pipeline(PipelineError::RealizedCapacity {
                region: "slot".to_string(),
                resource: "LUT",
                used: 11,
                limit: 10,
            })),
            Some(RejectionKind::Retryable)
        );
        assert_eq!(
            rejection_kind(&PlanError::Ilp(IlpError::NoIncumbent(LpStatus::NotSolved))),
            None
        );
        assert_eq!(
            rejection_kind(&PlanError::Pipeline(PipelineError::Route(
                RouteError::NoIncumbent(LpStatus::NotSolved)
            ))),
            None
        );
    }

    #[test]
    fn realized_utilization_uses_every_slot_and_resource() {
        let device = select_device("u280").expect("u280");
        let slot = &device.slots[0];
        let region = slot.coor().region_name();
        let floorplan = FloorplanResult {
            device: device.key.clone(),
            grid: (device.cols, device.rows),
            regions: BTreeMap::new(),
            routes: Vec::new(),
            slot_usage: BTreeMap::from([(
                region,
                Area {
                    lut: slot.area.lut / 2,
                    ff: slot.area.ff / 4,
                    bram_18k: 0,
                    dsp: 0,
                    uram: 0,
                },
            )]),
        };

        let utilization = maximum_realized_utilization(&floorplan, &device).expect("valid usage");
        assert!((utilization - 0.5).abs() < f64::EPSILON);
    }
}
