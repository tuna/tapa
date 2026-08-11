//! The post-placement routing MILP: choose one path per cross-slot net without
//! exceeding any boundary's usable wire capacity.
//!
//! Each net selects exactly one candidate path. Every bounded boundary
//! contributes a *hard* `sum(width * selected_path) <= capacity` row, using
//! the same per-boundary [`bounded_border_capacity`] budget the placement
//! cuts model, so a time-limited incumbent is always a physically legal
//! route. The objective minimizes the worst normalized utilization over the
//! positive-capacity boundaries; on a device with no bounded boundary at all
//! it minimizes total hop count instead. There is deliberately no bend-count
//! objective. A lexicographic second solve then pins the achieved objective
//! and minimizes the stable candidate-path index, so equal-quality routes
//! resolve deterministically across solver versions.

use std::collections::BTreeSet;

use crate::device::model::{bounded_border_capacity, Device};
use crate::route::paths::{enumerate_paths, Cell};
use crate::solver::assign::{add_one_of_k_row, read_one_of_k, OneOfKError};
use crate::solver::sparse::SparseRow;
use crate::solver::{
    Comparison, LinExpr, LpModel, LpStatus, LpVar, Sense, SolveOpts, Solver, SolverError,
};
use crate::ExactInt;

/// Maximum extra slot visits in a generated candidate path.
const MAX_DETOUR: usize = 2;
/// Absolute MIP gap requested when the objective is worst-boundary
/// utilization: a tenth of a percent of a boundary's budget is not worth more
/// search, and the lexicographic refinement then resolves the selection
/// deterministically.
///
/// It is deliberately not applied to the hop-count fallback objective, whose
/// units are whole hops — there the same number would be an expensive way of
/// asking for the exact optimum.
const UTILIZATION_MIP_GAP_ABS: f64 = 0.001;

/// A net to route: a width-weighted connection between two slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteNet {
    /// Source slot.
    pub src: Cell,
    /// Destination slot.
    pub dst: Cell,
    /// Bundled wire width.
    pub width: u32,
}

/// Why routing failed to produce paths.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    /// The solver failed to run or parse.
    #[error(transparent)]
    Solver(#[from] SolverError),
    /// The solver proved that the routing MILP is infeasible.
    #[error("routing is infeasible: {0}")]
    Infeasible(String),
    /// The solver neither proved infeasibility nor returned a usable
    /// incumbent.
    #[error("routing solver returned no usable incumbent ({0:?})")]
    NoIncumbent(LpStatus),
    /// A net endpoint is not a slot in the selected device.
    #[error("net {net_index} has invalid {endpoint} slot ({cell_x}, {cell_y})")]
    InvalidEndpoint {
        /// Net position in the input slice.
        net_index: usize,
        /// `source` or `destination`.
        endpoint: &'static str,
        /// Invalid x coordinate.
        cell_x: u32,
        /// Invalid y coordinate.
        cell_y: u32,
    },
    /// A generated route is structurally invalid.
    #[error("invalid candidate path for net {net_index}: {reason}")]
    InvalidPath {
        /// Net position in the input slice.
        net_index: usize,
        /// Failed invariant.
        reason: String,
    },
    /// The solver claimed success but did not return an integral, complete,
    /// capacity-respecting selected-path assignment.
    #[error("invalid routing solution: {0}")]
    InvalidSolution(String),
}

/// One inter-slot boundary and its usable crossing capacity, if bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Boundary {
    a: Cell,
    b: Cell,
    /// Usable wire budget across this boundary; `None` when both facing
    /// declarations are unconstrained.
    capacity: Option<u64>,
}

/// Collect every adjacent-grid boundary in deterministic row-major order.
fn boundaries(device: &Device) -> Vec<Boundary> {
    let mut out = Vec::new();
    for y in 0..device.rows {
        for x in 0..device.cols {
            let Some(slot) = device.slot(x, y) else {
                continue;
            };
            if x + 1 < device.cols {
                if let Some(right) = device.slot(x + 1, y) {
                    out.push(Boundary {
                        a: (x, y),
                        b: (x + 1, y),
                        capacity: bounded_border_capacity(slot.wire_cap.east, right.wire_cap.west),
                    });
                }
            }
            if y + 1 < device.rows {
                if let Some(up) = device.slot(x, y + 1) {
                    out.push(Boundary {
                        a: (x, y),
                        b: (x, y + 1),
                        capacity: bounded_border_capacity(slot.wire_cap.north, up.wire_cap.south),
                    });
                }
            }
        }
    }
    out
}

