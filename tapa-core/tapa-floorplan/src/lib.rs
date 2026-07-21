//! Coarse-grained floorplanning and latency-insensitive pipeline planning for
//! TAPA dataflow designs on multi-die AMD FPGAs.
//!
//! The planner assigns each flattened task/FIFO instance to a physical *slot*
//! on a rows×cols grid (rows = SLRs) by solving a wire-crossing-minimizing ILP
//! under per-slot resource and per-boundary wire-capacity constraints, then
//! plans register pipelining for every channel that crosses a slot boundary.
//! Its output is a [`tapa_ir::FloorplanResult`], the plain-data contract
//! codegen consumes.
//!
//! Module map (built out across phases):
//! - [`device`] — device model (`Area`/`Coor`/`Slot`/`Device`), embedded
//!   per-part JSON tables, and `part_num → Device` selection.
//! - `solver` — an `LpModel` + CPLEX-LP writer + `Solver` trait, with a first
//!   backend that spawns the external `cbc` binary.
//! - `graph`/`partition` — the `FloorGraph` and the floorplan ILP.
//! - `route`/`pipeline` — inter-slot routing and the pipeline plan.
//! - `xdc` — pblock/anchor XDC emission from a `FloorplanResult`.

pub mod device;
pub mod graph;
pub mod partition;
pub mod pipeline;
pub mod route;
pub mod solver;
pub mod xdc;

use std::time::Duration;

use tapa_ir::{FloorplanResult, PipelineScheme, WorkState};

use crate::device::select::{select_device, SelectError};
use crate::graph::{FloorGraph, GraphError};
use crate::partition::ilp::{floorplan_flat, IlpError, DEFAULT_USAGE_LIMIT};
use crate::pipeline::plan::{plan_crossings, PipelineError};
use crate::solver::{CbcSolver, SolveOpts};

/// Options controlling a [`plan`] run. Defaults match the CLI's defaults.
#[derive(Debug, Clone, Copy)]
pub struct PlanOptions {
    /// Base per-slot utilization target; raised on infeasibility.
    pub usage_limit: f64,
    /// ILP wall-clock limit, in seconds.
    pub max_seconds: u64,
    /// CBC worker threads. `1` keeps the solve deterministic.
    pub threads: u32,
    /// How pipeline registers are distributed across each crossing's route.
    pub scheme: PipelineScheme,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            usage_limit: DEFAULT_USAGE_LIMIT,
            max_seconds: 600,
            threads: 1,
            scheme: PipelineScheme::Double,
        }
    }
}

/// Why [`plan`] could not produce a floorplan.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// The work state has no resolved part number to select a device from.
    #[error("no part number in the work state; run `synth` first")]
    NoPartNum,
    /// The part number did not resolve to a device table.
    #[error(transparent)]
    Device(#[from] SelectError),
    /// Flattening the task graph failed.
    #[error(transparent)]
    Transform(#[from] tapa_ir::TransformError),
    /// Building the placement graph failed.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The floorplan ILP produced no placement.
    #[error(transparent)]
    Ilp(#[from] IlpError),
    /// The pipeline plan (routing) failed.
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
}

/// Plan a floorplan for a synthesized design.
///
/// Selects the device from the work state's part number, flattens the graph,
/// places every instance with the floorplan ILP, routes and pipelines every
/// cross-slot channel, and returns the complete [`FloorplanResult`] contract.
pub fn plan(state: &WorkState, options: &PlanOptions) -> Result<FloorplanResult, PlanError> {
    let part_num = state.flow.part_num.as_deref().ok_or(PlanError::NoPartNum)?;
    let device = select_device(part_num)?;
    let flat = tapa_ir::flatten(&state.graph)?;
    let graph = FloorGraph::build(&flat)?;

    let solver = CbcSolver::new();
    let opts = SolveOpts {
        time_limit: Some(Duration::from_secs(options.max_seconds)),
        threads: Some(options.threads),
        mip_gap: None,
    };
    let assignment = floorplan_flat(&graph, &device, options.usage_limit, &solver, &opts)?;
    let crossings = plan_crossings(
        &graph,
        &assignment.regions,
        &device,
        options.scheme,
        &solver,
        &opts,
    )?;

    Ok(FloorplanResult {
        device: device.key.clone(),
        grid: (device.cols, device.rows),
        regions: assignment.regions,
        crossings,
        slot_usage: assignment.slot_usage,
    })
}

/// Render a floorplan's pblock XDC, re-selecting the device from the result.
pub fn render_xdc(result: &FloorplanResult) -> Result<String, SelectError> {
    let device = select_device(&result.device)?;
    Ok(xdc::emit_xdc(result, &device))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::SolverError;

    #[test]
    fn plan_end_to_end_on_vadd() {
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
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        match plan(&state, &PlanOptions::default()) {
            Ok(result) => {
                assert_eq!(result.device, "u280");
                assert_eq!(result.grid, (2, 3));
                assert!(result.crossings.is_empty(), "the tiny design co-locates");
                assert_eq!(result.regions.len(), 3, "A_0, B_0, and the FIFO");
                // The rendered XDC references the assigned pblock.
                let xdc = render_xdc(&result).expect("render xdc");
                assert!(xdc.contains("create_pblock SLOT_X"));
            }
            Err(PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))) => {
                eprintln!("skipping plan_end_to_end_on_vadd: `cbc` not found");
            }
            Err(other) => panic!("plan failed: {other}"),
        }
    }

    #[test]
    fn plan_pipelines_a_forced_crossing() {
        use tapa_ir::CrossingKind;
        // Two tasks whose LUTs together exceed one u280 slot's derated
        // capacity (2 * 120000 > 220800 * 0.7) must split into adjacent slots,
        // so their connecting stream crosses one boundary.
        let json = r#"{
            "cflags": [], "top": "VecAdd", "target": "xilinx-vitis",
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
                    "self_area": {"LUT": 120000}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 120000}}
            }
        }"#;
        let graph = tapa_ir::TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());

        match plan(&state, &PlanOptions::default()) {
            Ok(result) => {
                assert!(
                    result
                        .regions
                        .values()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        >= 2,
                    "the two large tasks cannot share a slot",
                );
                assert_eq!(result.crossings.len(), 1, "the one stream crosses");
                let crossing = &result.crossings[0];
                assert_eq!(crossing.kind, CrossingKind::Stream);
                assert_eq!(crossing.link, "fifo_VecAdd");
                assert_eq!(crossing.route.len(), 2, "adjacent slots, one hop");
                assert_eq!(crossing.level, 2, "double scheme, one hop -> 2 stages");
            }
            Err(PlanError::Ilp(IlpError::Solver(SolverError::Spawn { .. }))) => {
                eprintln!("skipping plan_pipelines_a_forced_crossing: `cbc` not found");
            }
            Err(other) => panic!("plan failed: {other}"),
        }
    }

    #[test]
    fn plan_without_part_number_errors() {
        let graph = tapa_ir::TaskGraph::from_json(
            r#"{"cflags": [], "top": "T", "target": "xilinx-hls",
                "tasks": {"T": {"readable_name": "T", "code": "void T(){}", "level": "upper",
                    "synth": "hls", "ports": [], "tasks": {}, "fifos": {}}}}"#,
        )
        .expect("parse");
        let state = WorkState::new(graph);
        assert!(matches!(
            plan(&state, &PlanOptions::default()),
            Err(PlanError::NoPartNum)
        ));
    }
}
