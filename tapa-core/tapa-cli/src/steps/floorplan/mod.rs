//! `tapa floorplan` — coarse-grained floorplanning between `synth` and `pack`.
//!
//! Loads the synthesized work state, plans a placement with `tapa-floorplan`,
//! stores the resulting [`FloorplanResult`](tapa_ir::FloorplanResult) back into
//! `tapa.json`, and writes the pblock constraints to `<work_dir>/floorplan.xdc`.
//! Its presence in the state switches codegen and `pack` onto the floorplanned
//! path.

use std::path::Path;

use clap::{Parser, ValueEnum};
use tapa_floorplan::{plan, render_xdc, PartitionStrategy, PlanOptions};
use tapa_ir::PipelineScheme;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{json, work as work_io};
use crate::steps::synth::rtl_codegen::{collect_hdl_inputs, generate_rtl_tree};

/// Name of the emitted pblock constraints file in the work directory.
pub const FLOORPLAN_XDC: &str = "floorplan.xdc";

/// Replace the active floorplan marker only after all dependent outputs are
/// ready. Removing an earlier marker first prevents `pack` from consuming RTL
/// or state left partially updated by a failed regeneration.
fn publish_xdc_after_update(
    work_dir: &Path,
    xdc: &str,
    update: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let active_path = work_dir.join(FLOORPLAN_XDC);
    match fs_err::remove_file(&active_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    update()?;
    json::write_bytes_atomic(work_dir, FLOORPLAN_XDC, xdc.as_bytes())
}

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

/// Placement subdivision schedule exposed by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum PartitionMode {
    /// Select flat or multilevel placement with the built-in heuristic.
    Auto,
    /// Place directly into atomic slots with one ILP.
    Flat,
    /// Place into rows first, then refine jointly into atomic slots.
    MultiLevel,
}

impl From<PartitionMode> for PartitionStrategy {
    fn from(mode: PartitionMode) -> Self {
        match mode {
            PartitionMode::Auto => Self::Auto,
            PartitionMode::Flat => Self::Flat,
            PartitionMode::MultiLevel => Self::MultiLevel,
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
    #[arg(
        long = "usage-limit",
        default_value_t = 0.7,
        value_parser = parse_usage_limit
    )]
    pub usage_limit: f64,

    /// Placement subdivision schedule.
    #[arg(
        long = "partition-strategy",
        value_enum,
        default_value_t = PartitionMode::Auto
    )]
    pub partition_strategy: PartitionMode,

    /// How pipeline registers are distributed across a crossing's route.
    #[arg(long = "pp-scheme", value_enum, default_value_t = PpScheme::Double)]
    pub pp_scheme: PpScheme,

    /// ILP time limit, in seconds.
    #[arg(
        long = "max-seconds",
        default_value_t = 600,
        value_parser = parse_positive_u64
    )]
    pub max_seconds: u64,
}

fn parse_usage_limit(value: &str) -> std::result::Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if parsed.is_finite() && parsed > 0.0 && parsed <= 1.0 {
        Ok(parsed)
    } else {
        Err(format!(
            "usage limit must be finite and in the range (0, 1], got {value}",
        ))
    }
}

