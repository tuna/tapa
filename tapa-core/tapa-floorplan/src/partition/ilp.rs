//! Edge-based placement ILP and partition schedule.
//!
//! Each partition iteration uses the same formulation:
//!
//! * sparse `x[v][s]` binaries for a vertex's legal candidate regions;
//! * sparse `y[e][a][b]` binaries for the Cartesian product of the source and
//!   destination candidate regions;
//! * endpoint-coupling, resource, and guillotine-cut constraints; and
//! * width-weighted, vertically penalized centroid distance as the sole
//!   variable objective cost.
//!
//! After a successful solve, the achieved objective is pinned and a unique
//! stable candidate ranking is minimized, so equal-cost optima resolve
//! deterministically across solver versions.
//!
//! Multilevel placement merely changes the candidate regions: the first pass
//! assigns vertices to full-width rows, and the second jointly refines every
//! row into atomic column slots while retaining parent containment. If that
//! aggregate row assignment cannot be decomposed, the atomic iteration is
//! retried without the provisional parent restriction.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::Area;

use crate::device::model::{Coor, Device, Resource, DEFAULT_USAGE_LIMIT, VERTICAL_DIST_PENALTY};
use crate::graph::{FloorGraph, PlacementEdge};
use crate::partition::cut::{find_cuts_for_regions, Cut};
use crate::solver::assign::{add_one_of_k_row, read_one_of_k, OneOfKError};
use crate::solver::sparse::SparseRow;
use crate::solver::{
    Comparison, LinExpr, LpModel, LpStatus, LpVar, Sense, SolveOpts, Solver, SolverError,
};
use crate::ExactInt;

/// The retry envelope for partitioning.
const USAGE_LIMIT_STEP: f64 = 0.02;
pub(crate) const MAX_USAGE_LIMIT: f64 = 0.95;
/// Below this unique placement-edge count the automatic schedule keeps one
/// flat ILP.
const FLAT_SCHEDULE_MAX_EDGE_COUNT: usize = 300;
/// Above this unique placement-edge count the automatic schedule always
/// refines multilevel.
const MULTILEVEL_SCHEDULE_EDGE_COUNT: usize = 800;
/// Between the edge-count thresholds the automatic schedule refines
/// multilevel once the device has at least this many rows.
const MULTILEVEL_SCHEDULE_MIN_ROWS: u32 = 3;

/// How the device is subdivided into placement iterations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PartitionStrategy {
    /// Apply the built-in edge-count/device-size selection heuristic.
    #[default]
    Auto,
    /// Assign directly to all atomic slots in one ILP.
    Flat,
    /// First assign to full-width rows, then refine all rows jointly by column.
    MultiLevel,
}

/// One partition iteration's regions and their summed slot capacities.
///
/// Every rung of the utilization ladder filters candidates and builds resource
/// rows over the same regions, so the areas are resolved once per iteration
/// rather than once per (vertex, region, rung).
type RegionAreas = BTreeMap<Coor, Area>;

/// Per-region, per-resource fractional limits.  Region names use
/// `SLOT_X..._TO_SLOT_X...`.
type RegionResourceLimits = BTreeMap<String, BTreeMap<Resource, f64>>;

/// Constraints that narrow candidate domains or override resource limits.
///
/// `vertex_regions` uses overlap semantics: a region is a candidate during an
/// iteration when it overlaps the target. Parent
/// containment and exact external-terminal placement are added by the
/// schedule itself, so none of these restrictions require extra ILP rows.
#[derive(Debug, Clone, Default)]
struct PlacementConstraints {
    vertex_regions: BTreeMap<String, Coor>,
    max_resource_limits: RegionResourceLimits,
}

/// Configuration for [`floorplan_with_config`].
#[derive(Debug, Clone)]
struct PlacementConfig {
    usage_limit: f64,
    retry_ceiling: f64,
    strategy: PartitionStrategy,
    constraints: PlacementConstraints,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            usage_limit: DEFAULT_USAGE_LIMIT,
            retry_ceiling: MAX_USAGE_LIMIT,
            strategy: PartitionStrategy::Auto,
            constraints: PlacementConstraints::default(),
        }
    }
}

/// A completed atomic placement and the resources used in occupied slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// Vertex name → atomic region tag.
    pub regions: BTreeMap<String, String>,
    /// Atomic region tag → summed resource use.
    pub slot_usage: BTreeMap<String, Area>,
}

