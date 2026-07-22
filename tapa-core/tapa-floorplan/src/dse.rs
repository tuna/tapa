//! Deterministic utilization-cap exploration around the exact-cap planner.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::{FloorplanResult, WorkState};

use crate::device::model::{Coor, Device, Resource};
use crate::device::select::select_device;
use crate::partition::ilp::IlpError;
use crate::pipeline::plan::PipelineError;
use crate::route::ilp::RouteError;
use crate::{
    plan_with_inputs_at_usage_limit_and_caps, ExactDseResourceCaps, PlanError, PlanInputs,
    PlanOptions, EXACT_DSE_CAP_SCALE, MULTILEVEL_BLOCK_RESOURCE_MARGIN_UNITS,
};

const ADAPTIVE_MARGIN: u32 = EXACT_DSE_CAP_SCALE / 100;

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
        /// Swept exact limit used for LUT and FF resources.
        logic_utilization_cap: f64,
        /// Effective exact limit used for BRAM18K, DSP, and URAM resources.
        effective_block_utilization_cap: f64,
        /// Whether the multilevel block-resource margin policy was selected.
        multilevel_block_margin_applied: bool,
        /// Largest realized resource fraction over every slot and resource.
        max_utilization: f64,
        /// Complete placement, routing, and pipeline plan.
        floorplan: FloorplanResult,
    },
    /// The exact-cap problem was proven infeasible or exceeded its resource cap.
    Infeasible {
        /// Swept exact limit used for LUT and FF resources.
        logic_utilization_cap: f64,
        /// Effective exact limit used for BRAM18K, DSP, and URAM resources.
        effective_block_utilization_cap: f64,
        /// Whether the multilevel block-resource margin policy was selected.
        multilevel_block_margin_applied: bool,
    },
}

