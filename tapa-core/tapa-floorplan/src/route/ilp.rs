//! The routing ILP: choose one path per cross-slot net so the worst boundary
//! utilization is minimized.
//!
//! Ported from RapidStream's `route_design/router.py`. Each net picks exactly
//! one of its candidate ≤2-bend paths; a continuous `max_crossings` upper-bounds
//! every boundary's `Σ width·crossing / capacity`, and the objective minimizes
//! it. `max_crossings` is unbounded above, so the ILP always has a solution;
//! a value above 1 means that boundary is over budget (reported, not fatal).

use crate::device::model::{Device, WIRE_CAPACITY_INF};
use crate::route::paths::{enumerate_paths, Cell};
use crate::solver::{Comparison, LinExpr, LpModel, LpVar, Sense, SolveOpts, Solver, SolverError};

/// Maximum direction changes in a candidate path (RapidStream's ≤2 bends).
const MAX_BENDS: usize = 2;

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
    /// The routing ILP had no solution (a net with no candidate path).
    #[error("routing is infeasible")]
    Infeasible,
}

/// Capacities are small non-negative integers well inside f64's exact range.
#[allow(clippy::cast_precision_loss, reason = "wire capacities are < 2^32")]
fn to_f64(value: u64) -> f64 {
    value as f64
}

/// One inter-slot boundary and its finite crossing capacity.
struct Boundary {
    a: Cell,
    b: Cell,
    capacity: u64,
}

/// The finite-capacity boundaries between adjacent slots (north and east
/// directions cover every pair once).
fn boundaries(device: &Device) -> Vec<Boundary> {
    let mut out = Vec::new();
    for slot in &device.slots {
        if let Some(up) = device.slot(slot.x, slot.y + 1) {
            let capacity = slot.wire_cap.north.min(up.wire_cap.south);
            if capacity < WIRE_CAPACITY_INF {
                out.push(Boundary {
                    a: (slot.x, slot.y),
                    b: (slot.x, slot.y + 1),
                    capacity,
                });
            }
        }
        if let Some(right) = device.slot(slot.x + 1, slot.y) {
            let capacity = slot.wire_cap.east.min(right.wire_cap.west);
            if capacity < WIRE_CAPACITY_INF {
                out.push(Boundary {
                    a: (slot.x, slot.y),
                    b: (slot.x + 1, slot.y),
                    capacity,
                });
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

/// Route every net, returning a chosen slot path (`src` first, `dst` last) per
/// net in input order.
pub fn route_nets(
    nets: &[RouteNet],
    device: &Device,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Vec<Vec<Cell>>, RouteError> {
    let candidates: Vec<Vec<Vec<Cell>>> = nets
        .iter()
        .map(|net| enumerate_paths(net.src, net.dst, MAX_BENDS))
        .collect();
    if candidates.iter().any(Vec::is_empty) {
        return Err(RouteError::Infeasible);
    }
    let boundaries = boundaries(device);

    let mut lp = LpModel::new(Sense::Minimize);
    let max_crossings = lp.add_continuous("max_crossings", 0.0, f64::INFINITY);

    // One binary per candidate path; exactly one per net.
    let mut path_vars: Vec<Vec<LpVar>> = Vec::with_capacity(nets.len());
    for (ni, paths) in candidates.iter().enumerate() {
        let vars: Vec<LpVar> = (0..paths.len())
            .map(|pi| lp.add_binary(format!("p_{ni}_{pi}")))
            .collect();
        lp.add_constraint(
            format!("net_{ni}"),
            LinExpr::sum(vars.iter().map(|&v| (1.0, v))),
            Comparison::Eq,
            1.0,
        );
        path_vars.push(vars);
    }

    // Per boundary: Σ width·crossing·path ≤ capacity·max_crossings.
    for (bi, boundary) in boundaries.iter().enumerate() {
        let mut terms: Vec<(f64, LpVar)> = Vec::new();
        for (ni, paths) in candidates.iter().enumerate() {
            for (pi, path) in paths.iter().enumerate() {
                if path_crosses(path, boundary) {
                    terms.push((f64::from(nets[ni].width), path_vars[ni][pi]));
                }
            }
        }
        if terms.is_empty() {
            continue;
        }
        terms.push((-to_f64(boundary.capacity), max_crossings));
        lp.add_constraint(
            format!("bound_{bi}"),
            LinExpr::sum(terms),
            Comparison::Le,
            0.0,
        );
    }

    lp.set_objective(LinExpr::sum([(1.0, max_crossings)]));

    let solution = solver.solve(&lp, opts)?;
    if !solution.is_found() {
        return Err(RouteError::Infeasible);
    }

    let mut routes = Vec::with_capacity(nets.len());
    for (ni, paths) in candidates.iter().enumerate() {
        let chosen = (0..paths.len())
            .find(|&pi| solution.is_set(path_vars[ni][pi]))
            .unwrap_or(0);
        routes.push(paths[chosen].clone());
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
    use super::*;
    use crate::device::select::select_device;
    use crate::solver::CbcSolver;

    fn route(nets: &[RouteNet], device: &Device) -> Option<Vec<Vec<Cell>>> {
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
        let Some(routes) = route(&nets, &device) else {
            return;
        };
        assert_eq!(
            routes[0],
            vec![(0, 0), (0, 1), (0, 2)],
            "the only monotone path"
        );
    }

    #[test]
    fn diagonal_net_routes_a_valid_l_path() {
        let device = select_device("u280").expect("u280");
        let nets = [RouteNet {
            src: (0, 0),
            dst: (1, 1),
            width: 33,
        }];
        let Some(routes) = route(&nets, &device) else {
            return;
        };
        let route = &routes[0];
        assert_eq!(route.first(), Some(&(0, 0)));
        assert_eq!(route.last(), Some(&(1, 1)));
        assert_eq!(route.len(), 3, "one bend, three slots");
    }

    #[test]
    fn slot_tag_is_the_short_form() {
        assert_eq!(slot_tag((1, 2)), "SLOT_X1Y2");
    }
}