/// Why the floorplan ILP produced no placement.
#[derive(Debug, thiserror::Error)]
pub enum IlpError {
    /// The solver failed to run, parse, or validate its output.
    #[error(transparent)]
    Solver(#[from] SolverError),
    /// No feasible placement was found within the utilization retry envelope.
    #[error("floorplan is infeasible up to usage limit {limit}: {demand}")]
    Infeasible {
        /// The highest derating that was attempted.
        limit: f64,
        /// What the design asks of the device at that derating.
        demand: String,
    },
    /// The solver neither proved infeasibility nor returned a usable
    /// incumbent. Increasing utilization would hide this failure.
    #[error("floorplan solver returned no usable incumbent ({0:?})")]
    NoIncumbent(LpStatus),
    /// A utilization fraction was not finite or outside `(0, 1]`.
    #[error("invalid {kind} utilization limit {value} for {region}: expected {range}")]
    InvalidLimit {
        kind: &'static str,
        region: String,
        value: f64,
        range: &'static str,
    },
    /// Candidate filtering left a vertex with nowhere legal to go.
    #[error("vertex `{vertex}` has no feasible candidate region")]
    NoCandidates { vertex: String },
    /// A vertex is anchored to a tag whose slot cannot hold it. Raising the
    /// utilization limit cannot help — the anchor admits exactly one region —
    /// so this is reported instead of being retried into a generic
    /// infeasibility. Preformatted rather than structured to keep the error
    /// enum small enough for `clippy::result_large_err`.
    #[error("{0}")]
    AnchorUnplaceable(String),
    /// A partition region refers to cells absent from the device model.
    #[error("partition region `{0}` is outside the device model")]
    InvalidRegion(String),
    /// Solver values did not encode exactly one integral assignment.
    #[error("invalid floorplan solver assignment: {0}")]
    InvalidSolution(String),
}

/// Plan with an explicit schedule and utilization retry ceiling.
pub(crate) fn floorplan_with_strategy(
    graph: &FloorGraph,
    device: &Device,
    base_usage_limit: f64,
    retry_ceiling: f64,
    strategy: PartitionStrategy,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Assignment, IlpError> {
    floorplan_with_config(
        graph,
        device,
        &PlacementConfig {
            usage_limit: base_usage_limit,
            retry_ceiling,
            strategy,
            constraints: PlacementConstraints::default(),
        },
        solver,
        opts,
    )
}

/// Plan one exact-cap candidate, applying separate logic and block-resource
/// limits only when the requested schedule resolves to multilevel placement.
///
/// The resource overrides change only the right-hand sides of the existing
/// per-region resource rows, and candidate filtering honors those same limits.
/// Flat placement retains the ordinary single-cap formulation.
pub(crate) fn floorplan_with_exact_resource_caps(
    graph: &FloorGraph,
    device: &Device,
    logic_limit: f64,
    block_limit: f64,
    strategy: PartitionStrategy,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<(Assignment, PartitionStrategy), IlpError> {
    let strategy = resolve_strategy(graph, device, strategy);
    let constraints = if strategy == PartitionStrategy::MultiLevel {
        exact_resource_cap_constraints(device, block_limit)
    } else {
        PlacementConstraints::default()
    };
    let assignment = floorplan_with_config(
        graph,
        device,
        &PlacementConfig {
            usage_limit: logic_limit,
            retry_ceiling: logic_limit,
            strategy,
            constraints,
        },
        solver,
        opts,
    )?;
    Ok((assignment, strategy))
}

fn exact_resource_cap_constraints(device: &Device, block_limit: f64) -> PlacementConstraints {
    let by_resource: BTreeMap<_, _> = Resource::ALL
        .into_iter()
        .filter(|resource| !resource.is_logic())
        .map(|resource| (resource, block_limit))
        .collect();
    let max_resource_limits = row_regions(device)
        .into_iter()
        .chain(atomic_regions(device))
        .map(|region| (region.region_name(), by_resource.clone()))
        .collect();
    PlacementConstraints {
        max_resource_limits,
        ..PlacementConstraints::default()
    }
}

/// Plan using an explicit schedule and optional pin/resource constraints.
fn floorplan_with_config(
    graph: &FloorGraph,
    device: &Device,
    config: &PlacementConfig,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Assignment, IlpError> {
    validate_config(config)?;
    let strategy = resolve_strategy(graph, device, config.strategy);

    let final_regions = match strategy {
        PartitionStrategy::Auto => unreachable!("auto strategy was resolved"),
        PartitionStrategy::Flat => {
            let slots = atomic_regions(device);
            solve_iteration(
                graph,
                device,
                &slots,
                None,
                config,
                config.usage_limit,
                solver,
                opts,
            )?
            .placements
        }
        PartitionStrategy::MultiLevel => {
            let rows = row_regions(device);
            let row_pass = solve_iteration(
                graph,
                device,
                &rows,
                None,
                config,
                config.usage_limit,
                solver,
                opts,
            )?;
            // The row pass is a relaxation of the atomic pass at the same
            // limit: a row's capacity is the sum of its slots', every atomic
            // assignment induces a row assignment, and the horizontal cuts are
            // the same rows over the same boundaries. So a rung the rows could
            // not place is one the slots cannot place either, and the atomic
            // search starts where the rows landed instead of re-proving them.
            let atomic_base = row_pass.usage_limit;
            let slots = atomic_regions(device);
            match solve_iteration(
                graph,
                device,
                &slots,
                Some(&row_pass.placements),
                config,
                atomic_base,
                solver,
                opts,
            ) {
                Ok(assignment) => assignment.placements,
                Err(IlpError::Infeasible { .. } | IlpError::NoCandidates { .. }) => {
                    solve_iteration(
                        graph,
                        device,
                        &slots,
                        None,
                        config,
                        atomic_base,
                        solver,
                        opts,
                    )?
                    .placements
                }
                Err(error) => return Err(error),
            }
        }
    };

    complete_assignment(graph, &final_regions)
}

pub(crate) fn resolve_strategy(
    graph: &FloorGraph,
    device: &Device,
    requested: PartitionStrategy,
) -> PartitionStrategy {
    match requested {
        PartitionStrategy::Auto => select_strategy(device, graph.placement_edges().len()),
        explicit @ (PartitionStrategy::Flat | PartitionStrategy::MultiLevel) => explicit,
    }
}

/// Automatic schedule heuristic, including its row-count test for medium-sized
/// designs.
#[must_use]
pub fn select_strategy(device: &Device, edge_count: usize) -> PartitionStrategy {
    if device.cols == 1 || edge_count < FLAT_SCHEDULE_MAX_EDGE_COUNT {
        return PartitionStrategy::Flat;
    }
    if edge_count > MULTILEVEL_SCHEDULE_EDGE_COUNT {
        return PartitionStrategy::MultiLevel;
    }
    if device.rows >= MULTILEVEL_SCHEDULE_MIN_ROWS {
        PartitionStrategy::MultiLevel
    } else {
        PartitionStrategy::Flat
    }
}

fn validate_config(config: &PlacementConfig) -> Result<(), IlpError> {
    validate_limit("base", "all regions", config.usage_limit, false)?;
    validate_limit("retry ceiling", "all regions", config.retry_ceiling, false)?;
    if config.retry_ceiling < config.usage_limit {
        return Err(IlpError::InvalidLimit {
            kind: "retry ceiling",
            region: "all regions".to_string(),
            value: config.retry_ceiling,
            range: "base limit <= ceiling <= 1",
        });
    }
    for (region, by_resource) in &config.constraints.max_resource_limits {
        for &value in by_resource.values() {
            validate_limit("maximum", region, value, true)?;
        }
    }
    Ok(())
}

fn validate_limit(
    kind: &'static str,
    region: &str,
    value: f64,
    allow_zero: bool,
) -> Result<(), IlpError> {
    let lower_bound_ok = value > 0.0 || (allow_zero && value >= 0.0);
    if value.is_finite() && lower_bound_ok && value <= 1.0 {
        Ok(())
    } else {
        Err(IlpError::InvalidLimit {
            kind,
            region: region.to_string(),
            value,
            range: if allow_zero {
                "0 <= limit <= 1"
            } else {
                "0 < limit <= 1"
            },
        })
    }
}

/// Resolve every region's capacity once, failing on a region the device model
/// does not cover.
fn region_areas(device: &Device, regions: &[Coor]) -> Result<RegionAreas, IlpError> {
    regions
        .iter()
        .map(|region| {
            let area = device
                .island_area(region)
                .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
            Ok((*region, area))
        })
        .collect()
}

fn atomic_regions(device: &Device) -> Vec<Coor> {
    let mut regions = Vec::with_capacity((device.rows * device.cols) as usize);
    // Candidate ordering is x-major, then y-minor.
    for x in 0..device.cols {
        for y in 0..device.rows {
            if device.slot(x, y).is_some() {
                regions.push(Coor::slot(x, y));
            }
        }
    }
    regions
}

fn row_regions(device: &Device) -> Vec<Coor> {
    // `Device::validate` rejects a zero-width grid before a device reaches the
    // planner, so `cols - 1` is the rightmost column, never an underflow.
    (0..device.rows)
        .map(|y| Coor::span(0, y, device.cols - 1, y))
        .collect()
}

/// A completed partition iteration and the rung of the utilization ladder it
/// was found on.
struct SolvedIteration {
    placements: BTreeMap<String, Coor>,
    /// The utilization limit this placement was found at. Every lower rung was
    /// proven infeasible for this iteration.
    usage_limit: f64,
}

/// A rung that placed, kept whole so the ladder search can refine the rung it
/// settles on instead of refining every rung it probes.
struct FeasibleRung {
    usage_limit: f64,
    model: FloorplanModel,
    domains: Vec<Vec<Coor>>,
    solution: crate::solver::LpSolution,
}

/// What one rung of the utilization ladder produced.
enum Rung {
    Placed(Box<FeasibleRung>),
    /// Nothing places at this derating. Carries the error to surface if even
    /// the ceiling turns out to be infeasible, so a permanent pin, parent, or
    /// memory-domain conflict is still reported as itself.
    Infeasible(IlpError),
}

/// The utilization ladder from `base` up to `ceiling` inclusive, in steps of
/// [`USAGE_LIMIT_STEP`] with the ceiling always the last rung.
fn usage_ladder(base: f64, ceiling: f64) -> Vec<f64> {
    let mut rungs = vec![base];
    // Step by repeated addition rather than by multiplying an index, so every
    // rung is bit-identical to what walking the ladder one step at a time
    // produced. The ceiling is the last rung and terminates the loop.
    #[allow(
        clippy::while_float,
        reason = "the loop stops at the ceiling, which `min` makes exactly reachable"
    )]
    while *rungs.last().expect("the base rung is always present") < ceiling {
        let next = (rungs.last().expect("non-empty") + USAGE_LIMIT_STEP).min(ceiling);
        rungs.push(next);
    }
    rungs
}

/// Run one partition iteration, searching the utilization ladder for the
/// lowest derating that places.
///
/// Feasibility is monotone in the limit: raising it only widens every candidate
/// domain and relaxes every resource row, leaving the cuts and the objective
/// untouched. So the ladder is a sorted predicate and the lowest rung that
/// places can be searched for rather than walked to.
///
/// The search gallops — base, then one rung up, then two, four, … — before
/// bisecting the bracket it lands in. A design that fits at the requested
/// derating still costs exactly one solve, a design needing one bump costs the
/// same as a linear walk, and a design that only fits near the ceiling costs
/// logarithmically many instead of one solve per rung.
#[allow(
    clippy::too_many_arguments,
    reason = "one iteration needs its graph, device, regions, parent restriction, config, base rung, and solver explicitly"
)]
fn solve_iteration(
    graph: &FloorGraph,
    device: &Device,
    regions: &[Coor],
    parents: Option<&BTreeMap<String, Coor>>,
    config: &PlacementConfig,
    base_usage_limit: f64,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<SolvedIteration, IlpError> {
    // Cuts depend only on the iteration's regions, not on the derating being
    // searched; compute them once.
    let cuts = find_cuts_for_regions(device, regions);
    let areas = region_areas(device, regions)?;
    let ladder = usage_ladder(base_usage_limit, config.retry_ceiling);
    let probe = |rung: usize| {
        attempt_placement(
            graph,
            device,
            regions,
            &areas,
            parents,
            config,
            &cuts,
            ladder[rung],
            solver,
            opts,
        )
    };

    // The base rung, then the bracket the gallop lands in.
    let mut infeasible = match probe(0)? {
        Rung::Placed(found) => return finish_iteration(graph, *found, solver, opts),
        Rung::Infeasible(reason) => {
            if ladder.len() == 1 {
                return Err(reason);
            }
            0
        }
    };
    let last = ladder.len() - 1;
    let mut stride = 1;
    let (mut feasible, mut placed) = loop {
        let rung = (infeasible + stride).min(last);
        match probe(rung)? {
            Rung::Placed(found) => break (rung, found),
            Rung::Infeasible(reason) if rung == last => return Err(reason),
            Rung::Infeasible(_) => {
                infeasible = rung;
                stride *= 2;
            }
        }
    };

    // Bisect the half-open bracket `(infeasible, feasible]`.
    while feasible - infeasible > 1 {
        let rung = infeasible + (feasible - infeasible) / 2;
        match probe(rung)? {
            Rung::Placed(found) => {
                feasible = rung;
                placed = found;
            }
            Rung::Infeasible(_) => infeasible = rung,
        }
    }
    finish_iteration(graph, *placed, solver, opts)
}

/// Refine the rung the ladder settled on and read its placement back.
///
/// The lexicographic tie-break runs once, here, rather than at every probed
/// rung: pin the achieved objective, then minimize a stable candidate ranking
/// so translation-equivalent optima resolve identically across solver versions.
fn finish_iteration(
    graph: &FloorGraph,
    mut rung: FeasibleRung,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<SolvedIteration, IlpError> {
    log::info!("placement succeeded at usage limit {:.2}", rung.usage_limit);
    let refined = rung
        .model
        .refine_lexicographic(solver, opts, &rung.solution)?;
    if refined.is_none() {
        log::warn!(
            "the placement's lexicographic refinement found no incumbent; this plan is still \
             valid but may not reproduce across solver versions",
        );
    }
    let placements = rung.model.read_back(
        graph,
        &rung.domains,
        refined.as_ref().unwrap_or(&rung.solution),
    )?;
    Ok(SolvedIteration {
        placements,
        usage_limit: rung.usage_limit,
    })
}

/// Try to place at exactly `usage_limit`, without the lexicographic refinement
/// the settled rung gets.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors solve_iteration's inputs plus the rung being probed"
)]
fn attempt_placement(
    graph: &FloorGraph,
    device: &Device,
    regions: &[Coor],
    areas: &RegionAreas,
    parents: Option<&BTreeMap<String, Coor>>,
    config: &PlacementConfig,
    cuts: &[Cut],
    usage_limit: f64,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Rung, IlpError> {
    let domains = match candidate_domains(
        graph,
        device,
        regions,
        areas,
        usage_limit,
        parents,
        &config.constraints,
    ) {
        Ok(domains) => domains,
        // An empty domain may be a derating that is merely too strict, or a
        // permanent pin, parent, or memory conflict. Either way nothing places
        // here; the ladder search decides which by asking the ceiling.
        Err(error @ IlpError::NoCandidates { .. }) => return Ok(Rung::Infeasible(error)),
        Err(error) => return Err(error),
    };

    let model = FloorplanModel::build(
        graph,
        device,
        &domains,
        areas,
        cuts,
        usage_limit,
        &config.constraints,
    )?;
    let solution = solver.solve(&model.lp, opts)?;
    if solution.is_found() {
        return Ok(Rung::Placed(Box::new(FeasibleRung {
            usage_limit,
            model,
            domains,
            solution,
        })));
    }

    match solution.status {
        LpStatus::Infeasible => {
            log::info!("placement infeasible at usage limit {usage_limit:.2}");
            Ok(Rung::Infeasible(IlpError::Infeasible {
                limit: config.retry_ceiling,
                demand: describe_demand(graph, areas, config.retry_ceiling, &config.constraints),
            }))
        }
        LpStatus::NotSolved | LpStatus::Unbounded => Err(IlpError::NoIncumbent(solution.status)),
        LpStatus::Optimal | LpStatus::Feasible => {
            unreachable!("found incumbents returned during readback")
        }
    }
}

/// Name the resource the design asks most of, so an infeasible placement says
/// which wall it hit rather than only that it hit one.
///
/// The iteration's regions tile the device, so summing their derated areas is
/// the whole budget at `usage_limit`. A resource over budget is a definitive
/// explanation; if none is, the binding constraint is the wire-capacity cuts or
/// per-slot packing, and the message says so instead of inventing a cause.
fn describe_demand(
    graph: &FloorGraph,
    areas: &RegionAreas,
    usage_limit: f64,
    constraints: &PlacementConstraints,
) -> String {
    let mut worst: Option<(Resource, u64, u64)> = None;
    for resource in Resource::ALL {
        let demand: u64 = graph
            .vertices()
            .iter()
            .map(|vertex| resource.amount(&vertex.area))
            .sum();
        let supply: u64 = areas
            .iter()
            .map(|(region, total)| {
                let limit = lookup_limit(&constraints.max_resource_limits, region, resource)
                    .unwrap_or(usage_limit);
                scaled_amount(resource.amount(total), limit)
            })
            .sum();
        if demand > supply
            && worst.is_none_or(|(_, worst_demand, worst_supply)| {
                // Compare `demand / supply` without dividing by a zero supply.
                demand.saturating_mul(worst_supply) > worst_demand.saturating_mul(supply)
            })
        {
            worst = Some((resource, demand, supply));
        }
    }

    match worst {
        Some((resource, demand, supply)) => format!(
            "the design needs {demand} {} against {supply} available at that limit",
            resource.name(),
        ),
        None => "every resource fits, so the binding constraint is inter-slot wire capacity or \
                 per-slot packing rather than the utilization limit"
            .to_string(),
    }
}

/// Legal region lists for every vertex.  All placement restrictions are
/// represented here, before variables are allocated.
fn candidate_domains(
    graph: &FloorGraph,
    device: &Device,
    regions: &[Coor],
    areas: &RegionAreas,
    usage_limit: f64,
    parents: Option<&BTreeMap<String, Coor>>,
    constraints: &PlacementConstraints,
) -> Result<Vec<Vec<Coor>>, IlpError> {
    let mut domains = Vec::with_capacity(graph.vertices().len());

    for vertex in graph.vertices() {
        let parent = parents.and_then(|assignments| assignments.get(&vertex.name));
        let target = constraints.vertex_regions.get(&vertex.name);
        let mut candidates = Vec::new();

        for &region in regions {
            if let Some(parent) = parent {
                if !region.is_inside(parent) {
                    continue;
                }
            }
            if let Some(target) = target {
                if !region.has_overlap(target) {
                    continue;
                }
            }
            if let Some(tag) = vertex.required_tag.as_deref() {
                if !region_has_tag(device, &region, tag) {
                    continue;
                }
            }

            let total = areas
                .get(&region)
                .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
            let available = scaled_area_with_overrides(
                *total,
                usage_limit,
                &constraints.max_resource_limits,
                &region,
            );
            // A zero-area vertex fits even on a slot whose derated area
            // scaled to zero (0 <= 0); only the fitting check decides.
            if !area_fits(vertex.area, available) {
                continue;
            }
            candidates.push(region);
        }

        candidates.sort_unstable();
        candidates.dedup();
        if candidates.is_empty() {
            // An anchored vertex has one admissible region, so "nowhere to go"
            // has a single cause worth naming. Without this the failure is
            // retried at a higher limit and finally reported as a generic
            // infeasibility that blames wire capacity.
            if let Some(tag) = vertex.required_tag.as_deref() {
                let tagged: Vec<Coor> = regions
                    .iter()
                    .copied()
                    .filter(|region| region_has_tag(device, region, tag))
                    .collect();
                let available = tagged
                    .first()
                    .and_then(|region| areas.get(region).map(|total| (region, total)))
                    .map(|(region, total)| {
                        scaled_area_with_overrides(
                            *total,
                            usage_limit,
                            &constraints.max_resource_limits,
                            region,
                        )
                    })
                    .unwrap_or_default();
                let where_ = if tagged.is_empty() {
                    "no region".to_string()
                } else {
                    tagged
                        .iter()
                        .map(Coor::region_name)
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                return Err(IlpError::AnchorUnplaceable(format!(
                    "`{}` is anchored to `{tag}`, which selects {where_}, but it needs {} \
                     and only {} is available there; the device table must give that slot \
                     the resources the platform leaves it, or tag the slot that borders \
                     the interface instead",
                    vertex.name,
                    describe_area(vertex.area),
                    describe_area(available),
                )));
            }
            return Err(IlpError::NoCandidates {
                vertex: vertex.name.clone(),
            });
        }
        domains.push(candidates);
    }

    Ok(domains)
}

fn region_has_tag(device: &Device, region: &Coor, required: &str) -> bool {
    region.all_slot_coors().into_iter().any(|(x, y)| {
        device
            .slot(x, y)
            .is_some_and(|slot| slot.tags.iter().any(|tag| tag == required))
    })
}

pub(crate) fn scaled_area(area: Area, limit: f64) -> Area {
    Area {
        lut: scaled_amount(area.lut, limit),
        ff: scaled_amount(area.ff, limit),
        bram_18k: scaled_amount(area.bram_18k, limit),
        dsp: scaled_amount(area.dsp, limit),
        uram: scaled_amount(area.uram, limit),
    }
}

fn scaled_area_with_overrides(
    area: Area,
    default_limit: f64,
    limits: &RegionResourceLimits,
    region: &Coor,
) -> Area {
    Area::from_amounts(|resource| {
        scaled_amount(
            resource.amount(&area),
            lookup_limit(limits, region, resource).unwrap_or(default_limit),
        )
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the formulation converts the positive scaled resource amount with int(), i.e. floor"
)]
fn scaled_amount(amount: u64, limit: f64) -> u64 {
    (amount as f64 * limit) as u64
}

fn area_fits(required: Area, available: Area) -> bool {
    Resource::ALL
        .into_iter()
        .all(|resource| resource.amount(&required) <= resource.amount(&available))
}

/// An area as a short `LUT=… FF=…` line for an error message, listing only the
/// resources that are non-zero so an all-zero area reads as "nothing".
fn describe_area(area: Area) -> String {
    let parts: Vec<String> = Resource::ALL
        .into_iter()
        .filter(|resource| resource.amount(&area) > 0)
        .map(|resource| format!("{resource:?}={}", resource.amount(&area)))
        .collect();
    if parts.is_empty() {
        "no resources".to_string()
    } else {
        parts.join(" ")
    }
}

fn complete_assignment(
    graph: &FloorGraph,
    placements: &BTreeMap<String, Coor>,
) -> Result<Assignment, IlpError> {
    let mut regions = BTreeMap::new();
    let mut slot_usage: BTreeMap<String, Area> = BTreeMap::new();
    for vertex in graph.vertices() {
        let slot = placements.get(&vertex.name).ok_or_else(|| {
            IlpError::InvalidSolution(format!("vertex `{}` is missing", vertex.name))
        })?;
        if slot.width() != 1 || slot.height() != 1 {
            return Err(IlpError::InvalidSolution(format!(
                "vertex `{}` remained in non-atomic region `{}`",
                vertex.name,
                slot.region_name()
            )));
        }
        let region = slot.region_name();
        if regions
            .insert(vertex.name.clone(), region.clone())
            .is_some()
        {
            return Err(IlpError::InvalidSolution(format!(
                "duplicate vertex name `{}`",
                vertex.name
            )));
        }
        if vertex.materialize {
            let entry = slot_usage.entry(region).or_default();
            *entry = entry.checked_add(vertex.area).ok_or_else(|| {
                IlpError::InvalidSolution(format!(
                    "resource accounting overflows in `{}`",
                    slot.region_name()
                ))
            })?;
        }
    }
    Ok(Assignment {
        regions,
        slot_usage,
    })
}

/// A built iteration's LP plus sparse assignment-variable handles.
struct FloorplanModel {
    lp: LpModel,
    /// `x[vertex][candidate_index]`.
    x: Vec<Vec<LpVar>>,
}

impl FloorplanModel {
    #[allow(
        clippy::too_many_arguments,
        reason = "one iteration's model needs its graph, device, domains, region areas, cuts, derating, and overrides"
    )]
    fn build(
        graph: &FloorGraph,
        device: &Device,
        domains: &[Vec<Coor>],
        areas: &RegionAreas,
        cuts: &[Cut],
        usage_limit: f64,
        constraints: &PlacementConstraints,
    ) -> Result<Self, IlpError> {
        debug_assert_eq!(
            graph.vertices().len(),
            domains.len(),
            "every graph vertex needs one sparse candidate row"
        );
        let mut lp = LpModel::new(Sense::Minimize);
        let x = add_assignment_vars(&mut lp, graph, domains);
        let y = add_edge_vars(&mut lp, graph, domains);
        add_coupling(&mut lp, graph, domains, &x, &y);
        add_resource_constraints(
            &mut lp,
            graph,
            areas,
            domains,
            usage_limit,
            constraints,
            &x,
            &y,
        )?;
        add_cut_constraints(&mut lp, graph, domains, cuts, &y);
        add_objective(&mut lp, graph, device, domains, &y)?;
        Ok(Self { lp, x })
    }

    fn read_back(
        &self,
        graph: &FloorGraph,
        domains: &[Vec<Coor>],
        solution: &crate::solver::LpSolution,
    ) -> Result<BTreeMap<String, Coor>, IlpError> {
        let mut assignments = BTreeMap::new();

        for (vi, vertex) in graph.vertices().iter().enumerate() {
            let selected = read_one_of_k(solution, &self.x[vi]).map_err(|error| match error {
                OneOfKError::MissingVariable { position } => IlpError::InvalidSolution(format!(
                    "solver result omitted assignment variable x{} for vertex `{}`",
                    self.x[vi][position].0, vertex.name
                )),
                OneOfKError::NonBinary { values, .. } => IlpError::InvalidSolution(format!(
                    "vertex `{}` has non-binary values {values:?}",
                    vertex.name
                )),
                OneOfKError::SelectionCount { values, .. } => IlpError::InvalidSolution(format!(
                    "vertex `{}` must select exactly one candidate, got {values:?}",
                    vertex.name
                )),
            })?;
            if assignments
                .insert(vertex.name.clone(), domains[vi][selected])
                .is_some()
            {
                return Err(IlpError::InvalidSolution(format!(
                    "duplicate vertex name `{}`",
                    vertex.name
                )));
            }
        }

        Ok(assignments)
    }
}

impl FloorplanModel {
    /// Pin the achieved primary objective and re-solve minimizing the stable
    /// candidate ranking (`Σ candidate_index·x`), whose optimum is unique:
    /// each vertex independently selects its lowest-ranked feasible
    /// candidate, and y is then forced by the coupling rows. This makes
    /// equal-cost placements deterministic across solver versions. Falls
    /// back to the primary incumbent when the refinement yields none.
    fn refine_lexicographic(
        &mut self,
        solver: &dyn Solver,
        opts: &SolveOpts,
        primary: &crate::solver::LpSolution,
    ) -> Result<Option<crate::solver::LpSolution>, IlpError> {
        let pin = self.lp.objective.clone();
        Ok(crate::solver::lexicographic::refine(
            &mut self.lp,
            solver,
            opts,
            pin,
            primary.objective,
            &self.x,
        )?)
    }
}

/// Sparse `x[v][s]` binaries and redundant one-hot rows.
fn add_assignment_vars(
    lp: &mut LpModel,
    graph: &FloorGraph,
    domains: &[Vec<Coor>],
) -> Vec<Vec<LpVar>> {
    graph
        .vertices()
        .iter()
        .enumerate()
        .map(|(vi, vertex)| {
            add_one_of_k_row(
                lp,
                &format!("vertex_{}", vertex.name),
                domains[vi].len(),
                |candidate| format!("x_{}_{}", vertex.name, domains[vi][candidate].region_name()),
            )
        })
        .collect()
}

/// One sparse edge-route term with its endpoint candidates.
#[derive(Debug, Clone, Copy)]
struct YTerm<'a> {
    /// Placement-edge index.
    ei: usize,
    /// The placement edge.
    edge: &'a PlacementEdge,
    /// Source candidate index and region.
    src_ci: usize,
    src: &'a Coor,
    /// Destination candidate index and region.
    dst_ci: usize,
    dst: &'a Coor,
}

