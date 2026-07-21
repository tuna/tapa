//! The floorplan ILP: assign every [`FloorGraph`] vertex to a device slot so
//! that weighted wire crossings are minimized under per-slot resource and
//! per-cut wire-capacity limits.
//!
//! Ported from RapidStream's `autobridge/partition/floorplan.py`. The
//! edge-based formulation uses vertex-assignment binaries `x[v][s]` and
//! directed edge-routing binaries `y[e][a][b]`, coupled so a routed edge's
//! endpoints match their vertices' placement.

use std::collections::BTreeMap;

use tapa_ir::Area;

use crate::device::model::{
    add_area, penalized_distance, Coor, Device, Resource, VERTICAL_DIST_PENALTY,
};
use crate::graph::FloorGraph;
use crate::partition::cut::{find_cuts, Cut};
use crate::solver::{Comparison, LinExpr, LpModel, LpVar, Sense, SolveOpts, Solver, SolverError};

/// The default base utilization target and the retry envelope, matching
/// RapidStream (`schedule.py:15`, `tree.py:119-120`).
pub const DEFAULT_USAGE_LIMIT: f64 = 0.7;
const USAGE_LIMIT_STEP: f64 = 0.02;
const MAX_USAGE_LIMIT: f64 = 0.95;

/// A completed placement: where each vertex landed, and the resource usage of
/// each occupied region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// Vertex name → region tag.
    pub regions: BTreeMap<String, String>,
    /// Region tag → summed resource usage.
    pub slot_usage: BTreeMap<String, Area>,
}

/// Why the floorplan ILP produced no placement.
#[derive(Debug, thiserror::Error)]
pub enum IlpError {
    /// The solver failed to run or parse.
    #[error(transparent)]
    Solver(#[from] SolverError),
    /// No feasible placement even at the maximum usage limit.
    #[error("floorplan is infeasible up to usage limit {0}")]
    Infeasible(f64),
}

/// Coefficients are small non-negative integers well inside f64's exact range.
#[allow(
    clippy::cast_precision_loss,
    reason = "areas, widths, distances, and capacities are < 2^32"
)]
fn to_f64(value: u64) -> f64 {
    value as f64
}

/// Plan a floorplan over the whole device in one flat ILP, raising the usage
/// limit by 0.02 (up to 0.95) whenever `base_usage_limit` is infeasible.
pub fn floorplan_flat(
    graph: &FloorGraph,
    device: &Device,
    base_usage_limit: f64,
    solver: &dyn Solver,
    opts: &SolveOpts,
) -> Result<Assignment, IlpError> {
    let slots: Vec<Coor> = device.slots.iter().map(|s| Coor::slot(s.x, s.y)).collect();
    let cuts = find_cuts(device);

    let mut usage_limit = base_usage_limit;
    loop {
        let model = FloorplanModel::build(graph, device, &slots, &cuts, usage_limit);
        let solution = solver.solve(&model.lp, opts)?;
        if solution.is_found() {
            return Ok(model.read_back(graph, &slots, &solution));
        }
        usage_limit += USAGE_LIMIT_STEP;
        if usage_limit > MAX_USAGE_LIMIT {
            return Err(IlpError::Infeasible(MAX_USAGE_LIMIT));
        }
    }
}

/// The built LP plus the `x[v][s]` handles needed to read the placement back.
struct FloorplanModel {
    lp: LpModel,
    /// `x[vertex][slot]` assignment binaries.
    x: Vec<Vec<LpVar>>,
}

impl FloorplanModel {
    fn build(
        graph: &FloorGraph,
        device: &Device,
        slots: &[Coor],
        cuts: &[Cut],
        usage_limit: f64,
    ) -> Self {
        let mut lp = LpModel::new(Sense::Minimize);
        let x = add_assignment_vars(&mut lp, graph, slots);
        let y = add_routing_vars(&mut lp, graph.edges().len(), slots.len());
        add_coupling(&mut lp, graph, &x, &y, slots.len());
        add_pinning(&mut lp, graph, device, &x);
        add_capacity(&mut lp, graph, device, slots, usage_limit, &x);
        add_cuts(&mut lp, graph, slots, cuts, &y);
        add_objective(&mut lp, graph, device, slots, &y);
        Self { lp, x }
    }

    /// Read the placement out of a solved model.
    fn read_back(
        &self,
        graph: &FloorGraph,
        slots: &[Coor],
        solution: &crate::solver::LpSolution,
    ) -> Assignment {
        let mut regions = BTreeMap::new();
        let mut slot_usage: BTreeMap<String, Area> = BTreeMap::new();

        for (vi, vertex) in graph.vertices().iter().enumerate() {
            let slot_index = (0..slots.len())
                .find(|&si| solution.is_set(self.x[vi][si]))
                .unwrap_or(0);
            let region = slots[slot_index].region_name();
            regions.insert(vertex.name.clone(), region.clone());
            let entry = slot_usage.entry(region).or_default();
            *entry = add_area(*entry, vertex.area);
        }

        Assignment {
            regions,
            slot_usage,
        }
    }
}