/// Whether `path` uses the hop across `boundary` (in either direction).
fn path_crosses(path: &[Cell], boundary: &Boundary) -> bool {
    path.windows(2).any(|hop| {
        (hop[0] == boundary.a && hop[1] == boundary.b)
            || (hop[0] == boundary.b && hop[1] == boundary.a)
    })
}

/// Check endpoint, grid, simplicity, and adjacency invariants for a route.
fn validate_path(
    net_index: usize,
    net: &RouteNet,
    path: &[Cell],
    device: &Device,
) -> Result<(), RouteError> {
    if path.first().copied() != Some(net.src) {
        return Err(RouteError::InvalidPath {
            net_index,
            reason: format!("path must start at {:?}", net.src),
        });
    }
    if path.last().copied() != Some(net.dst) {
        return Err(RouteError::InvalidPath {
            net_index,
            reason: format!("path must end at {:?}", net.dst),
        });
    }
    if let Some(&cell) = path.iter().find(|&&(x, y)| device.slot(x, y).is_none()) {
        return Err(RouteError::InvalidPath {
            net_index,
            reason: format!("slot {cell:?} is outside the device"),
        });
    }
    if path.iter().copied().collect::<BTreeSet<_>>().len() != path.len() {
        return Err(RouteError::InvalidPath {
            net_index,
            reason: "path revisits a slot".to_string(),
        });
    }
    if let Some([from, to]) = path
        .windows(2)
        .find(|hop| hop[0].0.abs_diff(hop[1].0) + hop[0].1.abs_diff(hop[1].1) != 1)
    {
        return Err(RouteError::InvalidPath {
            net_index,
            reason: format!("non-adjacent hop {from:?} -> {to:?}"),
        });
    }
    Ok(())
}

/// Validate net endpoints before candidate generation.
fn validate_nets(nets: &[RouteNet], device: &Device) -> Result<(), RouteError> {
    for (net_index, net) in nets.iter().enumerate() {
        for (endpoint, cell) in [("source", net.src), ("destination", net.dst)] {
            if device.slot(cell.0, cell.1).is_none() {
                return Err(RouteError::InvalidEndpoint {
                    net_index,
                    endpoint,
                    cell_x: cell.0,
                    cell_y: cell.1,
                });
            }
        }
    }
    Ok(())
}

/// Candidate paths for every net, sharing one enumeration per distinct
/// `(src, dst)` pair.
///
/// A design routes thousands of nets across a handful of slot pairs, and the
/// depth-first enumeration is the expensive part; the same pair always yields
/// the same paths.
struct Candidates {
    /// One path set per distinct endpoint pair, in first-use order.
    sets: Vec<Vec<Vec<Cell>>>,
    /// Which set each net uses, in net input order.
    of_net: Vec<usize>,
}

impl Candidates {
    /// The candidate paths of one net.
    fn paths(&self, net_index: usize) -> &[Vec<Cell>] {
        &self.sets[self.of_net[net_index]]
    }

    /// Every net's candidate paths, in net input order.
    fn iter(&self) -> impl Iterator<Item = &[Vec<Cell>]> {
        self.of_net.iter().map(|&set| self.sets[set].as_slice())
    }
}

/// Build the generated candidates in net input order.
fn candidates_for(nets: &[RouteNet], device: &Device) -> Result<Candidates, RouteError> {
    let mut sets: Vec<Vec<Vec<Cell>>> = Vec::new();
    let mut of_pair = std::collections::BTreeMap::<(Cell, Cell), usize>::new();
    let mut of_net = Vec::with_capacity(nets.len());

    for (net_index, net) in nets.iter().enumerate() {
        if let Some(&set) = of_pair.get(&(net.src, net.dst)) {
            of_net.push(set);
            continue;
        }
        let paths = enumerate_paths(net.src, net.dst, device.cols, device.rows, MAX_DETOUR);
        if paths.is_empty() {
            return Err(RouteError::Infeasible(format!(
                "net {net_index} has no path from {:?} to {:?} within {MAX_DETOUR} extra hops",
                net.src, net.dst,
            )));
        }
        // Every invariant `validate_path` checks — endpoints, in-grid, simple,
        // adjacent — depends only on the pair and the device, so validating the
        // first net that uses a set covers every later net that shares it.
        for path in &paths {
            validate_path(net_index, net, path, device)?;
        }
        sets.push(paths);
        of_pair.insert((net.src, net.dst), sets.len() - 1);
        of_net.push(sets.len() - 1);
    }
    Ok(Candidates { sets, of_net })
}

