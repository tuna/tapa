//! The post-placement routing MILP: choose one path per cross-slot net so the
//! worst normalized boundary utilization is minimized.
//!
//! Each net selects exactly one candidate path, and a continuous
//! `max_crossings` variable bounds
//! `sum(width * selected_path) / max(boundary_capacity, 1)` for every bounded
//! boundary. There is deliberately no route-length or bend-count objective.

use std::collections::{BTreeMap, BTreeSet};

use crate::device::model::{Device, WIRE_CAPACITY_INF};
use crate::route::paths::{enumerate_paths, Cell};
use crate::solver::{
    Comparison, LinExpr, LpModel, LpStatus, LpVar, Sense, SolveOpts, Solver, SolverError,
};

/// Maximum extra slot visits in a generated candidate path.
const MAX_DETOUR: usize = 2;
/// Fallback capacity when a device specifies no finite boundary.
const FALLBACK_WIRE_CAPACITY: u64 = 100_000;
/// Tolerance used only to validate a solver's binary readback.
const BINARY_TOLERANCE: f64 = 1e-6;
/// Tolerance used to compare recomputed and solver-reported utilization.
const UTILIZATION_TOLERANCE: f64 = 1e-7;

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
    /// The solver claimed success but did not return an integral, complete
    /// selected-path assignment.
    #[error("invalid routing solution: {0}")]
    InvalidSolution(String),
    /// The selected routes exceed at least one modeled boundary capacity.
    #[error("routing exceeds boundary capacity (maximum utilization {utilization:.6})")]
    CapacityExceeded {
        /// Maximum normalized utilization of the selected routes.
        utilization: f64,
    },
}

/// One inter-slot boundary and its finite crossing capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Boundary {
    a: Cell,
    b: Cell,
    capacity: u64,
}