impl YTerm<'_> {
    /// The allocated plane variable of this term.
    fn var(self, y: &[Vec<Vec<LpVar>>]) -> LpVar {
        y[self.ei][self.src_ci][self.dst_ci]
    }
}

/// Enumerate one placement edge's sparse `y[src][dst]` terms in build
/// order: source candidate major, destination candidate minor. This is the
/// single definition of the plane's enumeration order; every y loop
/// delegates here so model construction cannot drift between call sites.
fn edge_y_terms<'a>(
    ei: usize,
    edge: &'a PlacementEdge,
    domains: &'a [Vec<Coor>],
) -> impl Iterator<Item = YTerm<'a>> + 'a {
    domains[edge.src]
        .iter()
        .enumerate()
        .flat_map(move |(src_ci, src)| {
            domains[edge.dst]
                .iter()
                .enumerate()
                .map(move |(dst_ci, dst)| YTerm {
                    ei,
                    edge,
                    src_ci,
                    src,
                    dst_ci,
                    dst,
                })
        })
}

/// Enumerate every sparse edge-route term in model-build order: placement
/// edge, then source candidate, then destination candidate.
fn y_terms<'a>(
    graph: &'a FloorGraph,
    domains: &'a [Vec<Coor>],
) -> impl Iterator<Item = YTerm<'a>> + 'a {
    graph
        .placement_edges()
        .iter()
        .enumerate()
        .flat_map(move |(ei, edge)| edge_y_terms(ei, edge, domains))
}