/// Order the path choices of nets that share endpoints and width.
///
/// Such nets are interchangeable: permuting their choices leaves the objective
/// and every boundary load untouched, because the model sees only endpoints and
/// width. Without this the search explores the same routing once per
/// permutation — factorially many for a wide bundle between two slots.
fn add_symmetry_rows(lp: &mut LpModel, nets: &[RouteNet], path_vars: &[Vec<LpVar>]) {
    let rank_terms = |vars: &[LpVar], sign: f64, terms: &mut SparseRow| {
        for (rank, &var) in vars.iter().enumerate() {
            let rank = u64::try_from(rank).expect("candidate count fits u64");
            if rank > 0 {
                terms.push(sign * rank.as_f64(), var);
            }
        }
    };

    let mut previous = std::collections::BTreeMap::<(Cell, Cell, u32), usize>::new();
    for (net_index, net) in nets.iter().enumerate() {
        let Some(earlier) = previous.insert((net.src, net.dst, net.width), net_index) else {
            continue;
        };
        let mut terms = SparseRow::new();
        rank_terms(&path_vars[earlier], 1.0, &mut terms);
        rank_terms(&path_vars[net_index], -1.0, &mut terms);
        lp.add_constraint(
            format!("symmetry_{earlier}_{net_index}"),
            terms.into_expr(),
            Comparison::Le,
            0.0,
        );
    }
}

/// Read exactly one integral path choice for each net. Every path variable
/// must be present; there is no implicit-zero or path-zero fallback.
fn selected_routes(
    candidates: &Candidates,
    path_vars: &[Vec<LpVar>],
    solution: &crate::solver::LpSolution,
) -> Result<Vec<Vec<Cell>>, RouteError> {
    let mut routes = Vec::with_capacity(path_vars.len());
    for (net_index, (paths, vars)) in candidates.iter().zip(path_vars).enumerate() {
        let path_index = read_one_of_k(solution, vars).map_err(|error| match error {
            OneOfKError::MissingVariable { position } => RouteError::InvalidSolution(format!(
                "solver omitted path variable for net {net_index}, candidate {position}"
            )),
            OneOfKError::NonBinary {
                position, value, ..
            } => RouteError::InvalidSolution(format!(
                "path variable for net {net_index}, candidate {position} is not binary: {value}"
            )),
            OneOfKError::SelectionCount { selected: 0, .. } => {
                RouteError::InvalidSolution(format!("no path selected for net {net_index}"))
            }
            OneOfKError::SelectionCount { .. } => RouteError::InvalidSolution(format!(
                "more than one path selected for net {net_index}"
            )),
        })?;
        routes.push(paths[path_index].to_vec());
    }
    Ok(routes)
}

/// Defensive recount: selected paths must respect every bounded capacity.
///
/// Solution validation in the solver backend already checks each hard
/// capacity row; this integer recount keeps the guarantee independent of
/// solver floating-point tolerances (and of test doubles).
fn validate_within_capacity(
    nets: &[RouteNet],
    routes: &[Vec<Cell>],
    limits: &[Boundary],
) -> Result<(), RouteError> {
    for boundary in limits {
        let Some(capacity) = boundary.capacity else {
            continue;
        };
        let crossing: u64 = nets
            .iter()
            .zip(routes)
            .filter(|(_, path)| path_crosses(path, boundary))
            .map(|(net, _)| u64::from(net.width))
            .sum();
        if crossing > capacity {
            return Err(RouteError::InvalidSolution(format!(
                "boundary {:?}->{:?} carries {crossing} wires, exceeding its capacity {capacity}",
                boundary.a, boundary.b,
            )));
        }
    }
    Ok(())
}

/// Name the boundary that is most over-subscribed when every net takes its
/// shortest path, so an infeasible route says which wall it hit.
///
/// Candidates are enumerated shortest-first, so path zero is a shortest path.
/// The router may spread traffic better than this, which is exactly why the
/// message reports the shortest-path demand as evidence rather than as proof.
fn describe_congestion(nets: &[RouteNet], candidates: &Candidates, limits: &[Boundary]) -> String {
    let worst = limits
        .iter()
        .filter_map(|boundary| {
            let capacity = boundary.capacity?;
            let demand: u64 = nets
                .iter()
                .zip(candidates.iter())
                .filter(|(_, paths)| path_crosses(&paths[0], boundary))
                .map(|(net, _)| u64::from(net.width))
                .sum();
            (demand > capacity).then_some((demand, capacity, boundary))
        })
        .max_by_key(|(demand, capacity, _)| (demand - capacity, *demand));

    match worst {
        Some((demand, capacity, boundary)) => format!(
            "with every net on a shortest path the {:?}-{:?} boundary carries {demand} wires \
             against a budget of {capacity}; place the endpoints closer together, or widen the \
             boundary's declared wire capacity",
            boundary.a, boundary.b,
        ),
        None => "no single boundary is over-subscribed on shortest paths, so the nets cannot be \
                 spread across the available detours"
            .to_string(),
    }
}

