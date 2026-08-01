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

use crate::device::model::{
    add_area, Coor, Device, Resource, DEFAULT_USAGE_LIMIT, VERTICAL_DIST_PENALTY,
};
use crate::graph::{FloorGraph, PlacementEdge};
use crate::partition::cut::{find_cuts_for_regions, Cut};
use crate::solver::assign::{add_one_of_k_row, read_one_of_k, OneOfKError};
use crate::solver::sparse::SparseRow;
use crate::solver::{
    Comparison, LinExpr, LpModel, LpStatus, LpVar, Sense, SolveOpts, Solver, SolverError,
};

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
/// multilevel once the device has at least this many squared rows.
const MULTILEVEL_SCHEDULE_MIN_SQUARED_ROWS: u64 = 8;

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
    if device.cols == 1 || edge_count < FLAT_SCHEDULE_MAX_EDGE_COUNT {
        return PartitionStrategy::Flat;
    }
    if edge_count > MULTILEVEL_SCHEDULE_EDGE_COUNT {
        return PartitionStrategy::MultiLevel;
    }
    if u64::from(device.rows) * u64::from(device.rows) >= MULTILEVEL_SCHEDULE_MIN_SQUARED_ROWS {
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
    // Cuts depend only on the iteration's regions, not on the derating being
    // retried; compute them once.
    let cuts = find_cuts_for_regions(device, regions);

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
        let mut model = FloorplanModel::build(
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
            // Lexicographic tie-break: pin the achieved objective, then
            // minimize a stable candidate ranking so translation-equivalent
            // optima resolve identically across solver versions.
            let refined = model.refine_lexicographic(solver, opts, &solution)?;
            return model.read_back(graph, &domains, refined.as_ref().unwrap_or(&solution));
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
            let mut terms = SparseRow::new();
            for (vi, vertex) in graph.vertices().iter().enumerate() {
                if let Some(ci) = domains[vi]
                    .iter()
                    .position(|candidate| *candidate == region)
                {
                    terms.push(u64_as_f64(resource.amount(&vertex.area)), x[vi][ci]);
                }
            }

            let max_rhs = lookup_limit(&constraints.max_resource_limits, &region, resource)
                .map_or_else(
                    || u64_as_f64(scaled_amount(resource.amount(&total), usage_limit)),
                    |limit| u64_as_f64(resource.amount(&total)) * limit,
                );
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
            let coefficient = f64::from(term.edge.width) * i64_as_f64(distance_twice) / 2.0;
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
mod tests;
