//! The post-placement routing MILP: choose one path per cross-slot net without
//! exceeding any boundary's usable wire capacity.
//!
//! Each net selects exactly one candidate path. Every bounded boundary
//! contributes a *hard* `sum(width * selected_path) <= capacity` row, using
//! the same per-boundary [`effective_border_capacity`] budget the placement
//! cuts model, so a time-limited incumbent is always a physically legal
//! route. The objective minimizes the worst normalized utilization over the
//! positive-capacity boundaries; on a device with no bounded boundary at all
//! it minimizes total hop count instead. There is deliberately no bend-count
//! objective. A lexicographic second solve then pins the achieved objective
//! and minimizes the stable candidate-path index, so equal-quality routes
//! resolve deterministically across solver versions.

use std::collections::{BTreeMap, BTreeSet};

use crate::device::model::{effective_border_capacity, Device, WIRE_CAPACITY_INF};
use crate::route::paths::{enumerate_paths, Cell};
use crate::solver::{
    Comparison, LinExpr, LpModel, LpStatus, LpVar, Sense, SolveOpts, Solver, SolverError,
};

/// Maximum extra slot visits in a generated candidate path.
const MAX_DETOUR: usize = 2;
/// Tolerance used only to validate a solver's binary readback.
const BINARY_TOLERANCE: f64 = 1e-6;

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

/// Preassigned routes keyed by the net's stable input index.
///
/// A preassignment replaces that net's generated candidate set with exactly
/// one route.
type PreassignedRoutes = BTreeMap<usize, Vec<Cell>>;