/// The total-hop objective over the path variables, used on devices with no
/// bounded boundary and to pin an unconstrained primary solve.
fn hop_objective(candidates: &Candidates, path_vars: &[Vec<LpVar>]) -> LinExpr {
    let mut terms = SparseRow::new();
    for (paths, vars) in candidates.iter().zip(path_vars) {
        for (path, &var) in paths.iter().zip(vars) {
            let hops = u32::try_from(path.len().saturating_sub(1)).expect("hop count fits u32");
            if hops > 0 {
                terms.push(f64::from(hops), var);
            }
        }
    }
    terms.into_expr()
}

/// Pin the achieved primary objective and re-solve minimizing the stable
/// candidate-path index, whose optimum is unique per net (candidates are
/// enumerated shortest/simplest first). Equal-quality routes therefore
/// resolve deterministically across solver versions. Falls back to the
/// primary incumbent when the refinement yields none.
fn refine_lexicographic(
    lp: &mut LpModel,
    solver: &dyn Solver,
    opts: &SolveOpts,
    primary: &crate::solver::LpSolution,
    max_crossings: Option<LpVar>,
    candidates: &Candidates,
    path_vars: &[Vec<LpVar>],
) -> Result<Option<crate::solver::LpSolution>, RouteError> {
    let pin = max_crossings.map_or_else(
        || hop_objective(candidates, path_vars),
        |max_crossings| LinExpr::sum([(1.0, max_crossings)]),
    );
    Ok(crate::solver::lexicographic::refine(
        lp,
        solver,
        opts,
        pin,
        primary.objective,
        path_vars,
    )?)
}