/// `x[v][s]` binaries and the "each vertex in exactly one slot" constraints.
fn add_assignment_vars(lp: &mut LpModel, graph: &FloorGraph, slots: &[Coor]) -> Vec<Vec<LpVar>> {
    let mut x: Vec<Vec<LpVar>> = Vec::with_capacity(graph.vertices().len());
    for vertex in graph.vertices() {
        let row: Vec<LpVar> = slots
            .iter()
            .map(|slot| lp.add_binary(format!("x_{}_{}", vertex.name, slot.region_name())))
            .collect();
        x.push(row);
    }
    for (vi, row) in x.iter().enumerate() {
        lp.add_constraint(
            format!("assign_{vi}"),
            LinExpr::sum(row.iter().map(|&var| (1.0, var))),
            Comparison::Eq,
            1.0,
        );
    }
    x
}

/// Pin every M-AXI-bearing vertex to a memory-bearing slot.
///
/// On u280 the HBM controllers sit in SLR0, so an mmap reader placed elsewhere
/// pays a wide, unregistered die crossing on its M-AXI bundle — exactly what
/// caps Fmax on HBM designs. We forbid such a vertex from landing on any slot
/// the device table does not tag `HBM` by fixing its assignment binaries there
/// to zero. Devices with no `HBM`-tagged slot (or graphs with no mmap vertex)
/// leave the model untouched.
fn add_pinning(lp: &mut LpModel, graph: &FloorGraph, device: &Device, x: &[Vec<LpVar>]) {
    let is_hbm_slot: Vec<bool> = device
        .slots
        .iter()
        .map(|slot| slot.tags.iter().any(|tag| tag == "HBM"))
        .collect();
    if !is_hbm_slot.iter().any(|&hbm| hbm) {
        return; // device exposes no HBM row — nothing to pin against
    }
    for (vi, vertex) in graph.vertices().iter().enumerate() {
        if !vertex.needs_hbm {
            continue;
        }
        for (si, &hbm) in is_hbm_slot.iter().enumerate() {
            if !hbm {
                lp.add_constraint(
                    format!("pin_{vi}_{si}"),
                    LinExpr::sum([(1.0, x[vi][si])]),
                    Comparison::Eq,
                    0.0,
                );
            }
        }
    }
}

/// `y[e][a][b]` directed routing binaries and the "each edge one route"
/// constraints.
fn add_routing_vars(lp: &mut LpModel, edge_count: usize, s_count: usize) -> Vec<Vec<Vec<LpVar>>> {
    let mut y: Vec<Vec<Vec<LpVar>>> = Vec::with_capacity(edge_count);
    for ei in 0..edge_count {
        let plane: Vec<Vec<LpVar>> = (0..s_count)
            .map(|a| {
                (0..s_count)
                    .map(|b| lp.add_binary(format!("y_{ei}_{a}_{b}")))
                    .collect()
            })
            .collect();
        y.push(plane);
    }
    for (ei, plane) in y.iter().enumerate() {
        lp.add_constraint(
            format!("route_{ei}"),
            LinExpr::sum(plane.iter().flatten().map(|&var| (1.0, var))),
            Comparison::Eq,
            1.0,
        );
    }
    y
}

/// Couple each edge's route to its endpoints' placement: the route leaves
/// src(e)'s slot and arrives at dst(e)'s slot.
fn add_coupling(
    lp: &mut LpModel,
    graph: &FloorGraph,
    x: &[Vec<LpVar>],
    y: &[Vec<Vec<LpVar>>],
    s_count: usize,
) {
    for (ei, edge) in graph.edges().iter().enumerate() {
        for s in 0..s_count {
            let mut src_terms: Vec<(f64, LpVar)> =
                (0..s_count).map(|b| (1.0, y[ei][s][b])).collect();
            src_terms.push((-1.0, x[edge.src][s]));
            lp.add_constraint(
                format!("csrc_{ei}_{s}"),
                LinExpr::sum(src_terms),
                Comparison::Eq,
                0.0,
            );

            let mut dst_terms: Vec<(f64, LpVar)> =
                (0..s_count).map(|a| (1.0, y[ei][a][s])).collect();
            dst_terms.push((-1.0, x[edge.dst][s]));
            lp.add_constraint(
                format!("cdst_{ei}_{s}"),
                LinExpr::sum(dst_terms),
                Comparison::Eq,
                0.0,
            );
        }
    }
}

