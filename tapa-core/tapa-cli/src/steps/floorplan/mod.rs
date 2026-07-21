//! `tapa floorplan` — coarse-grained floorplanning between `synth` and `pack`.
//!
//! Loads the synthesized work state, plans a placement with `tapa-floorplan`,
//! stores the resulting [`FloorplanResult`](tapa_ir::FloorplanResult) back into
//! `tapa.json`, and writes the pblock constraints to `<work_dir>/floorplan.xdc`.
//! Its presence in the state switches codegen and `pack` onto the floorplanned
//! path.

use clap::{Parser, ValueEnum};
use tapa_floorplan::{plan, render_xdc, PlanOptions};
use tapa_ir::PipelineScheme;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{json, work as work_io};

/// Name of the emitted pblock constraints file in the work directory.
pub const FLOORPLAN_XDC: &str = "floorplan.xdc";

/// CLI spelling of [`PipelineScheme`], with `snake_case` values matching the
/// contract's serde tags.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum PpScheme {
    Single,
    Double,
    SingleHDoubleV,
}

impl From<PpScheme> for PipelineScheme {
    fn from(scheme: PpScheme) -> Self {
        match scheme {
            PpScheme::Single => Self::Single,
            PpScheme::Double => Self::Double,
            PpScheme::SingleHDoubleV => Self::SingleHDoubleV,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "floorplan",
    about = "Coarse-grained floorplan the synthesized design."
)]
pub struct FloorplanArgs {
    /// Per-slot resource utilization target; raised on infeasibility.
    #[arg(long = "usage-limit", default_value_t = 0.7)]
    pub usage_limit: f64,

    /// How pipeline registers are distributed across a crossing's route.
    #[arg(long = "pp-scheme", value_enum, default_value_t = PpScheme::Double)]
    pub pp_scheme: PpScheme,

    /// ILP time limit, in seconds.
    #[arg(long = "max-seconds", default_value_t = 600)]
    pub max_seconds: u64,
}

pub fn run(args: &FloorplanArgs, ctx: &CliContext) -> Result<()> {
    let mut state = work_io::load(&ctx.work_dir)?;
    if !state.flow.synthed {
        return Err(CliError::Floorplan(
            "run `synth` before `floorplan`: the placement needs per-task areas".to_string(),
        ));
    }

    let options = PlanOptions {
        usage_limit: args.usage_limit,
        max_seconds: args.max_seconds,
        threads: 1,
        scheme: args.pp_scheme.into(),
    };
    let result = plan(&state, &options).map_err(|e| CliError::Floorplan(e.to_string()))?;
    let xdc = render_xdc(&result).map_err(|e| CliError::Floorplan(e.to_string()))?;

    json::write_bytes_atomic(&ctx.work_dir, FLOORPLAN_XDC, xdc.as_bytes())?;
    state.floorplan = Some(result);
    work_io::store(&ctx.work_dir, &state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_ir::{TaskGraph, WorkState};

    fn ctx_at(work_dir: &std::path::Path) -> CliContext {
        CliContext {
            work_dir: work_dir.to_path_buf(),
            temp_dir: None,
            clang_format_quota_in_bytes: 0,
            remote_config: None,
            verbose: 0,
            quiet: 0,
        }
    }

    /// A minimal synthesized vadd state: top `VecAdd` with A -> fifo -> B.
    fn synthed_vadd_state() -> WorkState {
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
                    "self_area": {"LUT": 100, "FF": 200}},
                "B": {"readable_name": "B", "code": "void B() {}", "level": "lower", "synth": "hls",
                    "ports": [{"cat": "istream", "name": "in", "type": "float", "width": 32}],
                    "self_area": {"LUT": 50, "FF": 60}}
            }
        }"#;
        let graph = TaskGraph::from_json(json).expect("parse vadd graph");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
        state.flow.synthed = true;
        state
    }

    #[test]
    fn floorplan_step_writes_xdc_and_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        work_io::store(dir.path(), &synthed_vadd_state()).expect("store state");
        let ctx = ctx_at(dir.path());
        let args = FloorplanArgs {
            usage_limit: 0.7,
            pp_scheme: PpScheme::Double,
            max_seconds: 60,
        };

        match run(&args, &ctx) {
            Ok(()) => {
                let xdc = fs_err::read_to_string(dir.path().join(FLOORPLAN_XDC)).expect("xdc");
                assert!(xdc.contains("create_pblock SLOT_X"), "xdc has pblocks");
                let reloaded = work_io::load(dir.path()).expect("reload");
                let floorplan = reloaded.floorplan.expect("floorplan stored");
                assert_eq!(floorplan.device, "u280");
                assert_eq!(floorplan.regions.len(), 3, "A_0, B_0, and the FIFO");
            }
            Err(CliError::Floorplan(msg)) if msg.contains("cbc") || msg.contains("solver") => {
                eprintln!("skipping floorplan_step: cbc not available ({msg})");
            }
            Err(other) => panic!("floorplan step failed: {other}"),
        }
    }

    #[test]
    fn floorplan_before_synth_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = synthed_vadd_state();
        state.flow.synthed = false;
        work_io::store(dir.path(), &state).expect("store");
        let err = run(
            &FloorplanArgs {
                usage_limit: 0.7,
                pp_scheme: PpScheme::Double,
                max_seconds: 60,
            },
            &ctx_at(dir.path()),
        )
        .expect_err("must require synth");
        assert!(matches!(err, CliError::Floorplan(_)));
    }
}