/// Route every net, returning a chosen slot path (`src` first, `dst` last) per
/// net in input order.
pub fn route_nets(
    nets: &[RouteNet],
    device: &Device,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Vec<Vec<Cell>>, RouteError> {
    if nets.is_empty() {
        return Ok(Vec::new());
    }
    validate_nets(nets, device)?;
    let candidates = candidates_for(nets, device)?;
    let limits = boundaries(device);

    let mut lp = LpModel::new(Sense::Minimize);
    // Created first so it stays `x0` whenever the balancing objective applies.
    let max_crossings = limits
        .iter()
        .any(|boundary| boundary.capacity.is_some_and(|capacity| capacity > 0))
        .then(|| lp.add_continuous("max_crossings", 0.0, f64::INFINITY));

    // One binary per candidate path; exactly one per net.
    let mut path_vars: Vec<Vec<LpVar>> = Vec::with_capacity(nets.len());
    for net_index in 0..nets.len() {
        path_vars.push(add_one_of_k_row(
            &mut lp,
            &format!("net_{net_index}"),
            candidates.paths(net_index).len(),
            |path_index| format!("p_{net_index}_{path_index}"),
        ));
    }
    add_symmetry_rows(&mut lp, nets, &path_vars);

    // Per bounded boundary: a hard capacity row, so every incumbent — even a
    // time-limited one — is a physically legal route; plus, when balancing,
    // the normalization row bounding `max_crossings`.
    for (boundary_index, boundary) in limits.iter().enumerate() {
        let Some(capacity) = boundary.capacity else {
            continue;
        };
        let mut crossings = SparseRow::new();
        for (net_index, paths) in candidates.iter().enumerate() {
            for (path_index, path) in paths.iter().enumerate() {
                if path_crosses(path, boundary) {
                    crossings.push(
                        f64::from(nets[net_index].width),
                        path_vars[net_index][path_index],
                    );
                }
            }
        }
        let mut normalization = crossings.clone();
        lp.add_constraint(
            format!("bound_{boundary_index}_capacity"),
            crossings.into_expr(),
            Comparison::Le,
            capacity.as_f64(),
        );
        if let (Some(max_crossings), true) = (max_crossings, capacity > 0) {
            normalization.push(-capacity.as_f64(), max_crossings);
            lp.add_constraint(
                format!("bound_{boundary_index}"),
                normalization.into_expr(),
                Comparison::Le,
                0.0,
            );
        }
    }

    // Balance bounded-boundary utilization when one exists; otherwise prefer
    // short routes so unconstrained devices still get deterministic answers.
    let objective = max_crossings.map_or_else(
        || hop_objective(&candidates, &path_vars),
        |max_crossings| LinExpr::sum([(1.0, max_crossings)]),
    );
    lp.set_objective(objective);

    let mut route_opts = opts.clone();
    route_opts.mip_gap_abs = max_crossings.map(|_| UTILIZATION_MIP_GAP_ABS);
    let solution = solver.solve(&lp, &route_opts)?;
    if !solution.is_found() {
        return Err(match solution.status {
            LpStatus::Infeasible => {
                RouteError::Infeasible(describe_congestion(nets, &candidates, &limits))
            }
            LpStatus::NotSolved | LpStatus::Unbounded => RouteError::NoIncumbent(solution.status),
            LpStatus::Optimal | LpStatus::Feasible => {
                unreachable!("found incumbents are handled above")
            }
        });
    }

    // Lexicographic tie-break: pin the achieved objective, then minimize the
    // stable candidate-path index so equal-quality routes resolve
    // identically across solver versions.
    let refined = refine_lexicographic(
        &mut lp,
        solver,
        &route_opts,
        &solution,
        max_crossings,
        &candidates,
        &path_vars,
    )?;
    if refined.is_none() {
        log::warn!(
            "the routing lexicographic refinement found no incumbent; these routes are still \
             valid but may not reproduce across solver versions",
        );
    }
    let solution = refined.as_ref().unwrap_or(&solution);
    let routes = selected_routes(&candidates, &path_vars, solution)?;
    validate_within_capacity(nets, &routes, &limits)?;
    Ok(routes)
}

/// The short slot tag `SLOT_X{x}Y{y}` a route step carries (distinct from a
/// multi-slot region's `_TO_` tag).
#[must_use]
pub fn slot_tag(cell: Cell) -> String {
    format!("SLOT_X{}Y{}", cell.0, cell.1)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::device::model::{DirCaps, Slot};
    use crate::device::select::select_device;
    use crate::solver::{CbcSolver, LpSolution, LpStatus};
    use tapa_ir::Area;

    #[derive(Debug)]
    struct FixedSolver {
        status: LpStatus,
        objective: f64,
        values: Vec<(LpVar, f64)>,
    }

    impl Solver for FixedSolver {
        fn solve(&self, _model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            Ok(LpSolution {
                status: self.status,
                objective: self.objective,
                values: self.values.iter().copied().collect::<HashMap<_, _>>(),
            })
        }
    }

    #[derive(Debug)]
    struct PanicSolver;

    impl Solver for PanicSolver {
        fn solve(&self, _model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            panic!("invalid routing input must be rejected before solving")
        }
    }

    /// A solver answering `value` for every model variable, with `selected`
    /// forced to 1: enough to steer selection readback deterministically.
    #[derive(Debug)]
    struct DictatedSolver {
        objective: f64,
        value: f64,
        selected: Option<LpVar>,
    }

    impl Solver for DictatedSolver {
        fn solve(&self, model: &LpModel, _opts: &SolveOpts) -> Result<LpSolution, SolverError> {
            let mut values = model
                .vars
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    (
                        LpVar(u32::try_from(index).expect("variable count fits u32")),
                        self.value,
                    )
                })
                .collect::<HashMap<_, _>>();
            if let Some(selected) = self.selected {
                values.insert(selected, 1.0);
            }
            Ok(LpSolution {
                status: LpStatus::Optimal,
                objective: self.objective,
                values,
            })
        }
    }

    fn toy_device(direct_capacity: u64) -> Device {
        let mut slots = Vec::new();
        for y in 0..2 {
            for x in 0..2 {
                let mut wire_cap = DirCaps::default();
                if x == 0 {
                    wire_cap.east = if y == 0 { direct_capacity } else { 100 };
                } else {
                    wire_cap.west = if y == 0 { direct_capacity } else { 100 };
                }
                if y == 0 {
                    wire_cap.north = 100;
                } else {
                    wire_cap.south = 100;
                }
                slots.push(Slot {
                    x,
                    y,
                    area: Area::default(),
                    centroid_x: i64::from(x) * 100,
                    centroid_y: i64::from(y) * 150,
                    pblock_ranges: Vec::new(),
                    wire_cap,
                    tags: Vec::new(),
                });
            }
        }
        Device {
            key: "toy".to_string(),
            part_num: "xctoy".to_string(),
            platform_name: None,
            rows: 2,
            cols: 2,
            is_versal: false,
            user_pblock_name: None,
            slots,
        }
    }

    /// A 2x2 device whose boundaries are all unconstrained.
    fn toy_unbounded_device() -> Device {
        let device = toy_device(100);
        Device {
            slots: device
                .slots
                .into_iter()
                .map(|slot| Slot {
                    wire_cap: DirCaps::default(),
                    ..slot
                })
                .collect(),
            ..device
        }
    }

    fn route_with_cbc(nets: &[RouteNet], device: &Device) -> Vec<Vec<Cell>> {
        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };
        match route_nets(nets, device, &CbcSolver::new(), &opts) {
            Ok(routes) => routes,
            Err(RouteError::Solver(SolverError::Spawn { .. })) => crate::solver::missing_cbc(),
            Err(other) => panic!("routing failed: {other}"),
        }
    }

    #[test]
    fn single_path_net_routes_straight() {
        let device = select_device("u280").expect("u280");
        let nets = [RouteNet {
            src: (0, 0),
            dst: (0, 2),
            width: 33,
        }];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(
            routes[0],
            vec![(0, 0), (0, 1), (0, 2)],
            "the only non-detouring path is optimal",
        );
    }

    #[test]
    fn minimax_router_uses_a_detour_around_a_narrow_boundary() {
        let device = toy_device(5);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 10,
        }];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(
            routes[0],
            vec![(0, 0), (0, 1), (1, 1), (1, 0)],
            "the direct edge is 200% utilized while the detour is 10%",
        );
    }

    #[test]
    fn unconstrained_routing_prefers_the_shortest_path() {
        let device = toy_unbounded_device();
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 1),
            width: 1,
        }];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(
            routes[0].len(),
            3,
            "diagonal nets take a two-hop path when utilization does not bind",
        );
    }

    /// A 3x3 device where every direct, one-bend, H-V-H, and V-H-V path from
    /// (0,0) to (2,2) crosses a zero-capacity boundary; only the staircase
    /// R-U-R-U remains.
    fn staircase_only_device() -> Device {
        let mut slots = Vec::new();
        for y in 0..3 {
            for x in 0..3 {
                let mut wire_cap = DirCaps {
                    north: 100,
                    south: 100,
                    east: 100,
                    west: 100,
                };
                // Block h(1,0)-(2,0), v(0,0)-(0,1), and v(1,1)-(1,2).
                if (x, y) == (1, 0) {
                    wire_cap.east = 0;
                }
                if (x, y) == (2, 0) {
                    wire_cap.west = 0;
                }
                if (x, y) == (0, 0) {
                    wire_cap.north = 0;
                }
                if (x, y) == (0, 1) {
                    wire_cap.south = 0;
                }
                if (x, y) == (1, 1) {
                    wire_cap.north = 0;
                }
                if (x, y) == (1, 2) {
                    wire_cap.south = 0;
                }
                slots.push(Slot {
                    x,
                    y,
                    area: Area::default(),
                    centroid_x: i64::from(x) * 100,
                    centroid_y: i64::from(y) * 100,
                    pblock_ranges: Vec::new(),
                    wire_cap,
                    tags: Vec::new(),
                });
            }
        }
        Device {
            key: "staircase".to_string(),
            part_num: "xctoy".to_string(),
            platform_name: None,
            rows: 3,
            cols: 3,
            is_versal: false,
            user_pblock_name: None,
            slots,
        }
    }

    #[test]
    fn staircase_route_is_found_when_shaped_paths_are_blocked() {
        let device = staircase_only_device();
        let nets = [RouteNet {
            src: (0, 0),
            dst: (2, 2),
            width: 1,
        }];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(
            routes[0],
            vec![(0, 0), (1, 0), (1, 1), (2, 1), (2, 2)],
            "the only capacity-feasible path is the staircase",
        );
    }

    #[test]
    fn equal_utilization_routes_pick_the_first_candidate() {
        // Direct and detour crossings both top out at 10/70 utilization, so
        // the primary objective ties; the refinement must pick the first
        // (direct) candidate deterministically.
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 10,
        }];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(
            routes[0],
            vec![(0, 0), (1, 0)],
            "the lexicographic refinement breaks utilization ties",
        );
    }

    #[test]
    fn bounded_capacity_is_a_hard_constraint_for_cbc() {
        // Width 10 against a derated capacity of 7 on the only direct hop;
        // the two-hop detour crosses capacity-70 boundaries instead.
        let device = toy_device(10);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 10,
        }];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(
            routes[0],
            vec![(0, 0), (0, 1), (1, 1), (1, 0)],
            "the direct hop is forbidden by its hard capacity row",
        );
    }

    #[test]
    fn missing_path_selection_is_not_defaulted_to_path_zero() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let err = route_nets(
            &nets,
            &device,
            &DictatedSolver {
                objective: 0.0,
                value: 0.0,
                selected: None,
            },
            &SolveOpts::default(),
        )
        .expect_err("no path selected");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");
        assert!(err.to_string().contains("no path selected"), "got {err}");
    }

    #[test]
    fn omitted_path_variable_is_rejected() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let err = route_nets(
            &nets,
            &device,
            &FixedSolver {
                status: LpStatus::Optimal,
                objective: 0.0,
                values: Vec::new(),
            },
            &SolveOpts::default(),
        )
        .expect_err("omitted path variable");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");
        assert!(err.to_string().contains("omitted"), "got {err}");
    }

    #[test]
    fn fractional_path_selection_is_rejected() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let err = route_nets(
            &nets,
            &device,
            &DictatedSolver {
                objective: 0.0,
                value: 0.5,
                selected: None,
            },
            &SolveOpts::default(),
        )
        .expect_err("fractional path variable");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");
        assert!(err.to_string().contains("not binary"), "got {err}");
    }

    #[test]
    fn over_capacity_solution_is_rejected() {
        // x0 is max_crossings; per-candidate path variables follow with the
        // direct path first (shortest-first enumeration). Its derated
        // capacity is round(10 * 0.7) = 7.
        let device = toy_device(10);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 11,
        }];
        let err = route_nets(
            &nets,
            &device,
            &DictatedSolver {
                objective: 1.1,
                value: 0.0,
                selected: Some(LpVar(1)),
            },
            &SolveOpts::default(),
        )
        .expect_err("11 wires over a capacity-7 boundary is illegal");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");
        assert!(
            err.to_string().contains("exceeding its capacity 7"),
            "got {err}"
        );
    }

    #[test]
    fn zero_capacity_forbids_any_crossing() {
        let device = toy_device(0);
        // x0 is max_crossings; x1 is the direct path's variable.
        let solver = || DictatedSolver {
            objective: 0.0,
            value: 0.0,
            selected: Some(LpVar(1)),
        };

        // A positive-width net may not cross: a zero-capacity boundary carries
        // no wires, derated or not.
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let err = route_nets(&nets, &device, &solver(), &SolveOpts::default())
            .expect_err("wire over a zero-capacity boundary");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");

        // A zero-width net crosses nothing and is legal.
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 0,
        }];
        assert_eq!(
            route_nets(&nets, &device, &solver(), &SolveOpts::default())
                .expect("zero-width net crosses nothing"),
            vec![vec![(0, 0), (1, 0)]],
        );
    }

    #[test]
    fn unconstrained_device_has_no_capacity_limit() {
        // Every boundary defaults to the unconstrained sentinel, so no
        // max_crossings variable exists: x0 is the direct path's variable.
        let device = toy_unbounded_device();
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1_000_000,
        }];
        let solver = DictatedSolver {
            objective: 1.0,
            value: 0.0,
            selected: Some(LpVar(0)),
        };
        assert_eq!(
            route_nets(&nets, &device, &solver, &SolveOpts::default())
                .expect("an unconstrained device imposes no wire budget"),
            vec![vec![(0, 0), (1, 0)]],
        );
    }

    /// An infeasible route has to name the wall it hit. The router's own
    /// answer is "no", so the message reconstructs the shortest-path demand and
    /// reports the boundary that cannot carry it.
    #[test]
    fn infeasible_routing_names_the_congested_boundary() {
        let device = toy_device(0);
        let nets = [
            RouteNet {
                src: (0, 0),
                dst: (1, 0),
                width: 40,
            },
            RouteNet {
                src: (0, 1),
                dst: (1, 1),
                width: 40,
            },
        ];
        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };
        let error = match route_nets(&nets, &device, &CbcSolver::new(), &opts) {
            Err(RouteError::Solver(SolverError::Spawn { .. })) => crate::solver::missing_cbc(),
            Err(error) => error,
            Ok(routes) => panic!("a zero-capacity crossing must not route: {routes:?}"),
        };
        let message = error.to_string();
        assert!(matches!(error, RouteError::Infeasible(_)), "got {message}");
        assert!(
            message.contains("(0, 0)-(1, 0)") && message.contains("carries 40 wires"),
            "got {message}",
        );
        assert!(message.contains("budget of 0"), "got {message}");
    }

    #[test]
    fn infeasible_routing_status_is_propagated() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let err = route_nets(
            &nets,
            &device,
            &FixedSolver {
                status: LpStatus::Infeasible,
                objective: 0.0,
                values: Vec::new(),
            },
            &SolveOpts::default(),
        )
        .expect_err("an over-congested design proves infeasible");
        assert!(matches!(err, RouteError::Infeasible(_)), "got {err}");
    }

    #[test]
    fn invalid_endpoint_is_rejected_before_solving() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (2, 0),
            width: 1,
        }];
        let err = route_nets(&nets, &device, &PanicSolver, &SolveOpts::default())
            .expect_err("outside endpoint");
        assert!(
            matches!(err, RouteError::InvalidEndpoint { .. }),
            "got {err}"
        );
    }

    #[test]
    fn routing_preserves_non_infeasible_solver_statuses() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];

        for status in [LpStatus::NotSolved, LpStatus::Unbounded] {
            let err = route_nets(
                &nets,
                &device,
                &FixedSolver {
                    status,
                    objective: 0.0,
                    values: Vec::new(),
                },
                &SolveOpts::default(),
            )
            .expect_err("non-incumbent status");
            assert!(
                matches!(err, RouteError::NoIncumbent(actual) if actual == status),
                "got {err}"
            );
        }
    }

    #[test]
    fn nets_between_the_same_slots_share_one_enumeration() {
        let device = toy_device(100);
        let net = |src, dst, width| RouteNet { src, dst, width };
        let nets = [
            net((0, 0), (1, 1), 8),
            net((0, 0), (1, 1), 9),
            net((1, 1), (0, 0), 8),
        ];
        let candidates = candidates_for(&nets, &device).expect("candidates");

        assert_eq!(
            candidates.sets.len(),
            2,
            "the two forward nets share a set; the reverse net is its own pair",
        );
        assert_eq!(candidates.of_net, [0, 0, 1]);
        assert_eq!(candidates.paths(0), candidates.paths(1));
    }

    #[test]
    fn identical_nets_are_ordered_to_break_their_permutation_symmetry() {
        let net = |width| RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width,
        };
        let nets = [net(8), net(8), net(9), net(8)];
        let mut lp = LpModel::new(Sense::Minimize);
        let path_vars: Vec<Vec<LpVar>> = (0..nets.len())
            .map(|net_index| {
                add_one_of_k_row(&mut lp, &format!("net_{net_index}"), 3, |path| {
                    format!("p_{net_index}_{path}")
                })
            })
            .collect();

        add_symmetry_rows(&mut lp, &nets, &path_vars);

        let ordered: Vec<&str> = lp
            .constraints
            .iter()
            .map(|row| row.name.as_str())
            .filter(|name| name.starts_with("symmetry_"))
            .collect();
        assert_eq!(
            ordered,
            ["symmetry_0_1", "symmetry_1_3"],
            "consecutive members of each identical group are ordered; the \
             odd width is its own group",
        );

        let row = lp
            .constraints
            .iter()
            .find(|row| row.name == "symmetry_0_1")
            .expect("symmetry row");
        assert_eq!(row.op, Comparison::Le);
        assert_eq!(row.rhs.to_bits(), 0.0_f64.to_bits());
        assert_eq!(
            row.expr.terms,
            vec![
                (1.0, path_vars[0][1]),
                (2.0, path_vars[0][2]),
                (-1.0, path_vars[1][1]),
                (-2.0, path_vars[1][2]),
            ],
            "rank zero is free on both sides, so the row orders the choices",
        );
    }

    /// Ordering identical nets removes permutations, not solutions: the router
    /// still spreads them however it likes, but always reports the one member
    /// ordering out of the `k!` that are otherwise indistinguishable.
    #[test]
    fn symmetry_ordering_canonicalizes_identical_nets_without_losing_answers() {
        let device = toy_device(100);
        let net = RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 10,
        };
        let nets = [net, net, net];
        let routes = route_with_cbc(&nets, &device);
        assert_eq!(routes.len(), 3);

        let candidates = candidates_for(&nets, &device).expect("candidates");
        let chosen: Vec<usize> = routes
            .iter()
            .enumerate()
            .map(|(net_index, route)| {
                candidates
                    .paths(net_index)
                    .iter()
                    .position(|path| path == route)
                    .expect("a routed path is one of the net's candidates")
            })
            .collect();
        assert!(
            chosen.windows(2).all(|pair| pair[0] <= pair[1]),
            "identical nets must report non-decreasing candidate indices: {chosen:?}",
        );
        assert!(
            chosen.iter().any(|index| *index > 0),
            "balancing still spreads them across boundaries: {routes:?}",
        );
    }

    #[test]
    fn slot_tag_is_the_short_form() {
        assert_eq!(slot_tag((1, 2)), "SLOT_X1Y2");
    }
}