/// Treat the infinite sentinel as an unspecified, unconstrained boundary.
fn finite_capacity(capacity: u64) -> Option<u64> {
    (capacity < WIRE_CAPACITY_INF).then_some(capacity)
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

/// Collect finite boundary limits in deterministic row-major order. If both
/// sides specify a limit, the north/east slot's south/west value wins.
fn finite_boundaries(device: &Device) -> Vec<Boundary> {
    let mut out = Vec::new();
    for y in 0..device.rows {
        for x in 0..device.cols {
            let Some(slot) = device.slot(x, y) else {
                continue;
            };
            if x + 1 < device.cols {
                let Some(right) = device.slot(x + 1, y) else {
                    continue;
                };
                let capacity = finite_capacity(right.wire_cap.west)
                    .or_else(|| finite_capacity(slot.wire_cap.east));
                if let Some(capacity) = capacity {
                    out.push(Boundary {
                        a: (x, y),
                        b: (x + 1, y),
                        capacity,
                    });
                }
            }
            if y + 1 < device.rows {
                let Some(up) = device.slot(x, y + 1) else {
                    continue;
                };
                let capacity = finite_capacity(up.wire_cap.south)
                    .or_else(|| finite_capacity(slot.wire_cap.north));
                if let Some(capacity) = capacity {
                    out.push(Boundary {
                        a: (x, y),
                        b: (x, y + 1),
                        capacity,
                    });
                }
            }
        }
    }
    out
}

/// Every adjacent grid boundary with the fallback capacity.
fn fallback_boundaries(device: &Device) -> Vec<Boundary> {
    let mut out = Vec::new();
    for y in 0..device.rows {
        for x in 0..device.cols {
            if x + 1 < device.cols && device.slot(x, y).is_some() && device.slot(x + 1, y).is_some()
            {
                out.push(Boundary {
                    a: (x, y),
                    b: (x + 1, y),
                    capacity: FALLBACK_WIRE_CAPACITY,
                });
            }
            if y + 1 < device.rows && device.slot(x, y).is_some() && device.slot(x, y + 1).is_some()
            {
                out.push(Boundary {
                    a: (x, y),
                    b: (x, y + 1),
                    capacity: FALLBACK_WIRE_CAPACITY,
                });
            }
        }
    }
    out
}

/// The boundary limits used by both the MILP and post-solve validation.
fn boundaries(device: &Device) -> Vec<Boundary> {
    let finite = finite_boundaries(device);
    if finite.is_empty() {
        fallback_boundaries(device)
    } else {
        finite
    }
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

/// Recompute the maximum normalized boundary utilization from selected paths.
#[allow(
    clippy::cast_precision_loss,
    reason = "routing widths and device capacities are small physical counts"
)]
fn maximum_utilization(nets: &[RouteNet], routes: &[Vec<Cell>], limits: &[Boundary]) -> f64 {
    limits.iter().fold(0.0_f64, |maximum, boundary| {
        let crossing: u64 = nets
            .iter()
            .zip(routes)
            .filter(|(_, path)| path_crosses(path, boundary))
            .map(|(net, _)| u64::from(net.width))
            .sum();
        maximum.max(crossing as f64 / boundary.capacity.max(1) as f64)
    })
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
    let max_crossings = lp.add_continuous("max_crossings", 0.0, f64::INFINITY);

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

    // Per boundary: max_crossings >= crossings / max(capacity, 1).
    for (boundary_index, boundary) in limits.iter().enumerate() {
        let mut terms: Vec<(f64, LpVar)> = Vec::new();
        for (net_index, paths) in candidates.iter().enumerate() {
            for (path_index, path) in paths.iter().enumerate() {
                if path_crosses(path, boundary) {
                    terms.push((
                        f64::from(nets[net_index].width),
                        path_vars[net_index][path_index],
                    ));
                }
            }
        }
        terms.push((-capacity_as_f64(boundary.capacity.max(1)), max_crossings));
        lp.add_constraint(
            format!("bound_{boundary_index}"),
            LinExpr::sum(terms),
            Comparison::Le,
            0.0,
        );
    }

    lp.set_objective(LinExpr::sum([(1.0, max_crossings)]));

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

    let routes = selected_routes(&candidates, &path_vars, &solution)?;
    let utilization = maximum_utilization(nets, &routes, &limits);
    let reported = solution
        .values
        .get(&max_crossings)
        .copied()
        .ok_or_else(|| RouteError::InvalidSolution("solver omitted max_crossings".to_string()))?;
    if !reported.is_finite() || reported < 0.0 {
        return Err(RouteError::InvalidSolution(format!(
            "max_crossings is invalid: {reported}"
        )));
    }
    if utilization > reported + UTILIZATION_TOLERANCE {
        return Err(RouteError::InvalidSolution(format!(
            "max_crossings {reported} does not bound recomputed utilization {utilization}"
        )));
    }
    if reported > 1.0 || utilization > 1.0 {
        return Err(RouteError::CapacityExceeded {
            utilization: reported.max(utilization),
        });
    }
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

    fn route_with_cbc(nets: &[RouteNet], device: &Device) -> Option<Vec<Vec<Cell>>> {
        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };
        match route_nets(nets, device, &CbcSolver::new(), &opts) {
            Ok(routes) => Some(routes),
            Err(RouteError::Solver(SolverError::Spawn { .. })) => {
                eprintln!("skipping: cbc not found");
                None
            }
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
        let Some(routes) = route_with_cbc(&nets, &device) else {
            return;
        };
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
        let Some(routes) = route_with_cbc(&nets, &device) else {
            return;
        };
        assert_eq!(
            routes[0],
            vec![(0, 0), (0, 1), (1, 1), (1, 0)],
            "the direct edge is 200% utilized while the detour is 10%",
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
        .expect_err("110% utilization is illegal");
        assert!(
            matches!(err, RouteError::CapacityExceeded { .. }),
            "got {err}"
        );
    }

    #[test]
    fn zero_capacity_uses_denominator_floor() {
        let device = toy_device(0);
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 0),
            width: 1,
        }];
        let preassigned = PreassignedRoutes::from([(0, vec![(0, 0), (1, 0)])]);
        let solver = FixedSolver {
            status: LpStatus::Optimal,
            objective: 1.0,
            values: vec![(LpVar(0), 1.0), (LpVar(1), 1.0)],
        };
        assert_eq!(
            route_nets_with_preassignments(
                &nets,
                &device,
                &solver,
                &SolveOpts::default(),
                &preassigned,
            )
            .expect("zero capacity is normalized by max(capacity, 1)"),
            vec![vec![(0, 0), (1, 0)]],
        );
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