/// Why routing failed to produce paths.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    /// The solver failed to run or parse.
    #[error(transparent)]
    Solver(#[from] SolverError),
    /// The solver proved that the routing MILP is infeasible.
    #[error("routing is infeasible")]
    Infeasible,
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
    /// A preassignment names a net that does not exist.
    #[error("preassigned route refers to net {net_index}, but only {net_count} nets exist")]
    UnknownPreassignment {
        /// Invalid net index.
        net_index: usize,
        /// Number of input nets.
        net_count: usize,
    },
    /// A generated or preassigned route is structurally invalid.
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

/// The usable budget of one boundary: the smaller facing declaration,
/// derated; `None` when the boundary is unconstrained on both sides.
fn boundary_capacity(lhs: u64, rhs: u64) -> Option<u64> {
    (lhs.min(rhs) < WIRE_CAPACITY_INF).then(|| effective_border_capacity(lhs, rhs))
}

/// Physical capacities are small integer counts and exactly representable in
/// the solver's numeric range.
#[allow(
    clippy::cast_precision_loss,
    reason = "wire capacities are small physical counts"
)]
fn capacity_as_f64(capacity: u64) -> f64 {
    capacity as f64
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
                        capacity: boundary_capacity(slot.wire_cap.east, right.wire_cap.west),
                    });
                }
            }
            if y + 1 < device.rows {
                if let Some(up) = device.slot(x, y + 1) {
                    out.push(Boundary {
                        a: (x, y),
                        b: (x, y + 1),
                        capacity: boundary_capacity(slot.wire_cap.north, up.wire_cap.south),
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

/// Build generated or singleton-preassigned candidates in net input order.
fn candidates_for(
    nets: &[RouteNet],
    device: &Device,
    preassigned: &PreassignedRoutes,
) -> Result<Vec<Vec<Vec<Cell>>>, RouteError> {
    if let Some((&net_index, _)) = preassigned.iter().find(|(index, _)| **index >= nets.len()) {
        return Err(RouteError::UnknownPreassignment {
            net_index,
            net_count: nets.len(),
        });
    }

    let mut candidates = Vec::with_capacity(nets.len());
    for (net_index, net) in nets.iter().enumerate() {
        let paths = if let Some(path) = preassigned.get(&net_index) {
            vec![path.clone()]
        } else {
            enumerate_paths(net.src, net.dst, device.cols, device.rows, MAX_DETOUR)
        };
        if paths.is_empty() {
            return Err(RouteError::Infeasible);
        }
        for path in &paths {
            validate_path(net_index, net, path, device)?;
        }
        candidates.push(paths);
    }
    Ok(candidates)
}

/// Read exactly one integral path choice for each net. Every path variable
/// must be present; there is no implicit-zero or path-zero fallback.
fn selected_routes(
    candidates: &[Vec<Vec<Cell>>],
    path_vars: &[Vec<LpVar>],
    solution: &crate::solver::LpSolution,
) -> Result<Vec<Vec<Cell>>, RouteError> {
    let mut routes = Vec::with_capacity(candidates.len());
    for (net_index, (paths, vars)) in candidates.iter().zip(path_vars).enumerate() {
        let mut chosen = None;
        for (path_index, &var) in vars.iter().enumerate() {
            let value = solution.values.get(&var).copied().ok_or_else(|| {
                RouteError::InvalidSolution(format!(
                    "solver omitted path variable for net {net_index}, candidate {path_index}"
                ))
            })?;
            if !value.is_finite()
                || ((value - 0.0).abs() > BINARY_TOLERANCE
                    && (value - 1.0).abs() > BINARY_TOLERANCE)
            {
                return Err(RouteError::InvalidSolution(format!(
                    "path variable for net {net_index}, candidate {path_index} is not binary: {value}"
                )));
            }
            if (value - 1.0).abs() <= BINARY_TOLERANCE && chosen.replace(path_index).is_some() {
                return Err(RouteError::InvalidSolution(format!(
                    "more than one path selected for net {net_index}"
                )));
            }
        }
        let Some(path_index) = chosen else {
            return Err(RouteError::InvalidSolution(format!(
                "no path selected for net {net_index}"
            )));
        };
        routes.push(paths[path_index].clone());
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

/// The total-hop objective over the path variables, used on devices with no
/// bounded boundary and to pin an unconstrained primary solve.
fn hop_objective(candidates: &[Vec<Vec<Cell>>], path_vars: &[Vec<LpVar>]) -> LinExpr {
    let mut terms = Vec::new();
    for (paths, vars) in candidates.iter().zip(path_vars) {
        for (path, &var) in paths.iter().zip(vars) {
            let hops = u32::try_from(path.len().saturating_sub(1)).expect("hop count fits u32");
            if hops > 0 {
                terms.push((f64::from(hops), var));
            }
        }
    }
    LinExpr::sum(terms)
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
    candidates: &[Vec<Vec<Cell>>],
    path_vars: &[Vec<LpVar>],
) -> Result<Option<crate::solver::LpSolution>, RouteError> {
    let pin = max_crossings.map_or_else(
        || hop_objective(candidates, path_vars),
        |max_crossings| LinExpr::sum([(1.0, max_crossings)]),
    );
    lp.add_constraint(
        "lexicographic_pin".to_string(),
        pin,
        Comparison::Le,
        primary.objective,
    );
    let mut terms = Vec::new();
    for vars in path_vars {
        for (index, &var) in vars.iter().enumerate() {
            let rank = u32::try_from(index).expect("path count fits u32");
            if rank > 0 {
                terms.push((f64::from(rank), var));
            }
        }
    }
    lp.set_objective(LinExpr::sum(terms));
    let refined = solver.solve(lp, opts)?;
    Ok(refined.is_found().then_some(refined))
}

/// Route every net, returning a chosen slot path (`src` first, `dst` last) per
/// net in input order.
pub fn route_nets(
    nets: &[RouteNet],
    device: &Device,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Vec<Vec<Cell>>, RouteError> {
    route_nets_with_preassignments(nets, device, solver, opts, &PreassignedRoutes::new())
}

/// Route every net with optional singleton preassigned routes.
///
/// Entries in `preassigned` are keyed by the corresponding position in
/// `nets`. A supplied path is validated and becomes that net's only MILP
/// candidate; omitted nets use the standard candidate generator.
fn route_nets_with_preassignments(
    nets: &[RouteNet],
    device: &Device,
    solver: &dyn Solver,
    opts: &SolveOpts,
    preassigned: &PreassignedRoutes,
) -> Result<Vec<Vec<Cell>>, RouteError> {
    validate_nets(nets, device)?;
    let candidates = candidates_for(nets, device, preassigned)?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let limits = boundaries(device);

    let mut lp = LpModel::new(Sense::Minimize);
    // Created first so it stays `x0` whenever the balancing objective applies.
    let max_crossings = limits
        .iter()
        .any(|boundary| boundary.capacity.is_some_and(|capacity| capacity > 0))
        .then(|| lp.add_continuous("max_crossings", 0.0, f64::INFINITY));

    // One binary per candidate path; exactly one per net.
    let mut path_vars: Vec<Vec<LpVar>> = Vec::with_capacity(nets.len());
    for (net_index, paths) in candidates.iter().enumerate() {
        let vars: Vec<LpVar> = (0..paths.len())
            .map(|path_index| lp.add_binary(format!("p_{net_index}_{path_index}")))
            .collect();
        lp.add_constraint(
            format!("net_{net_index}"),
            LinExpr::sum(vars.iter().map(|&var| (1.0, var))),
            Comparison::Eq,
            1.0,
        );
        path_vars.push(vars);
    }

    // Per bounded boundary: a hard capacity row, so every incumbent — even a
    // time-limited one — is a physically legal route; plus, when balancing,
    // the normalization row bounding `max_crossings`.
    for (boundary_index, boundary) in limits.iter().enumerate() {
        let Some(capacity) = boundary.capacity else {
            continue;
        };
        let mut crossings: Vec<(f64, LpVar)> = Vec::new();
        for (net_index, paths) in candidates.iter().enumerate() {
            for (path_index, path) in paths.iter().enumerate() {
                if path_crosses(path, boundary) {
                    crossings.push((
                        f64::from(nets[net_index].width),
                        path_vars[net_index][path_index],
                    ));
                }
            }
        }
        lp.add_constraint(
            format!("bound_{boundary_index}_capacity"),
            LinExpr::sum(crossings.iter().copied()),
            Comparison::Le,
            capacity_as_f64(capacity),
        );
        if let (Some(max_crossings), true) = (max_crossings, capacity > 0) {
            let mut terms = crossings;
            terms.push((-capacity_as_f64(capacity), max_crossings));
            lp.add_constraint(
                format!("bound_{boundary_index}"),
                LinExpr::sum(terms),
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
    route_opts.mip_gap_abs = Some(0.001);
    let solution = solver.solve(&lp, &route_opts)?;
    if !solution.is_found() {
        return Err(match solution.status {
            LpStatus::Infeasible => RouteError::Infeasible,
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
    use crate::device::model::{DirCaps, DirRegions, Slot, PP_DIST};
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
                    anchor: DirRegions::default(),
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
            pp_dist: PP_DIST,
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
    fn preassigned_route_is_the_only_candidate() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 10,
        }];
        let path = vec![(0, 0), (0, 1), (1, 1), (1, 0)];
        let preassigned = PreassignedRoutes::from([(0, path.clone())]);
        // x0 is max_crossings; x1 is the singleton path variable.
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 0.1,
            values: vec![(LpVar(0), 0.1), (LpVar(1), 1.0)],
        };
        assert_eq!(
            route_nets_with_preassignments(
                &nets,
                &device,
                &solver,
                &SolveOpts::default(),
                &preassigned,
            )
            .expect("valid preassignment"),
            vec![path],
        );
    }

    #[test]
    fn invalid_preassigned_path_is_rejected_before_solving() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 1),
            width: 1,
        }];
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 1)])]);
        let err = route_nets_with_preassignments(
            &nets,
            &device,
            &PanicSolver,
            &SolveOpts::default(),
            &preassigned,
        )
        .expect_err("diagonal hop is invalid");
        assert!(matches!(err, RouteError::InvalidPath { .. }), "got {err}");
    }

    #[test]
    fn missing_path_selection_is_not_defaulted_to_path_zero() {
        let device = toy_device(100);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: vec![(LpVar(0), 0.0), (LpVar(1), 0.0)],
        };
        let err = route_nets_with_preassignments(
            &nets,
            &device,
            &solver,
            &SolveOpts::default(),
            &preassigned,
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
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: vec![(LpVar(0), 0.0)],
        };
        let err = route_nets_with_preassignments(
            &nets,
            &device,
            &solver,
            &SolveOpts::default(),
            &preassigned,
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
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: vec![(LpVar(0), 0.0), (LpVar(1), 0.5)],
        };
        let err = route_nets_with_preassignments(
            &nets,
            &device,
            &solver,
            &SolveOpts::default(),
            &preassigned,
        )
        .expect_err("fractional path variable");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");
        assert!(err.to_string().contains("not binary"), "got {err}");
    }

    #[test]
    fn over_capacity_solution_is_rejected() {
        // The derated capacity of the preassigned hop is round(10 * 0.7) = 7.
        let device = toy_device(10);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 11,
        }];
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 1.1,
            values: vec![(LpVar(0), 1.1), (LpVar(1), 1.0)],
        };
        let err = route_nets_with_preassignments(
            &nets,
            &device,
            &solver,
            &SolveOpts::default(),
            &preassigned,
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
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = || FixedSolver {
            status: LpStatus::Optimal,
            objective: 0.0,
            values: vec![(LpVar(0), 0.0), (LpVar(1), 1.0)],
        };

        // A positive-width net may not cross: a zero-capacity boundary carries
        // no wires, derated or not.
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let err = route_nets_with_preassignments(
            &nets,
            &device,
            &solver(),
            &SolveOpts::default(),
            &preassigned,
        )
        .expect_err("wire over a zero-capacity boundary");
        assert!(matches!(err, RouteError::InvalidSolution(_)), "got {err}");

        // A zero-width net crosses nothing and is legal.
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 0,
        }];
        assert_eq!(
            route_nets_with_preassignments(
                &nets,
                &device,
                &solver(),
                &SolveOpts::default(),
                &preassigned,
            )
            .expect("zero-width net crosses nothing"),
            vec![vec![(0, 0), (1, 0)]],
        );
    }

    #[test]
    fn unconstrained_device_has_no_capacity_limit() {
        // Every boundary defaults to the unconstrained sentinel, so no
        // max_crossings variable exists: the first path variable is x0.
        let device = toy_unbounded_device();
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1_000_000,
        }];
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 1.0,
            values: vec![(LpVar(0), 1.0)],
        };
        assert_eq!(
            route_nets_with_preassignments(
                &nets,
                &device,
                &solver,
                &SolveOpts::default(),
                &preassigned,
            )
            .expect("an unconstrained device imposes no wire budget"),
            vec![vec![(0, 0), (1, 0)]],
        );
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
        assert!(matches!(err, RouteError::Infeasible), "got {err}");
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
    fn slot_tag_is_the_short_form() {
        assert_eq!(slot_tag((1, 2)), "SLOT_X1Y2");
    }
}