fn parse_positive_u64(value: &str) -> std::result::Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("`{value}` is not a non-negative integer"))?;
    if parsed == 0 {
        Err("max seconds must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
}

pub fn run(args: &FloorplanArgs, ctx: &CliContext) -> Result<()> {
    let options = PlanOptions {
        usage_limit: args.usage_limit,
        max_seconds: args.max_seconds,
        threads: 1,
        partition_strategy: args.partition_strategy.into(),
        scheme: args.pp_scheme.into(),
    };
    options
        .validate()
        .map_err(|error| CliError::InvalidArg(error.to_string()))?;

    let mut state = work_io::load(&ctx.work_dir)?;
    if !state.flow.synthed {
        return Err(CliError::Floorplan(
            "run `synth` before `floorplan`: the placement needs per-task areas".to_string(),
        ));
    }
    let result = plan(&state, &options).map_err(|e| CliError::Floorplan(e.to_string()))?;
    let xdc = render_xdc(&result).map_err(|e| CliError::Floorplan(e.to_string()))?;

    publish_xdc_after_update(&ctx.work_dir, &xdc, || {
        // Regenerate the top RTL with floorplanned handshake pipelines. `plan`
        // planned on the flattened graph, so codegen must too — flattening the
        // original graph again yields the global FIFO names routes carry.
        let flat = tapa_ir::flatten(&state.graph)
            .map_err(|e| CliError::Floorplan(format!("flatten failed: {e}")))?;
        let hdl_inputs = collect_hdl_inputs(&ctx.work_dir, &flat)?;
        generate_rtl_tree(&ctx.work_dir, &flat, &hdl_inputs, Some(&result))?;

        state.floorplan = Some(result);
        work_io::store(&ctx.work_dir, &state)
    })
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
    fn floorplan_args_reject_unsafe_solver_limits() {
        for value in ["0", "-0.1", "1.01", "NaN", "inf"] {
            FloorplanArgs::try_parse_from(["floorplan", "--usage-limit", value])
                .expect_err("invalid usage limit");
        }
        FloorplanArgs::try_parse_from(["floorplan", "--max-seconds", "0"])
            .expect_err("zero-second solve");
    }

    #[test]
    fn floorplan_args_accept_partition_strategies() {
        for (value, expected) in [
            ("auto", PartitionMode::Auto),
            ("flat", PartitionMode::Flat),
            ("multi-level", PartitionMode::MultiLevel),
        ] {
            let args = FloorplanArgs::try_parse_from(["floorplan", "--partition-strategy", value])
                .expect("valid partition strategy");
            assert_eq!(args.partition_strategy, expected);
        }
    }

    #[test]
    fn direct_floorplan_run_validates_options_before_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = run(
            &FloorplanArgs {
                usage_limit: 0.0,
                partition_strategy: PartitionMode::Auto,
                pp_scheme: PpScheme::Double,
                max_seconds: 60,
            },
            &ctx_at(dir.path()),
        )
        .expect_err("invalid usage limit");
        assert!(matches!(err, CliError::InvalidArg(_)));
    }

    #[test]
    fn failed_floorplan_update_leaves_xdc_inactive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active_path = dir.path().join(FLOORPLAN_XDC);
        fs_err::write(&active_path, "old constraints").expect("old xdc");

        let err = publish_xdc_after_update(dir.path(), "new constraints", || {
            Err(CliError::Codegen(
                "injected regeneration failure".to_string(),
            ))
        })
        .expect_err("update must fail");

        assert!(
            matches!(err, CliError::Codegen(ref message) if message == "injected regeneration failure"),
            "the original failure must be preserved: {err}",
        );
        assert!(
            !active_path.exists(),
            "a failed update must not leave an XDC marker for pack",
        );
    }

    #[test]
    fn floorplan_step_writes_xdc_and_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        work_io::store(dir.path(), &synthed_vadd_state()).expect("store state");
        let ctx = ctx_at(dir.path());
        let args = FloorplanArgs {
            usage_limit: 0.7,
            partition_strategy: PartitionMode::Auto,
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

    /// Write a fake HLS Verilog module under `hls/<task>/verilog/<task>.v`.
    fn write_hls_module(work_dir: &std::path::Path, task: &str, src: &str) {
        let dir = work_dir.join("hls").join(task).join("verilog");
        fs_err::create_dir_all(&dir).expect("hls dir");
        fs_err::write(dir.join(format!("{task}.v")), src).expect("hls verilog");
    }

    #[test]
    fn floorplan_step_regenerates_head_body_tail_rtl() {
        // A and B are too large to share a u280 slot, so the stream between
        // them crosses a boundary and must use the floorplanned handshake
        // pipeline in the regenerated top RTL.
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
        let graph = TaskGraph::from_json(json).expect("parse");
        let mut state = WorkState::new(graph);
        state.flow.part_num = Some("xcu280-fsvh2892-2L-e".to_string());
        state.flow.synthed = true;

        let dir = tempfile::tempdir().expect("tempdir");
        work_io::store(dir.path(), &state).expect("store");
        write_hls_module(
            dir.path(),
            "VecAdd",
            "module VecAdd(\n input wire ap_clk,\n input wire ap_rst_n\n);\nendmodule",
        );
        write_hls_module(
            dir.path(),
            "A",
            "module A(\n input wire ap_clk,\n input wire ap_rst_n,\n \
             output wire [31:0] out_din,\n output wire out_write,\n input wire out_full_n\n);\nendmodule",
        );
        write_hls_module(
            dir.path(),
            "B",
            "module B(\n input wire ap_clk,\n input wire ap_rst_n,\n \
             input wire [31:0] in_dout,\n input wire in_empty_n,\n output wire in_read\n);\nendmodule",
        );

        let args = FloorplanArgs {
            usage_limit: 0.7,
            partition_strategy: PartitionMode::Auto,
            pp_scheme: PpScheme::Double,
            max_seconds: 60,
        };
        match run(&args, &ctx_at(dir.path())) {
            Ok(()) => {
                let top_v = fs_err::read_to_string(dir.path().join("rtl").join("VecAdd.v"))
                    .expect("top rtl");
                assert!(
                    top_v.contains("tapa_hs_pipeline"),
                    "the cross-slot stream must be regenerated as a Head/Body/Tail pipeline, got:\n{top_v}"
                );
                let xdc = fs_err::read_to_string(dir.path().join(FLOORPLAN_XDC)).expect("xdc");
                assert!(
                    xdc.contains("TAPA_HS_HEAD"),
                    "Head constraint missing:\n{xdc}"
                );
                assert!(
                    xdc.contains("TAPA_HS_BODY"),
                    "Body constraint missing:\n{xdc}"
                );
                assert!(
                    xdc.contains("TAPA_HS_TAIL"),
                    "Tail constraint missing:\n{xdc}"
                );
            }
            Err(CliError::Floorplan(msg)) if msg.contains("cbc") || msg.contains("solver") => {
                eprintln!(
                    "skipping floorplan_step_regenerates_head_body_tail_rtl: cbc not available ({msg})"
                );
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
                partition_strategy: PartitionMode::Auto,
                pp_scheme: PpScheme::Double,
                max_seconds: 60,
            },
            &ctx_at(dir.path()),
        )
        .expect_err("must require synth");
        assert!(matches!(err, CliError::Floorplan(_)));
    }
}