/// Per-slot, per-resource capacity: `Σ_v area(v)·x[v][s] ≤ cap·usage_limit`.
fn add_capacity(
    lp: &mut LpModel,
    graph: &FloorGraph,
    device: &Device,
    slots: &[Coor],
    usage_limit: f64,
    x: &[Vec<LpVar>],
) {
    for (si, slot_coor) in slots.iter().enumerate() {
        let slot = device
            .slot(slot_coor.dl_x, slot_coor.dl_y)
            .expect("candidate slot exists");
        for resource in Resource::ALL {
            let capacity = to_f64(resource.amount(&slot.area)) * usage_limit;
            let terms = graph
                .vertices()
                .iter()
                .enumerate()
                .map(|(vi, vertex)| (to_f64(resource.amount(&vertex.area)), x[vi][si]));
            lp.add_constraint(
                format!("cap_{}_{}", slot_coor.region_name(), resource.name()),
                LinExpr::sum(terms),
                Comparison::Le,
                capacity.floor(),
            );
        }
    }
}

/// Per-cut wire crossing: `Σ_e width(e)·(crossings either direction) ≤ cap`.
fn add_cuts(
    lp: &mut LpModel,
    graph: &FloorGraph,
    slots: &[Coor],
    cuts: &[Cut],
    y: &[Vec<Vec<LpVar>>],
) {
    let index_of = |coor: &Coor| slots.iter().position(|s| s == coor);
    for cut in cuts {
        let lhs: Vec<usize> = cut.lhs.iter().filter_map(index_of).collect();
        let rhs: Vec<usize> = cut.rhs.iter().filter_map(index_of).collect();
        let mut terms: Vec<(f64, LpVar)> = Vec::new();
        for (ei, edge) in graph.edges().iter().enumerate() {
            let width = f64::from(edge.width);
            for &a in &lhs {
                for &b in &rhs {
                    terms.push((width, y[ei][a][b]));
                    terms.push((width, y[ei][b][a]));
                }
            }
        }
        lp.add_constraint(
            format!("cut_{}", cut.name),
            LinExpr::sum(terms),
            Comparison::Le,
            to_f64(cut.capacity),
        );
    }
}

/// Objective: `min Σ_e Σ_{a,b} width(e)·distance(a,b)·y[e][a][b] + 1`.
fn add_objective(
    lp: &mut LpModel,
    graph: &FloorGraph,
    device: &Device,
    slots: &[Coor],
    y: &[Vec<Vec<LpVar>>],
) {
    let centroids: Vec<(i64, i64)> = slots.iter().map(|s| centroid(device, s)).collect();
    let mut objective: Vec<(f64, LpVar)> = Vec::new();
    for (ei, edge) in graph.edges().iter().enumerate() {
        let width = f64::from(edge.width);
        for (a, &centroid_a) in centroids.iter().enumerate() {
            for (b, &centroid_b) in centroids.iter().enumerate() {
                let dist = penalized_distance(centroid_a, centroid_b, VERTICAL_DIST_PENALTY);
                if dist != 0 {
                    objective.push((width * to_f64(dist_abs(dist)), y[ei][a][b]));
                }
            }
        }
    }
    lp.set_objective(LinExpr::sum(objective).plus_constant(1.0));
}

/// A slot's centroid, from the device table.
fn centroid(device: &Device, slot: &Coor) -> (i64, i64) {
    let s = device.slot(slot.dl_x, slot.dl_y).expect("slot exists");
    (s.centroid_x, s.centroid_y)
}

