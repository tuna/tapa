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
//! Multilevel placement merely changes the candidate regions: the first pass
//! assigns vertices to full-width rows, and the second jointly refines every
//! row into atomic column slots while retaining parent containment. If that
//! aggregate row assignment cannot be decomposed, the atomic iteration is
//! retried without the provisional parent restriction.

use std::collections::{BTreeMap, BTreeSet};

use tapa_ir::Area;

use crate::device::model::{
    add_area, Coor, Device, Resource, DEFAULT_USAGE_LIMIT, VERTICAL_DIST_PENALTY,
};
use crate::graph::FloorGraph;
use crate::partition::cut::{find_cuts_for_regions, Cut};
use crate::solver::{
    Comparison, LinExpr, LpModel, LpStatus, LpVar, Sense, SolveOpts, Solver, SolverError,
};

/// The retry envelope for partitioning.
const USAGE_LIMIT_STEP: f64 = 0.02;
pub(crate) const MAX_USAGE_LIMIT: f64 = 0.95;
const INTEGRAL_TOLERANCE: f64 = 1e-5;

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
    min_resource_limits: RegionResourceLimits,
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
    #[error("floorplan is infeasible up to usage limit {0}")]
    Infeasible(f64),
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
    /// A partition region refers to cells absent from the device model.
    #[error("partition region `{0}` is outside the device model")]
    InvalidRegion(String),
    /// A positive minimum cannot be met by any candidate vertex.
    #[error("minimum {resource} utilization for `{region}` cannot be satisfied")]
    ImpossibleMinimum {
        region: String,
        resource: &'static str,
    },
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
    let by_resource = BTreeMap::from([
        (Resource::Bram18k, block_limit),
        (Resource::Dsp, block_limit),
        (Resource::Uram, block_limit),
    ]);
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
            solve_iteration(graph, device, &slots, None, config, solver, opts)?
        }
        PartitionStrategy::MultiLevel => {
            let rows = row_regions(device);
            let row_assignment = solve_iteration(graph, device, &rows, None, config, solver, opts)?;
            let slots = atomic_regions(device);
            match solve_iteration(
                graph,
                device,
                &slots,
                Some(&row_assignment),
                config,
                solver,
                opts,
            ) {
                Ok(assignment) => assignment,
                Err(IlpError::Infeasible(_) | IlpError::NoCandidates { .. }) => {
                    solve_iteration(graph, device, &slots, None, config, solver, opts)?
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
    if device.cols == 1 || edge_count < 300 {
        return PartitionStrategy::Flat;
    }
    if edge_count > 800 {
        return PartitionStrategy::MultiLevel;
    }
    if u64::from(device.rows) * u64::from(device.rows) >= 8 {
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
    for (kind, limits) in [
        ("minimum", &config.constraints.min_resource_limits),
        ("maximum", &config.constraints.max_resource_limits),
    ] {
        for (region, by_resource) in limits {
            for &value in by_resource.values() {
                validate_limit(kind, region, value, true)?;
            }
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
    if device.cols == 0 {
        return Vec::new();
    }
    (0..device.rows)
        .map(|y| Coor::span(0, y, device.cols - 1, y))
        .collect()
}

/// Run one partition iteration, retrying only this iteration at successively
/// higher global usage limits.
fn solve_iteration(
    graph: &FloorGraph,
    device: &Device,
    regions: &[Coor],
    parents: Option<&BTreeMap<String, Coor>>,
    config: &PlacementConfig,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<BTreeMap<String, Coor>, IlpError> {
    let retry_ceiling = config.retry_ceiling;
    let mut usage_limit = config.usage_limit;

    loop {
        let domains = match candidate_domains(
            graph,
            device,
            regions,
            usage_limit,
            parents,
            &config.constraints,
        ) {
            Ok(domains) => domains,
            Err(error @ IlpError::NoCandidates { .. }) => {
                // A domain can be empty merely because the current global
                // utilization derating is too strict. Probe the retry ceiling
                // once to distinguish that case from a permanent pin, parent,
                // or memory-domain conflict before deciding whether to retry.
                match candidate_domains(
                    graph,
                    device,
                    regions,
                    retry_ceiling,
                    parents,
                    &config.constraints,
                ) {
                    Ok(_) => {
                        let Some(next) = next_usage_limit(usage_limit, retry_ceiling) else {
                            return Err(error);
                        };
                        usage_limit = next;
                    }
                    Err(permanent) => return Err(permanent),
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        let cuts = find_cuts_for_regions(device, regions);
        let model = FloorplanModel::build(
            graph,
            device,
            &domains,
            &cuts,
            usage_limit,
            &config.constraints,
        )?;
        let solution = solver.solve(&model.lp, opts)?;
        if solution.is_found() {
            log::info!("placement succeeded at usage limit {usage_limit:.2}");
            return model.read_back(graph, &domains, &solution);
        }

        match solution.status {
            LpStatus::Infeasible => {
                log::info!("placement infeasible at usage limit {usage_limit:.2}; retrying higher");
            }
            LpStatus::NotSolved | LpStatus::Unbounded => {
                return Err(IlpError::NoIncumbent(solution.status));
            }
            LpStatus::Optimal | LpStatus::Feasible => {
                unreachable!("found incumbents returned during readback")
            }
        }

        let Some(next) = next_usage_limit(usage_limit, retry_ceiling) else {
            return Err(IlpError::Infeasible(retry_ceiling));
        };
        usage_limit = next;
    }
}

fn next_usage_limit(current: f64, ceiling: f64) -> Option<f64> {
    (current < ceiling).then(|| (current + USAGE_LIMIT_STEP).min(ceiling))
}

/// Legal region lists for every vertex.  All placement restrictions are
/// represented here, before variables are allocated.
fn candidate_domains(
    graph: &FloorGraph,
    device: &Device,
    regions: &[Coor],
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

            let total = device
                .island_area(&region)
                .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
            let available = scaled_area_with_overrides(
                total,
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
    let amount = |resource: Resource| {
        scaled_amount(
            resource.amount(&area),
            lookup_limit(limits, region, resource).unwrap_or(default_limit),
        )
    };
    Area {
        lut: amount(Resource::Lut),
        ff: amount(Resource::Ff),
        bram_18k: amount(Resource::Bram18k),
        dsp: amount(Resource::Dsp),
        uram: amount(Resource::Uram),
    }
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
            *entry = add_area(*entry, vertex.area);
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
    fn build(
        graph: &FloorGraph,
        device: &Device,
        domains: &[Vec<Coor>],
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
            device,
            domains,
            usage_limit,
            constraints,
            &x,
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
            let mut selected = Vec::new();
            let mut values = Vec::with_capacity(self.x[vi].len());
            for (ci, &var) in self.x[vi].iter().enumerate() {
                let value = solution.values.get(&var).copied().ok_or_else(|| {
                    IlpError::InvalidSolution(format!(
                        "solver result omitted assignment variable x{} for vertex `{}`",
                        var.0, vertex.name
                    ))
                })?;
                values.push(value);
                if !value.is_finite()
                    || (value - value.round()).abs() > INTEGRAL_TOLERANCE
                    || !(-INTEGRAL_TOLERANCE..=1.0 + INTEGRAL_TOLERANCE).contains(&value)
                {
                    return Err(IlpError::InvalidSolution(format!(
                        "vertex `{}` has non-binary values {values:?}",
                        vertex.name
                    )));
                }
                if (value - 1.0).abs() <= INTEGRAL_TOLERANCE {
                    selected.push(ci);
                }
            }
            if selected.len() != 1 {
                return Err(IlpError::InvalidSolution(format!(
                    "vertex `{}` must select exactly one candidate, got {values:?}",
                    vertex.name
                )));
            }
            if assignments
                .insert(vertex.name.clone(), domains[vi][selected[0]])
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
            let row: Vec<LpVar> = domains[vi]
                .iter()
                .map(|region| lp.add_binary(format!("x_{}_{}", vertex.name, region.region_name())))
                .collect();
            lp.add_constraint(
                format!("vertex_{}", vertex.name),
                LinExpr::sum(row.iter().map(|&var| (1.0, var))),
                Comparison::Eq,
                1.0,
            );
            row
        })
        .collect()
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
            let plane: Vec<Vec<LpVar>> = domains[edge.src]
                .iter()
                .enumerate()
                .map(|(src_ci, _)| {
                    domains[edge.dst]
                        .iter()
                        .enumerate()
                        .map(|(dst_ci, _)| lp.add_binary(format!("y_{ei}_{src_ci}_{dst_ci}")))
                        .collect()
                })
                .collect();
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
        for src_ci in 0..domains[edge.src].len() {
            let mut terms: Vec<(f64, LpVar)> =
                y[ei][src_ci].iter().map(|&var| (1.0, var)).collect();
            terms.push((-1.0, x[edge.src][src_ci]));
            lp.add_constraint(
                format!("edge_{ei}_src_{src_ci}"),
                LinExpr::sum(terms),
                Comparison::Eq,
                0.0,
            );
        }

        for dst_ci in 0..domains[edge.dst].len() {
            let mut terms: Vec<(f64, LpVar)> = y[ei].iter().map(|row| (1.0, row[dst_ci])).collect();
            terms.push((-1.0, x[edge.dst][dst_ci]));
            lp.add_constraint(
                format!("edge_{ei}_dst_{dst_ci}"),
                LinExpr::sum(terms),
                Comparison::Eq,
                0.0,
            );
        }
    }
}

fn add_resource_constraints(
    lp: &mut LpModel,
    graph: &FloorGraph,
    device: &Device,
    domains: &[Vec<Coor>],
    usage_limit: f64,
    constraints: &PlacementConstraints,
    x: &[Vec<LpVar>],
) -> Result<(), IlpError> {
    let active_regions: BTreeSet<Coor> = domains.iter().flatten().copied().collect();
    for region in active_regions {
        let total = device
            .island_area(&region)
            .ok_or_else(|| IlpError::InvalidRegion(region.region_name()))?;
        for resource in Resource::ALL {
            let terms: Vec<(f64, LpVar)> = graph
                .vertices()
                .iter()
                .enumerate()
                .filter_map(|(vi, vertex)| {
                    let ci = domains[vi]
                        .iter()
                        .position(|candidate| *candidate == region)?;
                    Some((u64_as_f64(resource.amount(&vertex.area)), x[vi][ci]))
                })
                .collect();

            let max_rhs = lookup_limit(&constraints.max_resource_limits, &region, resource)
                .map_or_else(
                    || u64_as_f64(scaled_amount(resource.amount(&total), usage_limit)),
                    |limit| u64_as_f64(resource.amount(&total)) * limit,
                );
            lp.add_constraint(
                format!("node_{}_{}_usage", region.region_name(), resource.name()),
                LinExpr::sum(terms.iter().copied()),
                Comparison::Le,
                max_rhs,
            );

            if let Some(limit) = lookup_limit(&constraints.min_resource_limits, &region, resource) {
                let min_rhs = u64_as_f64(resource.amount(&total)) * limit;
                if min_rhs > 0.0 && terms.iter().all(|(coef, _)| *coef == 0.0) {
                    return Err(IlpError::ImpossibleMinimum {
                        region: region.region_name(),
                        resource: resource.name(),
                    });
                }
                lp.add_constraint(
                    format!("node_{}_{}_usage_ge", region.region_name(), resource.name()),
                    LinExpr::sum(terms),
                    Comparison::Ge,
                    min_rhs,
                );
            }
        }
    }
    Ok(())
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
        let mut terms = Vec::new();
        for (ei, edge) in graph.placement_edges().iter().enumerate() {
            for (src_ci, src_region) in domains[edge.src].iter().enumerate() {
                for (dst_ci, dst_region) in domains[edge.dst].iter().enumerate() {
                    let crosses = (lhs.contains(src_region) && rhs.contains(dst_region))
                        || (rhs.contains(src_region) && lhs.contains(dst_region));
                    if crosses {
                        terms.push((f64::from(edge.width), y[ei][src_ci][dst_ci]));
                    }
                }
            }
        }
        lp.add_constraint(
            format!("cut_{}_capacity", cut.name),
            LinExpr::sum(terms),
            Comparison::Le,
            u64_as_f64(cut.capacity),
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
    let mut objective = Vec::new();
    for (ei, edge) in graph.placement_edges().iter().enumerate() {
        for (src_ci, src) in domains[edge.src].iter().enumerate() {
            let src_centroid = centroid_twice(device, src)?;
            for (dst_ci, dst) in domains[edge.dst].iter().enumerate() {
                let dst_centroid = centroid_twice(device, dst)?;
                let distance_twice = (src_centroid.0 - dst_centroid.0).abs()
                    + VERTICAL_DIST_PENALTY * (src_centroid.1 - dst_centroid.1).abs();
                if distance_twice != 0 {
                    let coefficient = f64::from(edge.width) * i64_as_f64(distance_twice) / 2.0;
                    objective.push((coefficient, y[ei][src_ci][dst_ci]));
                }
            }
        }
    }
    lp.set_objective(LinExpr::sum(objective).plus_constant(1.0));
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

#[allow(
    clippy::cast_precision_loss,
    reason = "FPGA resource and cut coefficients are small exact integers"
)]
fn u64_as_f64(value: u64) -> f64 {
    value as f64
}

#[allow(
    clippy::cast_precision_loss,
    reason = "device-grid centroid distances are small exact integers"
)]
fn i64_as_f64(value: i64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::device::model::{DirCaps, DirRegions, Slot};
    use crate::device::select::select_device;
    use crate::solver::{LpSolution, VarKind};

    fn named_terms(model: &LpModel, expr: &LinExpr) -> BTreeMap<String, f64> {
        let mut terms = BTreeMap::new();
        for &(coefficient, var) in &expr.terms {
            let index = usize::try_from(var.0).expect("variable index fits usize");
            let label = model.vars[index].label.clone();
            *terms.entry(label).or_insert(0.0) += coefficient;
        }
        terms
    }

    fn assert_row<'a>(
        model: &LpModel,
        name: &str,
        op: Comparison,
        rhs: f64,
        expected_terms: impl IntoIterator<Item = (f64, &'a str)>,
    ) {
        let row = model
            .constraints
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("missing model row `{name}`"));
        assert_eq!(row.op, op, "comparison drifted for `{name}`");
        assert_eq!(
            row.rhs.to_bits(),
            rhs.to_bits(),
            "right-hand side drifted for `{name}`"
        );
        assert_eq!(
            named_terms(model, &row.expr),
            expected_terms
                .into_iter()
                .map(|(coefficient, label)| (label.to_string(), coefficient))
                .collect(),
            "coefficients drifted for `{name}`"
        );
    }

    /// A deterministic test solver that selects a requested region suffix for
    /// each x row (or the first candidate) and sets all remaining variables to
    /// zero.  Placement readback deliberately does not rely on y values.
    struct ChooseSolver {
        preferred: Mutex<Vec<String>>,
    }

    impl ChooseSolver {
        fn first() -> Self {
            Self {
                preferred: Mutex::new(Vec::new()),
            }
        }

        fn with_preferences(preferred: Vec<String>) -> Self {
            Self {
                preferred: Mutex::new(preferred),
            }
        }
    }

    impl Solver for ChooseSolver {
        fn solve(&self, model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            let preferred = self.preferred.lock().expect("lock");
            let mut first_by_vertex: BTreeMap<String, LpVar> = BTreeMap::new();
            let mut chosen_by_vertex: BTreeMap<String, LpVar> = BTreeMap::new();
            for (index, var) in model.vars.iter().enumerate() {
                let Some(rest) = var.label.strip_prefix("x_") else {
                    continue;
                };
                let Some(marker) = rest.find("_SLOT_X") else {
                    continue;
                };
                let vertex = rest[..marker].to_string();
                let handle = LpVar(u32::try_from(index).expect("index"));
                first_by_vertex.entry(vertex.clone()).or_insert(handle);
                if preferred.iter().any(|suffix| var.label.ends_with(suffix)) {
                    chosen_by_vertex.insert(vertex, handle);
                }
            }
            let mut values: HashMap<LpVar, f64> = (0..model.num_vars())
                .map(|index| (LpVar(u32::try_from(index).expect("index")), 0.0))
                .collect();
            for (vertex, fallback) in first_by_vertex {
                let selected = chosen_by_vertex.get(&vertex).copied().unwrap_or(fallback);
                values.insert(selected, 1.0);
            }
            Ok(LpSolution {
                status: LpStatus::Optimal,
                objective: 0.0,
                values,
            })
        }
    }

    struct RecordingSolver {
        inner: ChooseSolver,
        models: Mutex<Vec<LpModel>>,
    }

    impl RecordingSolver {
        fn first() -> Self {
            Self {
                inner: ChooseSolver::first(),
                models: Mutex::new(Vec::new()),
            }
        }
    }

    struct RejectFirstRefinementSolver {
        calls: AtomicUsize,
        inner: ChooseSolver,
        models: Mutex<Vec<LpModel>>,
    }

    impl RejectFirstRefinementSolver {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                inner: ChooseSolver::first(),
                models: Mutex::new(Vec::new()),
            }
        }
    }

    impl Solver for RejectFirstRefinementSolver {
        fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            self.models.lock().expect("lock").push(model.clone());
            if call == 1 {
                return Ok(LpSolution {
                    status: LpStatus::Infeasible,
                    objective: 0.0,
                    values: HashMap::new(),
                });
            }
            self.inner.solve(model, opts)
        }
    }

    impl Solver for RecordingSolver {
        fn solve(&self, model: &LpModel, opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            self.models.lock().expect("lock").push(model.clone());
            self.inner.solve(model, opts)
        }
    }

    struct StatusSolver {
        status: LpStatus,
        calls: AtomicUsize,
    }

    impl StatusSolver {
        fn new(status: LpStatus) -> Self {
            Self {
                status,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Solver for StatusSolver {
        fn solve(&self, _model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(LpSolution {
                status: self.status,
                objective: 0.0,
                values: HashMap::new(),
            })
        }
    }

    fn vadd_floor_graph() -> FloorGraph {
        let json = r#"{
            "cflags": [], "top": "VecAdd", "target": "xilinx-hls",
            "tasks": {
                "VecAdd": {
                    "readable_name": "VecAdd", "code": "void VecAdd() {}", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "A": [{"args": {"out": {"arg": "fifo", "cat": "ostream"}}, "step": 0}],
                        "B": [{"args": {"in": {"arg": "fifo", "cat": "istream"}}, "step": 0}]
                    },
                    "fifos": {"fifo": {"depth": 2, "consumed_by": ["B", 0], "produced_by": ["A", 0]}}
                },
                "A": {"readable_name": "A", "code": "void A() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                    "self_area": {"LUT": 100, "FF": 200}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 50, "FF": 60}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let flat = tapa_ir::flatten(&graph).expect("flatten");
        FloorGraph::build(&flat).expect("floor graph")
    }

    fn parallel_floor_graph(stream_count: usize) -> FloorGraph {
        let mut producer_args = serde_json::Map::new();
        let mut consumer_args = serde_json::Map::new();
        let mut producer_ports = Vec::new();
        let mut consumer_ports = Vec::new();
        let mut fifos = serde_json::Map::new();
        for index in 0..stream_count {
            let port = format!("p{index}");
            let fifo = format!("q{index}");
            producer_args.insert(
                port.clone(),
                serde_json::json!({"arg": fifo.clone(), "cat": "ostream"}),
            );
            consumer_args.insert(
                port.clone(),
                serde_json::json!({"arg": fifo.clone(), "cat": "istream"}),
            );
            producer_ports.push(
                serde_json::json!({"cat": "ostream", "name": port, "type": "int", "width": 32}),
            );
            consumer_ports.push(
                serde_json::json!({"cat": "istream", "name": port, "type": "int", "width": 32}),
            );
            fifos.insert(
                fifo,
                serde_json::json!({
                    "depth": 2,
                    "produced_by": ["Producer", 0],
                    "consumed_by": ["Consumer", 0]
                }),
            );
        }
        let design = serde_json::json!({
            "cflags": [],
            "top": "Top",
            "target": "xilinx-hls",
            "tasks": {
                "Top": {
                    "readable_name": "Top", "code": "void Top() {}",
                    "level": "upper", "synth": "hls", "ports": [],
                    "tasks": {
                        "Producer": [{"args": producer_args, "step": 0}],
                        "Consumer": [{"args": consumer_args, "step": 0}]
                    },
                    "fifos": fifos
                },
                "Producer": {
                    "readable_name": "Producer", "code": "void Producer() {}",
                    "level": "lower", "synth": "hls", "ports": producer_ports
                },
                "Consumer": {
                    "readable_name": "Consumer", "code": "void Consumer() {}",
                    "level": "lower", "synth": "hls", "ports": consumer_ports
                }
            }
        });
        let graph = tapa_ir::TaskGraph::from_json(&design.to_string()).expect("parse");
        let flat = tapa_ir::flatten(&graph).expect("flatten");
        FloorGraph::build(&flat).expect("floor graph")
    }

    fn single_task_floor_graph(lut: u64) -> FloorGraph {
        single_task_floor_graph_with_area(Area {
            lut,
            ..Area::default()
        })
    }

    fn single_task_floor_graph_with_area(area: Area) -> FloorGraph {
        let json = serde_json::json!({
            "cflags": [],
            "top": "Top",
            "target": "xilinx-hls",
            "tasks": {
                "Top": {
                    "readable_name": "Top",
                    "code": "void Top() {}",
                    "level": "upper",
                    "synth": "hls",
                    "ports": [],
                    "tasks": {"A": [{"args": {}, "step": 0}]},
                    "fifos": {}
                },
                "A": {
                    "readable_name": "A",
                    "code": "void A() {}",
                    "level": "lower",
                    "synth": "hls",
                    "ports": [],
                    "self_area": {
                        "LUT": area.lut,
                        "FF": area.ff,
                        "BRAM_18K": area.bram_18k,
                        "DSP": area.dsp,
                        "URAM": area.uram
                    }
                }
            }
        });
        let graph = tapa_ir::TaskGraph::from_json(&json.to_string()).expect("parse");
        let flat = tapa_ir::flatten(&graph).expect("flatten");
        FloorGraph::build(&flat).expect("floor graph")
    }

    fn one_slot_device(lut: u64) -> Device {
        Device {
            key: "one-slot".to_string(),
            part_num: "xcone".to_string(),
            platform_name: None,
            rows: 1,
            cols: 1,
            pp_dist: 1,
            is_versal: false,
            user_pblock_name: None,
            slots: vec![Slot {
                x: 0,
                y: 0,
                area: Area {
                    lut,
                    ..Area::default()
                },
                centroid_x: 0,
                centroid_y: 0,
                pblock_ranges: Vec::new(),
                wire_cap: DirCaps::default(),
                anchor: DirRegions::default(),
                tags: Vec::new(),
            }],
        }
    }

    fn two_slot_golden_device() -> Device {
        let slot = |y, centroid_y| Slot {
            x: 0,
            y,
            area: Area {
                lut: 1_000,
                ff: 1_000,
                bram_18k: 100,
                dsp: 100,
                uram: 100,
            },
            centroid_x: 0,
            centroid_y,
            pblock_ranges: Vec::new(),
            wire_cap: DirCaps::default(),
            anchor: DirRegions::default(),
            tags: Vec::new(),
        };
        Device {
            key: "two-slot-golden".to_string(),
            part_num: "xctoy".to_string(),
            platform_name: None,
            rows: 2,
            cols: 1,
            pp_dist: 1,
            is_versal: false,
            user_pblock_name: None,
            slots: vec![slot(0, 0), slot(1, 150)],
        }
    }

    fn two_slot_golden_model(graph: &FloorGraph) -> FloorplanModel {
        let bottom = Coor::slot(0, 0);
        let top = Coor::slot(0, 1);
        FloorplanModel::build(
            graph,
            &two_slot_golden_device(),
            &vec![vec![bottom, top]; graph.vertices().len()],
            &[Cut {
                name: "y=0".to_string(),
                lhs: vec![bottom],
                rhs: vec![top],
                capacity: 34,
            }],
            DEFAULT_USAGE_LIMIT,
            &PlacementConstraints::default(),
        )
        .expect("golden model")
    }

    fn mmap_floor_graph() -> FloorGraph {
        let json = r#"{
            "cflags": [], "top": "Top", "target": "xilinx-hls",
            "tasks": {
                "Top": {"readable_name": "Top", "code": "void Top() {}", "level": "upper", "synth": "hls",
                    "ports": [{"cat": "mmap", "name": "mem", "type": "ap_uint<512>*", "width": 512}], "tasks": {
                        "R": [{"args": {"m": {"arg": "mem", "cat": "mmap"}}, "step": 0}],
                        "C": [{"args": {}, "step": 0}]}, "fifos": {}},
                "R": {"readable_name": "R", "code": "void R() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "mmap", "name": "m", "type": "ap_uint<512>*", "width": 512}],
                    "self_area": {"LUT": 400}},
                "C": {"readable_name": "C", "code": "void C() {}", "level": "lower", "synth": "hls",
                    "ports": [], "self_area": {"LUT": 400}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let flat = tapa_ir::flatten(&graph).expect("flatten");
        FloorGraph::build_with_memory(
            &flat,
            &[crate::graph::MemoryInterface {
                endpoint: tapa_ir::AxiEndpoint {
                    instance: "R_0".to_string(),
                    port: "m".to_string(),
                    top_port: "mem".to_string(),
                },
                bank: tapa_ir::MemoryBank {
                    kind: tapa_ir::MemoryKind::Hbm,
                    index: 7,
                },
                channel_widths: tapa_ir::AxiChannelWidths {
                    read_address: 80,
                    read_data: 518,
                    write_address: 80,
                    write_data: 579,
                    write_response: 5,
                },
                bridge_instance: None,
            }],
        )
        .expect("floor graph")
    }

    #[test]
    fn canonical_placement_model_matches_expected_formulation() {
        let graph = vadd_floor_graph();
        let bottom = Coor::slot(0, 0);
        let top = Coor::slot(0, 1);
        let model = two_slot_golden_model(&graph);
        let lp = &model.lp;

        let bottom_name = bottom.region_name();
        let top_name = top.region_name();
        let producer_x = [format!("x_A_0_{bottom_name}"), format!("x_A_0_{top_name}")];
        let consumer_x = [format!("x_B_0_{bottom_name}"), format!("x_B_0_{top_name}")];
        let route_y = ["y_0_0_0", "y_0_0_1", "y_0_1_0", "y_0_1_1"];

        assert_eq!(lp.sense, Sense::Minimize);
        assert_eq!(lp.num_vars(), 8, "four sparse x plus four sparse y");
        assert!(lp.vars.iter().all(|var| var.kind == VarKind::Binary
            && var.lower.to_bits() == 0.0_f64.to_bits()
            && var.upper.to_bits() == 1.0_f64.to_bits()));
        assert_eq!(
            lp.vars
                .iter()
                .map(|var| var.label.as_str())
                .collect::<BTreeSet<_>>(),
            producer_x
                .iter()
                .chain(&consumer_x)
                .map(String::as_str)
                .chain(route_y.iter().copied())
                .collect()
        );

        assert_eq!(lp.num_constraints(), 18);
        for (name, vars) in [("vertex_A_0", &producer_x), ("vertex_B_0", &consumer_x)] {
            assert_row(
                lp,
                name,
                Comparison::Eq,
                1.0,
                vars.iter().map(|label| (1.0, label.as_str())),
            );
        }
        assert_row(
            lp,
            "route_0",
            Comparison::Eq,
            1.0,
            route_y.iter().map(|label| (1.0, *label)),
        );
        for (name, terms) in [
            (
                "edge_0_src_0",
                [(1.0, route_y[0]), (1.0, route_y[1]), (-1.0, &producer_x[0])],
            ),
            (
                "edge_0_src_1",
                [(1.0, route_y[2]), (1.0, route_y[3]), (-1.0, &producer_x[1])],
            ),
            (
                "edge_0_dst_0",
                [(1.0, route_y[0]), (1.0, route_y[2]), (-1.0, &consumer_x[0])],
            ),
            (
                "edge_0_dst_1",
                [(1.0, route_y[1]), (1.0, route_y[3]), (-1.0, &consumer_x[1])],
            ),
        ] {
            assert_row(lp, name, Comparison::Eq, 0.0, terms);
        }

        assert_eq!(
            lp.constraints
                .iter()
                .filter(|row| row.name.starts_with("node_"))
                .count(),
            10,
            "five resource rows per active slot"
        );
        for (region, producer, consumer) in [
            (&bottom_name, &producer_x[0], &consumer_x[0]),
            (&top_name, &producer_x[1], &consumer_x[1]),
        ] {
            assert_row(
                lp,
                &format!("node_{region}_LUT_usage"),
                Comparison::Le,
                700.0,
                [(100.0, producer.as_str()), (116.0, consumer.as_str())],
            );
        }
        assert_row(
            lp,
            "cut_y=0_capacity",
            Comparison::Le,
            34.0,
            [(35.0, route_y[1]), (35.0, route_y[2])],
        );

        assert_eq!(lp.objective.constant.to_bits(), 1.0_f64.to_bits());
        assert_eq!(
            named_terms(lp, &lp.objective),
            BTreeMap::from([
                (route_y[1].to_string(), 10_500.0),
                (route_y[2].to_string(), 10_500.0),
            ]),
            "35-bit width times the vertically penalized 150-unit distance"
        );
    }

    #[test]
    fn parallel_streams_share_one_placement_edge_plane() {
        let graph = parallel_floor_graph(2);
        let model = two_slot_golden_model(&graph);
        let route_y = ["y_0_0_0", "y_0_0_1", "y_0_1_0", "y_0_1_1"];

        assert_eq!(graph.streams().len(), 2);
        assert_eq!(graph.placement_edges().len(), 1);
        assert_eq!(graph.placement_edges()[0].width, 70);
        assert_eq!(
            model
                .lp
                .vars
                .iter()
                .filter(|var| var.label.starts_with("y_"))
                .count(),
            4,
            "one endpoint pair allocates one sparse y plane"
        );
        assert_row(
            &model.lp,
            "cut_y=0_capacity",
            Comparison::Le,
            34.0,
            [(70.0, route_y[1]), (70.0, route_y[2])],
        );
        assert_eq!(
            named_terms(&model.lp, &model.lp.objective),
            BTreeMap::from([
                (route_y[1].to_string(), 21_000.0),
                (route_y[2].to_string(), 21_000.0),
            ])
        );
    }

    #[test]
    fn sparse_domains_encode_exact_terminals_and_user_pins() {
        let graph = mmap_floor_graph();
        let device = select_device("u280").expect("u280");
        let regions = atomic_regions(&device);
        let mut constraints = PlacementConstraints::default();
        constraints
            .vertex_regions
            .insert("C_0".to_string(), Coor::slot(1, 2));
        let domains = candidate_domains(
            &graph,
            &device,
            &regions,
            DEFAULT_USAGE_LIMIT,
            None,
            &constraints,
        )
        .expect("domains");

        let reader = graph.index_of("R_0").expect("reader");
        assert_eq!(
            domains[reader], regions,
            "the compute task remains movable; its bank is an ordinary weighted endpoint"
        );
        let terminal = graph.index_of("__tapa_bank_hbm_7").expect("bank terminal");
        assert_eq!(
            domains[terminal],
            [Coor::slot(0, 0)],
            "the exact HBM bank terminal is fixed by its device tag"
        );
        let compute = graph.index_of("C_0").expect("compute");
        assert_eq!(domains[compute], [Coor::slot(1, 2)]);

        let model = FloorplanModel::build(
            &graph,
            &device,
            &domains,
            &find_cuts_for_regions(&device, &regions),
            DEFAULT_USAGE_LIMIT,
            &constraints,
        )
        .expect("model");
        let x_count = model
            .lp
            .vars
            .iter()
            .filter(|var| var.label.starts_with("x_"))
            .count();
        assert_eq!(
            x_count, 8,
            "six reader candidates, one compute pin, and one exact bank terminal"
        );

        let stream_graph = vadd_floor_graph();
        let a = stream_graph.index_of("A_0").expect("A");
        let b = stream_graph.index_of("B_0").expect("B");
        let mut sparse_domains = vec![Vec::new(); stream_graph.vertices().len()];
        sparse_domains[a] = vec![Coor::slot(0, 0), Coor::slot(1, 0)];
        sparse_domains[b] = vec![Coor::slot(0, 0), Coor::slot(1, 0)];
        let sparse_model = FloorplanModel::build(
            &stream_graph,
            &device,
            &sparse_domains,
            &[],
            DEFAULT_USAGE_LIMIT,
            &PlacementConstraints::default(),
        )
        .expect("sparse model");
        let y_count = sparse_model
            .lp
            .vars
            .iter()
            .filter(|var| var.label.starts_with("y_"))
            .count();
        assert_eq!(
            y_count, 4,
            "each edge allocates only src-domain × dst-domain route variables"
        );
    }

    #[test]
    fn multilevel_refinement_preserves_the_selected_parent_row() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let solver = ChooseSolver::with_preferences(vec![
            Coor::span(0, 2, 1, 2).region_name(),
            Coor::slot(1, 2).region_name(),
        ]);
        let result = floorplan_with_strategy(
            &graph,
            &device,
            DEFAULT_USAGE_LIMIT,
            (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
            PartitionStrategy::MultiLevel,
            &solver,
            &SolveOpts::default(),
        )
        .expect("multilevel floorplan");
        assert!(
            result
                .regions
                .values()
                .all(|region| region == &Coor::slot(1, 2).region_name()),
            "second-pass candidates must remain within the first-pass row"
        );
    }

    #[test]
    fn multilevel_recovers_when_an_aggregate_parent_has_no_feasible_child() {
        let graph = single_task_floor_graph_with_area(Area {
            bram_18k: 75,
            ..Area::default()
        });
        let slot = |x, y, bram_18k| Slot {
            x,
            y,
            area: Area {
                bram_18k,
                ..Area::default()
            },
            centroid_x: i64::from(x) * 100,
            centroid_y: i64::from(y) * 100,
            pblock_ranges: Vec::new(),
            wire_cap: DirCaps::default(),
            anchor: DirRegions::default(),
            tags: Vec::new(),
        };
        let device = Device {
            key: "non-decomposable-row".to_string(),
            part_num: "xctoy".to_string(),
            platform_name: None,
            rows: 2,
            cols: 2,
            pp_dist: 1,
            is_versal: false,
            user_pblock_name: None,
            slots: vec![
                slot(0, 0, 50),
                slot(1, 0, 50),
                slot(0, 1, 100),
                slot(1, 1, 0),
            ],
        };
        let solver = RecordingSolver::first();

        let result = floorplan_with_strategy(
            &graph,
            &device,
            1.0,
            1.0_f64.max(MAX_USAGE_LIMIT),
            PartitionStrategy::MultiLevel,
            &solver,
            &SolveOpts::default(),
        )
        .expect("the globally feasible atomic placement must survive a bad provisional row");

        assert_eq!(
            result.regions.get("A_0"),
            Some(&Coor::slot(0, 1).region_name())
        );
        assert_eq!(
            solver.models.lock().expect("lock").len(),
            2,
            "the parent-filtered refinement has no domain, then one atomic fallback is solved"
        );
    }

    #[test]
    fn multilevel_infeasible_refinement_reuses_the_flat_atomic_formulation() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let logic_limit = 0.7;
        let block_limit = 0.8;
        let solver = RejectFirstRefinementSolver::new();

        floorplan_with_exact_resource_caps(
            &graph,
            &device,
            logic_limit,
            block_limit,
            PartitionStrategy::MultiLevel,
            &solver,
            &SolveOpts::default(),
        )
        .expect("a proven parent-refinement failure must retry atomic placement globally");

        let models = solver.models.lock().expect("lock");
        assert_eq!(models.len(), 3, "row, parent refinement, atomic fallback");
        let constraints = exact_resource_cap_constraints(&device, block_limit);
        let regions = atomic_regions(&device);
        let domains = candidate_domains(&graph, &device, &regions, logic_limit, None, &constraints)
            .expect("flat domains");
        let expected = FloorplanModel::build(
            &graph,
            &device,
            &domains,
            &find_cuts_for_regions(&device, &regions),
            logic_limit,
            &constraints,
        )
        .expect("flat model");
        assert_eq!(
            crate::solver::write_cplex_lp(&models[2]).expect("render fallback model"),
            crate::solver::write_cplex_lp(&expected.lp).expect("render expected model"),
            "fallback must use the existing atomic variables, rows, and objective unchanged"
        );
    }

    #[test]
    fn readback_rejects_missing_and_fractional_assignments() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let regions = [Coor::slot(0, 0), Coor::slot(1, 0)];
        let domains = vec![regions.to_vec(); graph.vertices().len()];
        let model = FloorplanModel::build(
            &graph,
            &device,
            &domains,
            &[],
            DEFAULT_USAGE_LIMIT,
            &PlacementConstraints::default(),
        )
        .expect("model");

        let missing = LpSolution {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: HashMap::new(),
        };
        assert!(matches!(
            model.read_back(&graph, &domains, &missing),
            Err(IlpError::InvalidSolution(_))
        ));

        let mut values = HashMap::new();
        for row in &model.x {
            values.insert(row[0], 0.5);
            values.insert(row[1], 0.5);
        }
        let fractional = LpSolution {
            status: LpStatus::Optimal,
            objective: 0.0,
            values,
        };
        assert!(matches!(
            model.read_back(&graph, &domains, &fractional),
            Err(IlpError::InvalidSolution(_))
        ));
    }

    #[test]
    fn resource_overrides_use_total_region_capacity() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let region = Coor::slot(0, 0);
        let domains = vec![vec![region]; graph.vertices().len()];
        let mut constraints = PlacementConstraints::default();
        constraints
            .max_resource_limits
            .insert(region.region_name(), BTreeMap::from([(Resource::Lut, 0.5)]));
        constraints
            .min_resource_limits
            .insert(region.region_name(), BTreeMap::from([(Resource::Lut, 0.1)]));
        let model = FloorplanModel::build(
            &graph,
            &device,
            &domains,
            &[],
            DEFAULT_USAGE_LIMIT,
            &constraints,
        )
        .expect("model");
        let total_lut = u64_as_f64(device.island_area(&region).expect("area").lut);
        let max = model
            .lp
            .constraints
            .iter()
            .find(|constraint| {
                constraint.name == format!("node_{}_LUT_usage", region.region_name())
            })
            .expect("max");
        let min = model
            .lp
            .constraints
            .iter()
            .find(|constraint| constraint.name.ends_with("_LUT_usage_ge"))
            .expect("min");
        assert!(
            (max.rhs - total_lut * 0.5).abs() < f64::EPSILON,
            "slot-specific maximum is based on total, not globally derated, capacity"
        );
        assert!(
            (min.rhs - total_lut * 0.1).abs() < f64::EPSILON,
            "slot-specific minimum is based on total capacity"
        );
    }

    #[test]
    fn exact_multilevel_caps_apply_to_row_and_atomic_resource_rows() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let solver = RecordingSolver::first();
        let logic_limit = 0.7;
        let block_limit = 0.8;

        let (_, strategy) = floorplan_with_exact_resource_caps(
            &graph,
            &device,
            logic_limit,
            block_limit,
            PartitionStrategy::MultiLevel,
            &solver,
            &SolveOpts::default(),
        )
        .expect("exact multilevel floorplan");
        assert_eq!(strategy, PartitionStrategy::MultiLevel);

        let baseline_solver = RecordingSolver::first();
        floorplan_with_strategy(
            &graph,
            &device,
            logic_limit,
            logic_limit,
            PartitionStrategy::MultiLevel,
            &baseline_solver,
            &SolveOpts::default(),
        )
        .expect("single-cap multilevel floorplan");

        let models = solver.models.lock().expect("lock");
        let baseline_models = baseline_solver.models.lock().expect("lock");
        assert_eq!(models.len(), 2, "one row solve and one atomic solve");
        assert_eq!(baseline_models.len(), models.len());
        for ((model, baseline), region) in models
            .iter()
            .zip(baseline_models.iter())
            .zip([Coor::span(0, 0, device.cols - 1, 0), Coor::slot(0, 0)])
        {
            assert_eq!(model.num_vars(), baseline.num_vars());
            assert_eq!(model.num_constraints(), baseline.num_constraints());
            assert_eq!(
                model
                    .vars
                    .iter()
                    .map(|variable| variable.label.as_str())
                    .collect::<Vec<_>>(),
                baseline
                    .vars
                    .iter()
                    .map(|variable| variable.label.as_str())
                    .collect::<Vec<_>>(),
                "the cap policy must not add or remove variables",
            );
            assert_eq!(model.objective, baseline.objective);
            for row in &model.constraints {
                let baseline_row = baseline
                    .constraints
                    .iter()
                    .find(|candidate| candidate.name == row.name)
                    .expect("baseline row");
                assert_eq!(row.op, baseline_row.op);
                assert_eq!(row.expr, baseline_row.expr);
            }

            let area = device.island_area(&region).expect("region area");
            for resource in Resource::ALL {
                let row = model
                    .constraints
                    .iter()
                    .find(|row| {
                        row.name
                            == format!("node_{}_{}_usage", region.region_name(), resource.name())
                    })
                    .expect("resource row");
                let expected = match resource {
                    Resource::Ff | Resource::Lut => {
                        u64_as_f64(scaled_amount(resource.amount(&area), logic_limit))
                    }
                    Resource::Bram18k | Resource::Dsp | Resource::Uram => {
                        u64_as_f64(resource.amount(&area)) * block_limit
                    }
                };
                assert_eq!(
                    row.rhs.to_bits(),
                    expected.to_bits(),
                    "{} cap drifted for {}",
                    resource.name(),
                    region.region_name(),
                );
            }
        }
    }

    #[test]
    fn exact_flat_candidates_keep_one_resource_cap() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let solver = RecordingSolver::first();
        let logic_limit = 0.7;

        let (_, strategy) = floorplan_with_exact_resource_caps(
            &graph,
            &device,
            logic_limit,
            0.8,
            PartitionStrategy::Flat,
            &solver,
            &SolveOpts::default(),
        )
        .expect("exact flat floorplan");
        assert_eq!(strategy, PartitionStrategy::Flat);

        let models = solver.models.lock().expect("lock");
        assert_eq!(models.len(), 1);
        let region = Coor::slot(0, 0);
        let area = device.island_area(&region).expect("slot area");
        for resource in Resource::ALL {
            let row = models[0]
                .constraints
                .iter()
                .find(|row| {
                    row.name == format!("node_{}_{}_usage", region.region_name(), resource.name())
                })
                .expect("resource row");
            let expected = u64_as_f64(scaled_amount(resource.amount(&area), logic_limit));
            assert_eq!(
                row.rhs.to_bits(),
                expected.to_bits(),
                "flat {} unexpectedly received a block margin",
                resource.name(),
            );
        }
    }

    #[test]
    fn exact_multilevel_candidate_filter_honors_block_overrides() {
        let graph = single_task_floor_graph_with_area(Area {
            bram_18k: 75,
            ..Area::default()
        });
        let mut device = one_slot_device(1_000);
        device.slots[0].area.bram_18k = 100;

        floorplan_with_exact_resource_caps(
            &graph,
            &device,
            0.7,
            0.8,
            PartitionStrategy::MultiLevel,
            &ChooseSolver::first(),
            &SolveOpts::default(),
        )
        .expect("the block margin must keep the task in the candidate domain");

        assert!(matches!(
            floorplan_with_exact_resource_caps(
                &graph,
                &device,
                0.7,
                0.8,
                PartitionStrategy::Flat,
                &ChooseSolver::first(),
                &SolveOpts::default(),
            ),
            Err(IlpError::NoCandidates { .. })
        ));
    }

    #[test]
    fn rectangular_centroid_coefficients_preserve_half_units() {
        let mk_slot = |x, centroid_x| Slot {
            x,
            y: 0,
            area: Area {
                lut: 1000,
                ..Area::default()
            },
            centroid_x,
            centroid_y: 0,
            pblock_ranges: Vec::new(),
            wire_cap: DirCaps::default(),
            anchor: DirRegions::default(),
            tags: Vec::new(),
        };
        let device = Device {
            key: "odd".to_string(),
            part_num: "odd".to_string(),
            platform_name: None,
            rows: 1,
            cols: 3,
            pp_dist: 1,
            is_versal: false,
            user_pblock_name: None,
            slots: vec![mk_slot(0, 0), mk_slot(1, 1), mk_slot(2, 4)],
        };
        let graph = vadd_floor_graph();
        let domains = vec![vec![Coor::span(0, 0, 1, 0)], vec![Coor::slot(2, 0)]];
        let model = FloorplanModel::build(
            &graph,
            &device,
            &domains,
            &[],
            DEFAULT_USAGE_LIMIT,
            &PlacementConstraints::default(),
        )
        .expect("model");
        assert!(
            model
                .lp
                .objective
                .terms
                .iter()
                .any(|(coefficient, _)| (*coefficient - 122.5).abs() < 1e-9),
            "35-bit physical stream times the exact 3.5-unit centroid distance"
        );
    }

    #[test]
    fn strategy_matches_expected_thresholds() {
        let u280 = select_device("u280").expect("u280");
        assert_eq!(select_strategy(&u280, 299), PartitionStrategy::Flat);
        assert_eq!(select_strategy(&u280, 300), PartitionStrategy::MultiLevel);
        assert_eq!(select_strategy(&u280, 801), PartitionStrategy::MultiLevel);

        let vck = select_device("vck190").expect("vck190");
        assert_eq!(select_strategy(&vck, 300), PartitionStrategy::Flat);
    }

    #[test]
    fn auto_strategy_counts_unique_endpoint_pairs() {
        let graph = parallel_floor_graph(300);
        let device = select_device("u280").expect("u280");

        assert_eq!(graph.streams().len(), 300);
        assert_eq!(graph.placement_edges().len(), 1);
        assert_eq!(
            resolve_strategy(&graph, &device, PartitionStrategy::Auto),
            PartitionStrategy::Flat,
            "parallel FIFOs form one placement edge for schedule selection"
        );
    }

    #[test]
    fn flat_floorplan_assigns_every_vertex_without_cbc() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let result = floorplan_with_strategy(
            &graph,
            &device,
            DEFAULT_USAGE_LIMIT,
            (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
            PartitionStrategy::Flat,
            &ChooseSolver::first(),
            &SolveOpts::default(),
        )
        .expect("placement");
        assert_eq!(result.regions.len(), graph.vertices().len());
    }

    #[test]
    fn invalid_usage_limit_is_rejected_before_model_building() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        assert!(matches!(
            floorplan_with_strategy(
                &graph,
                &device,
                0.0,
                0.0_f64.max(MAX_USAGE_LIMIT),
                PartitionStrategy::Flat,
                &ChooseSolver::first(),
                &SolveOpts::default()
            ),
            Err(IlpError::InvalidLimit { .. })
        ));

        let zero_override = PlacementConfig {
            constraints: PlacementConstraints {
                max_resource_limits: BTreeMap::from([(
                    Coor::slot(0, 0).region_name(),
                    BTreeMap::from([(Resource::Lut, 0.0)]),
                )]),
                ..PlacementConstraints::default()
            },
            ..PlacementConfig::default()
        };
        validate_config(&zero_override).expect("a zero override can intentionally empty a slot");
    }

    #[test]
    fn area_limited_empty_domain_retries_through_the_usage_ceiling() {
        // The first case becomes legal on the regular 0.72 step. The second
        // remains illegal at 0.94 and verifies that the exact 0.95 ceiling is
        // attempted instead of being skipped by a 0.02 increment.
        for lut in [719, 949] {
            let graph = single_task_floor_graph(lut);
            let result = floorplan_with_strategy(
                &graph,
                &one_slot_device(1000),
                DEFAULT_USAGE_LIMIT,
                (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
                PartitionStrategy::Flat,
                &ChooseSolver::first(),
                &SolveOpts::default(),
            )
            .expect("area-only domain failure should retry");
            assert_eq!(
                result.regions.get("A_0"),
                Some(&Coor::slot(0, 0).region_name())
            );
        }
    }

    #[test]
    fn exact_usage_limit_disables_infeasibility_retries() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");

        let ordinary_solver = StatusSolver::new(LpStatus::Infeasible);
        assert!(matches!(
            floorplan_with_strategy(&graph, &device, DEFAULT_USAGE_LIMIT, (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT), PartitionStrategy::Flat, &ordinary_solver, &SolveOpts::default()),
            Err(IlpError::Infeasible(limit)) if limit == MAX_USAGE_LIMIT
        ));
        assert!(
            ordinary_solver.calls.load(Ordering::Relaxed) > 1,
            "ordinary floorplanning must retain utilization retries",
        );

        let exact_solver = StatusSolver::new(LpStatus::Infeasible);
        assert!(matches!(
            floorplan_with_strategy(
                &graph,
                &device,
                DEFAULT_USAGE_LIMIT,
                DEFAULT_USAGE_LIMIT,
                PartitionStrategy::Flat,
                &exact_solver,
                &SolveOpts::default(),
            ),
            Err(IlpError::Infeasible(limit)) if limit == DEFAULT_USAGE_LIMIT
        ));
        assert_eq!(
            exact_solver.calls.load(Ordering::Relaxed),
            1,
            "an exact DSE candidate must perform only its requested solve",
        );
    }

    #[test]
    fn zero_area_vertex_fits_a_zero_derated_slot() {
        // A radically small usage limit floors every scaled slot resource to
        // zero; a resource-free vertex still fits (0 <= 0) and must not be
        // rejected as candidate-less.
        let graph = single_task_floor_graph(0);
        let device = one_slot_device(1000);
        let assignment = floorplan_with_strategy(
            &graph,
            &device,
            1e-12,
            1e-12,
            PartitionStrategy::Flat,
            &ChooseSolver::first(),
            &SolveOpts::default(),
        )
        .expect("the fitting check alone decides a zero-area vertex");
        assert_eq!(assignment.regions.len(), 1);
    }

    #[test]
    fn permanent_pin_conflict_does_not_retry_or_solve() {
        let graph = single_task_floor_graph(1);
        let device = one_slot_device(1000);
        let solver = StatusSolver::new(LpStatus::Infeasible);
        let config = PlacementConfig {
            strategy: PartitionStrategy::Flat,
            constraints: PlacementConstraints {
                vertex_regions: BTreeMap::from([("A_0".to_string(), Coor::slot(1, 0))]),
                ..PlacementConstraints::default()
            },
            ..PlacementConfig::default()
        };

        assert!(matches!(
            floorplan_with_config(&graph, &device, &config, &solver, &SolveOpts::default()),
            Err(IlpError::NoCandidates { vertex }) if vertex == "A_0"
        ));
        assert_eq!(
            solver.calls.load(Ordering::Relaxed),
            0,
            "a utilization increase cannot repair a permanent pin conflict"
        );
    }

    #[test]
    fn unsolved_status_is_not_disguised_as_a_utilization_retry() {
        let graph = vadd_floor_graph();
        let device = select_device("u280").expect("u280");
        let solver = StatusSolver::new(LpStatus::NotSolved);
        assert!(matches!(
            floorplan_with_strategy(
                &graph,
                &device,
                DEFAULT_USAGE_LIMIT,
                (DEFAULT_USAGE_LIMIT).max(MAX_USAGE_LIMIT),
                PartitionStrategy::Flat,
                &solver,
                &SolveOpts::default()
            ),
            Err(IlpError::NoIncumbent(LpStatus::NotSolved))
        ));
        assert_eq!(
            solver.calls.load(Ordering::Relaxed),
            1,
            "only proven infeasibility may trigger a higher-utilization solve"
        );
    }
}