/// Sparse `y[e][src_candidate][dst_candidate]` binaries.
fn add_edge_vars(
    lp: &mut LpModel,
    graph: &FloorGraph,
    domains: &[Vec<Coor>],
) -> Vec<Vec<Vec<LpVar>>> {
    graph
        .placement_edges()
        .iter()
        .enumerate()
        .map(|(ei, edge)| {
            let mut plane: Vec<Vec<LpVar>> = vec![Vec::new(); domains[edge.src].len()];
            for YTerm { src_ci, dst_ci, .. } in edge_y_terms(ei, edge, domains) {
                plane[src_ci].push(lp.add_binary(format!("y_{ei}_{src_ci}_{dst_ci}")));
            }
            lp.add_constraint(
                format!("route_{ei}"),
                LinExpr::sum(plane.iter().flatten().map(|&var| (1.0, var))),
                Comparison::Eq,
                1.0,
            );
            plane
        })
        .collect()
}

/// Couple every edge route to the sparse source and destination one-hot rows.
fn add_coupling(
    lp: &mut LpModel,
    graph: &FloorGraph,
    domains: &[Vec<Coor>],
    x: &[Vec<LpVar>],
    y: &[Vec<Vec<LpVar>>],
) {
    for (ei, edge) in graph.placement_edges().iter().enumerate() {
        // Re-bucket the shared enumeration into this edge's 2-D plane: one
        // source row per source candidate, one destination column per
        // destination candidate.
        let mut src_rows: Vec<SparseRow> = std::iter::repeat_with(SparseRow::new)
            .take(domains[edge.src].len())
            .collect();
        let mut dst_cols: Vec<SparseRow> = std::iter::repeat_with(SparseRow::new)
            .take(domains[edge.dst].len())
            .collect();
        for term in edge_y_terms(ei, edge, domains) {
            let var = term.var(y);
            src_rows[term.src_ci].push(1.0, var);
            dst_cols[term.dst_ci].push(1.0, var);
        }

        for (src_ci, mut terms) in src_rows.into_iter().enumerate() {
            terms.push(-1.0, x[edge.src][src_ci]);
            lp.add_constraint(
                format!("edge_{ei}_src_{src_ci}"),
                terms.into_expr(),
                Comparison::Eq,
                0.0,
            );
        }

        for (dst_ci, mut terms) in dst_cols.into_iter().enumerate() {
            terms.push(-1.0, x[edge.dst][dst_ci]);
            lp.add_constraint(
                format!("edge_{ei}_dst_{dst_ci}"),
                terms.into_expr(),
                Comparison::Eq,
                0.0,
            );
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "resource rows need the vertex areas, the crossing registers, and the limits that bound both"
)]
fn add_resource_constraints(
    lp: &mut LpModel,
    graph: &FloorGraph,
    areas: &RegionAreas,
    domains: &[Vec<Coor>],
    usage_limit: f64,
    constraints: &PlacementConstraints,
    x: &[Vec<LpVar>],
    y: &[Vec<Vec<LpVar>>],
) -> Result<(), IlpError> {
    let active_regions: BTreeSet<Coor> = domains.iter().flatten().copied().collect();
    for region in active_regions {
        let total = *areas
            .get(&region)
            .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
        for resource in Resource::ALL {
            let mut terms = SparseRow::new();
            for (vi, vertex) in graph.vertices().iter().enumerate() {
                if let Some(ci) = domains[vi]
                    .iter()
                    .position(|candidate| *candidate == region)
                {
                    terms.push(resource.amount(&vertex.area).as_f64(), x[vi][ci]);
                }
            }
            if resource == Resource::Ff {
                add_crossing_head_terms(&mut terms, graph, domains, y, &region);
            }

            // The same floor `scaled_area_with_overrides` applies when it
            // filters candidates, so a region the filter admits is never one
            // this row then rejects by a fraction of a unit.
            let limit = lookup_limit(&constraints.max_resource_limits, &region, resource)
                .unwrap_or(usage_limit);
            let max_rhs = scaled_amount(resource.amount(&total), limit).as_f64();
            lp.add_constraint(
                format!("node_{}_{}_usage", region.region_name(), resource.name()),
                terms.into_expr(),
                Comparison::Le,
                max_rhs,
            );
        }
    }
    Ok(())
}

/// Reserve the flip-flops a crossing's generated Head registers will take in
/// `region`.
///
/// Every channel that ends up crossing gets a Head register bundle in its
/// *source* slot, one flip-flop per physical wire. The route is not chosen yet,
/// so the Body registers in intermediate slots cannot be charged here — but the
/// Heads can: `y[e][a][b]` already says which region each endpoint landed in,
/// so a crossing term contributes its forward width to `a` and its reverse
/// width to `b`. Charging them makes the placement reserve room it will
/// certainly need, instead of discovering the overflow after routing.
///
/// The Head's single gate LUT per channel is left out: the edge aggregates
/// channels, so the count is not on hand, and one LUT against a slot's hundreds
/// of thousands is not worth carrying a second counter for.
fn add_crossing_head_terms(
    terms: &mut SparseRow,
    graph: &FloorGraph,
    domains: &[Vec<Coor>],
    y: &[Vec<Vec<LpVar>>],
    region: &Coor,
) {
    for term in y_terms(graph, domains) {
        if term.src == term.dst {
            continue; // co-located: the channel stays a direct wire
        }
        let heads = u64::from(if term.src == region {
            term.edge.forward_width
        } else if term.dst == region {
            term.edge.reverse_width
        } else {
            0
        });
        if heads > 0 {
            terms.push(heads.as_f64(), term.var(y));
        }
    }
}

fn lookup_limit(limits: &RegionResourceLimits, region: &Coor, resource: Resource) -> Option<f64> {
    limits
        .get(&region.region_name())
        .and_then(|by_resource| by_resource.get(&resource))
        .copied()
}

/// Per-cut wire constraint over only the sparse route variables that exist.
fn add_cut_constraints(
    lp: &mut LpModel,
    graph: &FloorGraph,
    domains: &[Vec<Coor>],
    cuts: &[Cut],
    y: &[Vec<Vec<LpVar>>],
) {
    for cut in cuts {
        let lhs: BTreeSet<Coor> = cut.lhs.iter().copied().collect();
        let rhs: BTreeSet<Coor> = cut.rhs.iter().copied().collect();
        let mut terms = SparseRow::new();
        for term in y_terms(graph, domains) {
            let crosses = (lhs.contains(term.src) && rhs.contains(term.dst))
                || (rhs.contains(term.src) && lhs.contains(term.dst));
            if crosses {
                terms.push(f64::from(term.edge.width), term.var(y));
            }
        }
        lp.add_constraint(
            format!("cut_{}_capacity", cut.name),
            terms.into_expr(),
            Comparison::Le,
            cut.capacity.as_f64(),
        );
    }
}

/// Width-weighted adjusted centroid distance plus the formulation's constant
/// one.
fn add_objective(
    lp: &mut LpModel,
    graph: &FloorGraph,
    device: &Device,
    domains: &[Vec<Coor>],
    y: &[Vec<Vec<LpVar>>],
) -> Result<(), IlpError> {
    // Region centroids repeat across every incident edge; resolve once.
    let centroids: std::collections::BTreeMap<Coor, (i64, i64)> = domains
        .iter()
        .flatten()
        .map(|region| centroid_twice(device, region).map(|centroid| (*region, centroid)))
        .collect::<Result<_, _>>()?;
    let mut objective = SparseRow::new();
    for term in y_terms(graph, domains) {
        let src_centroid = centroids[term.src];
        let dst_centroid = centroids[term.dst];
        let distance_twice = (src_centroid.0 - dst_centroid.0).abs()
            + VERTICAL_DIST_PENALTY * (src_centroid.1 - dst_centroid.1).abs();
        if distance_twice != 0 {
            let coefficient = f64::from(term.edge.width) * distance_twice.as_f64() / 2.0;
            objective.push(coefficient, term.var(y));
        }
    }
    lp.set_objective(objective.into_expr().plus_constant(1.0));
    Ok(())
}

/// Twice the exact midpoint, so half-integer rectangular centroids remain
/// exact until the final LP coefficient conversion.
fn centroid_twice(device: &Device, region: &Coor) -> Result<(i64, i64), IlpError> {
    let dl = device
        .slot(region.dl_x, region.dl_y)
        .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
    let ur = device
        .slot(region.ur_x, region.ur_y)
        .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
    Ok((dl.centroid_x + ur.centroid_x, dl.centroid_y + ur.centroid_y))
}

#[cfg(test)]
mod tests;