/// `penalized_distance` is non-negative (both terms are `abs`), so this is a
/// total function into `u64` for the objective coefficient.
fn dist_abs(distance: i64) -> u64 {
    u64::try_from(distance).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::select::select_device;
    use crate::solver::CbcSolver;

    /// Build a flat placement, skipping the test when `cbc` is unavailable.
    fn plan(graph: &FloorGraph, device: &Device) -> Option<Assignment> {
        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };
        match floorplan_flat(graph, device, DEFAULT_USAGE_LIMIT, &CbcSolver::new(), &opts) {
            Ok(assignment) => Some(assignment),
            Err(IlpError::Solver(SolverError::Spawn { .. })) => {
                eprintln!("skipping: cbc not found");
                None
            }
            Err(other) => panic!("floorplan failed: {other}"),
        }
    }

    /// A two-task design connected by one FIFO.
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

    #[test]
    fn vadd_places_every_instance_in_one_region() {
        let device = select_device("u280").expect("u280");
        let graph = vadd_floor_graph();
        let Some(assignment) = plan(&graph, &device) else {
            return;
        };

        for vertex in graph.vertices() {
            let region = assignment.regions.get(&vertex.name).expect("region");
            assert!(region.starts_with("SLOT_X"), "{region} is a slot tag");
        }
        // Minimizing wire crossing co-locates the whole tiny design.
        let regions: std::collections::BTreeSet<_> = assignment.regions.values().collect();
        assert_eq!(regions.len(), 1, "the whole design fits and co-locates");
    }

    #[test]
    fn oversized_design_is_infeasible() {
        // A task larger than any single slot's derated LUT capacity cannot be
        // placed at any usage limit.
        let json = r#"{
            "cflags": [], "top": "Big", "target": "xilinx-hls",
            "tasks": {
                "Big": {"readable_name": "Big", "code": "void Big() {}", "level": "upper", "synth": "hls",
                    "ports": [], "tasks": {"H": [{"args": {}, "step": 0}]}, "fifos": {}},
                "H": {"readable_name": "H", "code": "void H() {}", "level": "lower", "synth": "hls",
                    "ports": [], "self_area": {"LUT": 999999999}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let flat = tapa_ir::flatten(&graph).expect("flatten");
        let fg = FloorGraph::build(&flat).expect("floor graph");
        let device = select_device("u280").expect("u280");
        let opts = SolveOpts {
            threads: Some(1),
            ..SolveOpts::default()
        };
        match floorplan_flat(&fg, &device, DEFAULT_USAGE_LIMIT, &CbcSolver::new(), &opts) {
            Ok(_) => panic!("an oversized task must not place"),
            Err(IlpError::Infeasible(_)) => {}
            Err(IlpError::Solver(SolverError::Spawn { .. })) => eprintln!("skipping: no cbc"),
            Err(other) => panic!("unexpected: {other}"),
        }
    }

    #[test]
    fn mmap_readers_are_pinned_to_hbm_slots() {
        let device = select_device("u280").expect("u280");
        let hbm_regions: std::collections::BTreeSet<String> = device
            .slots
            .iter()
            .filter(|slot| slot.tags.iter().any(|tag| tag == "HBM"))
            .map(|slot| slot.coor().region_name())
            .collect();
        assert!(!hbm_regions.is_empty(), "u280 tags its SLR0 slots HBM");

        // Two async_mmap readers each stream to a large compute task placed far
        // from SLR0 by crossing minimization; pinning must still hold the
        // readers in an HBM slot. `R*` bind a top-level M-AXI; `C*` do not.
        let json = r#"{
            "cflags": [], "top": "Top", "target": "xilinx-hls",
            "tasks": {
                "Top": {
                    "readable_name": "Top", "code": "void Top() {}", "level": "upper", "synth": "hls",
                    "ports": [],
                    "tasks": {
                        "R": [
                            {"args": {"m": {"arg": "mem0", "cat": "async_mmap"}, "o": {"arg": "f0", "cat": "ostream"}}, "step": 0},
                            {"args": {"m": {"arg": "mem1", "cat": "async_mmap"}, "o": {"arg": "f1", "cat": "ostream"}}, "step": 0}
                        ],
                        "C": [
                            {"args": {"i": {"arg": "f0", "cat": "istream"}}, "step": 0},
                            {"args": {"i": {"arg": "f1", "cat": "istream"}}, "step": 0}
                        ]
                    },
                    "fifos": {
                        "f0": {"depth": 2, "consumed_by": ["C", 0], "produced_by": ["R", 0]},
                        "f1": {"depth": 2, "consumed_by": ["C", 1], "produced_by": ["R", 1]}
                    }
                },
                "R": {"readable_name": "R", "code": "void R() {}", "level": "lower", "synth": "hls",
                    "ports": [
                        {"cat": "async_mmap", "name": "m", "type": "ap_uint<512>*", "width": 512},
                        {"cat": "ostream", "name": "o", "type": "ap_uint<512>", "width": 512}
                    ],
                    "self_area": {"LUT": 400}},
                "C": {"readable_name": "C", "code": "void C() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "i", "type": "ap_uint<512>", "width": 512}],
                    "self_area": {"LUT": 120000}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let flat = tapa_ir::flatten(&graph).expect("flatten");
        let fg = FloorGraph::build(&flat).expect("floor graph");

        // The floor graph must flag both readers and neither consumer.
        for vertex in fg.vertices() {
            let expect_hbm = vertex.name.starts_with('R');
            assert_eq!(
                vertex.needs_hbm, expect_hbm,
                "{} needs_hbm should be {expect_hbm}",
                vertex.name
            );
        }

        let Some(assignment) = plan(&fg, &device) else {
            return;
        };
        for (name, region) in &assignment.regions {
            if name.starts_with('R') {
                assert!(
                    hbm_regions.contains(region),
                    "mmap reader {name} must be pinned to an HBM slot, landed in {region}",
                );
            }
        }
    }
}