/// Why design-space exploration could not complete its sweep.
#[derive(Debug, thiserror::Error)]
pub enum DseError {
    /// The requested range cannot form a meaningful utilization sweep.
    #[error("invalid DSE options: {0}")]
    InvalidOptions(String),
    /// An exact-cap planning attempt failed without rejecting the candidate.
    #[error(
        "planning DSE candidate at logic utilization cap {logic_utilization_cap} failed: {source}"
    )]
    Plan {
        /// Swept exact logic cap passed to the failed planner invocation.
        logic_utilization_cap: f64,
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
/// other planning errors abort the sweep. Repeated region assignments are
/// omitted while exploration continues toward a cap that may force a new
/// placement.
pub fn explore(
    state: &WorkState,
    plan_options: &PlanOptions,
    inputs: &PlanInputs,
    options: &DseOptions,
) -> Result<Vec<DseCandidate>, DseError> {
    let options = ValidatedOptions::try_from(*options)?;
    let attempts = sweep_with(options, |logic_utilization_cap| {
        let exact_options = PlanOptions {
            usage_limit: logic_utilization_cap,
            ..*plan_options
        };
        let planned = plan_with_inputs_at_usage_limit_and_caps(state, &exact_options, inputs)
            .map_err(|source| DseError::Plan {
                logic_utilization_cap,
                source,
            })?;
        match planned.result {
            Ok(floorplan) => {
                let device = select_device(&floorplan.device).map_err(|source| DseError::Plan {
                    logic_utilization_cap,
                    source: PlanError::from(source),
                })?;
                let realized = realized_utilization(&floorplan, &device)?;
                Ok(Attempt::Feasible {
                    regions: floorplan.regions.clone(),
                    max_utilization: realized.maximum(),
                    binding_logic_cap: realized.binding_logic_cap_units(planned.caps),
                    value: floorplan,
                    metadata: planned.caps,
                })
            }
            Err(error) => match rejection_kind(&error) {
                Some(kind) => Ok(Attempt::Infeasible {
                    kind,
                    metadata: planned.caps,
                }),
                None => Err(DseError::Plan {
                    logic_utilization_cap,
                    source: error,
                }),
            },
        }
    })?;

    Ok(candidates_from_sweep(attempts))
}

fn candidates_from_sweep(
    attempts: Vec<Swept<FloorplanResult, ExactDseResourceCaps>>,
) -> Vec<DseCandidate> {
    attempts
        .into_iter()
        .map(|attempt| {
            let caps = attempt.metadata;
            debug_assert_eq!(
                caps.logic_utilization_cap.to_bits(),
                cap_value(attempt.cap).to_bits(),
                "candidate metadata must describe the swept logic cap",
            );
            match attempt.value {
                Some((floorplan, max_utilization)) => DseCandidate::Feasible {
                    logic_utilization_cap: caps.logic_utilization_cap,
                    effective_block_utilization_cap: caps.effective_block_utilization_cap,
                    multilevel_block_margin_applied: caps.multilevel_block_margin_applied,
                    max_utilization,
                    floorplan,
                },
                None => DseCandidate::Infeasible {
                    logic_utilization_cap: caps.logic_utilization_cap,
                    effective_block_utilization_cap: caps.effective_block_utilization_cap,
                    multilevel_block_margin_applied: caps.multilevel_block_margin_applied,
                },
            }
        })
        .collect()
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
    let units = (value * f64::from(EXACT_DSE_CAP_SCALE)).round() as u32;
    if units == 0 {
        return Err(DseError::InvalidOptions(format!(
            "{name} is below the supported utilization resolution of {}",
            1.0 / f64::from(EXACT_DSE_CAP_SCALE)
        )));
    }
    Ok(units)
}

fn cap_value(units: u32) -> f64 {
    f64::from(units) / f64::from(EXACT_DSE_CAP_SCALE)
}

enum Attempt<T, M> {
    Feasible {
        value: T,
        regions: BTreeMap<String, String>,
        max_utilization: f64,
        binding_logic_cap: u32,
        metadata: M,
    },
    Infeasible {
        kind: RejectionKind,
        metadata: M,
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
struct Swept<T, M> {
    cap: u32,
    value: Option<(T, f64)>,
    metadata: M,
}

fn sweep_with<T, M, E>(
    options: ValidatedOptions,
    mut attempt: impl FnMut(f64) -> Result<Attempt<T, M>, E>,
) -> Result<Vec<Swept<T, M>>, E> {
    let mut cap = options.max;
    let mut visited = BTreeSet::new();
    let mut candidates = Vec::new();

    while cap >= options.min {
        let next_cap = match attempt(cap_value(cap))? {
            Attempt::Feasible {
                value,
                regions,
                max_utilization,
                binding_logic_cap,
                metadata,
            } => {
                if visited.insert(regions) {
                    candidates.push(Swept {
                        cap,
                        value: Some((value, max_utilization)),
                        metadata,
                    });
                }
                let nominal = cap.saturating_sub(options.step);
                let adaptive = binding_logic_cap.saturating_sub(ADAPTIVE_MARGIN);
                nominal.min(adaptive)
            }
            Attempt::Infeasible { kind, metadata } => {
                candidates.push(Swept {
                    cap,
                    value: None,
                    metadata,
                });
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
    (utilization.clamp(0.0, 1.0) * f64::from(EXACT_DSE_CAP_SCALE)).floor() as u32
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
        | PlanError::PlatformMismatch { .. }
        | PlanError::ControlTag { .. } => None,
    }
}

/// Return the largest realized resource fraction over every slot and resource.
///
/// `device` must be the model named by `floorplan.device`.
///
/// # Errors
///
/// Returns [`DseError::InvalidFloorplan`] when a usage entry is not an atomic
/// slot, lies outside the device, consumes an unavailable resource, or exceeds
/// device capacity.
pub fn maximum_realized_utilization(
    floorplan: &FloorplanResult,
    device: &Device,
) -> Result<f64, DseError> {
    realized_utilization(floorplan, device).map(RealizedUtilization::maximum)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct RealizedUtilization {
    logic: f64,
    block: f64,
}

impl RealizedUtilization {
    fn maximum(self) -> f64 {
        self.logic.max(self.block)
    }

    fn binding_logic_cap_units(self, caps: ExactDseResourceCaps) -> u32 {
        let block_margin = if caps.multilevel_block_margin_applied {
            MULTILEVEL_BLOCK_RESOURCE_MARGIN_UNITS
        } else {
            0
        };
        utilization_units(self.logic)
            .max(utilization_units(self.block).saturating_sub(block_margin))
    }
}

fn realized_utilization(
    floorplan: &FloorplanResult,
    device: &Device,
) -> Result<RealizedUtilization, DseError> {
    if floorplan.device != device.key {
        return Err(DseError::InvalidFloorplan(format!(
            "floorplan device `{}` does not match utilization device `{}`",
            floorplan.device, device.key
        )));
    }
    let mut utilization = RealizedUtilization::default();
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
            let ratio = resource_ratio(used, available);
            match resource {
                Resource::Ff | Resource::Lut => {
                    utilization.logic = utilization.logic.max(ratio);
                }
                Resource::Bram18k | Resource::Dsp | Resource::Uram => {
                    utilization.block = utilization.block.max(ratio);
                }
            }
        }
    }
    if utilization.maximum() > 1.0 {
        return Err(DseError::InvalidFloorplan(format!(
            "maximum realized utilization {} exceeds device capacity",
            utilization.maximum()
        )));
    }
    Ok(utilization)
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
    fn candidate_metadata_distinguishes_logic_and_block_caps() {
        let logic_cap = 0.9;
        let multilevel_caps =
            ExactDseResourceCaps::for_strategy(logic_cap, crate::PartitionStrategy::MultiLevel);
        let floorplan = FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::new(),
            routes: Vec::new(),
            slot_usage: BTreeMap::new(),
        };
        let candidates = candidates_from_sweep(vec![
            Swept {
                cap: cap_units("candidate", logic_cap).expect("cap"),
                value: Some((floorplan, 0.75)),
                metadata: multilevel_caps,
            },
            Swept {
                cap: cap_units("candidate", 0.85).expect("cap"),
                value: None,
                metadata: ExactDseResourceCaps::for_strategy(
                    0.85,
                    crate::PartitionStrategy::MultiLevel,
                ),
            },
        ]);

        assert!(matches!(
            candidates[0],
            DseCandidate::Feasible {
                logic_utilization_cap: 0.9,
                effective_block_utilization_cap: 1.0,
                multilevel_block_margin_applied: true,
                max_utilization: 0.75,
                ..
            }
        ));
        assert!(matches!(
            candidates[1],
            DseCandidate::Infeasible {
                logic_utilization_cap: 0.85,
                effective_block_utilization_cap: 0.95,
                multilevel_block_margin_applied: true,
            }
        ));

        let flat_caps = ExactDseResourceCaps::for_strategy(0.9, crate::PartitionStrategy::Flat);
        assert_eq!(
            flat_caps.effective_block_utilization_cap.to_bits(),
            0.9_f64.to_bits(),
            "flat candidates must use one cap for every resource",
        );
        assert!(!flat_caps.multilevel_block_margin_applied);

        let quantized_caps =
            ExactDseResourceCaps::for_strategy(0.57, crate::PartitionStrategy::MultiLevel);
        assert_eq!(
            quantized_caps.effective_block_utilization_cap.to_bits(),
            0.67_f64.to_bits(),
            "the block margin must remain on the fixed-point DSE cap grid",
        );
    }

    #[test]
    fn sweep_is_descending_without_accumulated_float_drift() {
        let mut called = Vec::new();
        let candidates = sweep_with(options(0.6, 0.9, 0.1), |cap| {
            called.push(cap);
            Ok::<_, ()>(Attempt::<(), ()>::Infeasible {
                kind: RejectionKind::Retryable,
                metadata: (),
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
        let candidates = sweep_with(options(0.71, 0.9, 0.05), |cap| {
            called.push(cap);
            Ok::<_, ()>(Attempt::Feasible {
                value: (),
                regions: BTreeMap::from([("task".to_string(), "slot-a".to_string())]),
                max_utilization: 0.72,
                binding_logic_cap: utilization_units(0.72),
                metadata: (),
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
                binding_logic_cap: utilization_units(0.89),
                metadata: (),
            })
        })
        .expect("sweep");
        assert_eq!(called, vec![0.9, 0.8, 0.7]);
    }

    #[test]
    fn duplicate_placement_is_omitted_without_ending_the_sweep() {
        let mut call = 0;
        let mut called = Vec::new();
        let candidates = sweep_with(options(0.75, 0.9, 0.05), |cap| {
            call += 1;
            called.push(cap);
            // Exercise the defensive path where the same published placement
            // has a different routed resource realization at the tighter cap.
            let slot = if call < 3 { "slot-a" } else { "slot-b" };
            let binding_logic_cap = match call {
                1 => 0.86,
                2 => 0.81,
                _ => 0.75,
            };
            Ok::<_, ()>(Attempt::Feasible {
                value: call,
                regions: BTreeMap::from([("task".to_string(), slot.to_string())]),
                max_utilization: binding_logic_cap,
                binding_logic_cap: utilization_units(binding_logic_cap),
                metadata: (),
            })
        })
        .expect("sweep");

        assert_eq!(called, vec![0.9, 0.85, 0.8]);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].value, Some((1, 0.86)));
        assert_eq!(candidates[1].value, Some((3, 0.75)));
    }

    #[test]
    fn multilevel_block_usage_adapts_in_the_logic_cap_domain() {
        let caps = ExactDseResourceCaps::for_strategy(0.9, crate::PartitionStrategy::MultiLevel);
        let block_bound = RealizedUtilization {
            logic: 0.4,
            block: 0.89,
        };
        assert_eq!(
            block_bound.binding_logic_cap_units(caps),
            utilization_units(0.79),
        );

        let flat_caps = ExactDseResourceCaps::for_strategy(0.9, crate::PartitionStrategy::Flat);
        assert_eq!(
            block_bound.binding_logic_cap_units(flat_caps),
            utilization_units(0.89),
        );

        let mut called = Vec::new();
        let candidates = sweep_with(options(0.55, 0.9, 0.05), |cap| {
            called.push(cap);
            let (slot, utilization) = if called.len() == 1 {
                ("slot-a", block_bound)
            } else {
                (
                    "slot-b",
                    RealizedUtilization {
                        logic: 0.55,
                        block: 0.0,
                    },
                )
            };
            Ok::<_, ()>(Attempt::Feasible {
                value: (),
                regions: BTreeMap::from([("task".to_string(), slot.to_string())]),
                max_utilization: utilization.maximum(),
                binding_logic_cap: utilization.binding_logic_cap_units(caps),
                metadata: (),
            })
        })
        .expect("sweep");

        assert_eq!(called, vec![0.9, 0.78]);
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn retryable_rejections_are_recorded_and_callback_errors_abort() {
        let mut call = 0;
        let result = sweep_with(options(0.6, 0.8, 0.1), |_| {
            call += 1;
            if call == 3 {
                return Err("timeout");
            }
            Ok(Attempt::<(), ()>::Infeasible {
                kind: RejectionKind::Retryable,
                metadata: (),
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
            Ok::<_, ()>(Attempt::<(), ()>::Infeasible {
                kind: RejectionKind::Terminal,
                metadata: (),
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
                    dsp: slot.area.dsp * 89 / 100,
                    uram: 0,
                },
            )]),
        };

        let utilization = realized_utilization(&floorplan, &device).expect("valid usage");
        assert!((utilization.logic - 0.5).abs() < f64::EPSILON);
        assert_eq!(
            utilization.block.to_bits(),
            resource_ratio(slot.area.dsp * 89 / 100, slot.area.dsp).to_bits(),
        );
        assert_eq!(
            maximum_realized_utilization(&floorplan, &device)
                .expect("valid usage")
                .to_bits(),
            utilization.maximum().to_bits(),
        );

        let wrong_device = select_device("u250").expect("u250");
        maximum_realized_utilization(&floorplan, &wrong_device)
            .expect_err("mismatched device must fail");
    }
}
