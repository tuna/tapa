//! Tests for `tapa compile` / `generate-floorplan` composite command
//! wiring. Split out of `mod.rs` to keep the production module under
//! the AC-10 450-LOC soft budget.

#![allow(
    clippy::similar_names,
    reason = "args/argv pair matches the production naming"
)]

use std::sync::Mutex;

use tapa_xilinx::{ToolInvocation, ToolOutput, ToolRunner};

use super::*;

#[test]
fn compile_args_round_trip_via_clap() {
    let args = CompileArgs::try_parse_from([
        "compile",
        "--input",
        "vadd.cpp",
        "--top",
        "VecAdd",
        "--platform",
        "xilinx_u250",
        "--output",
        "vadd.xo",
    ])
    .expect("compile args parse");
    assert_eq!(args.analyze.input_files.len(), 1);
    assert_eq!(args.analyze.top, "VecAdd");
    assert_eq!(args.synth.platform.as_deref(), Some("xilinx_u250"));
    assert_eq!(
        args.pack.output.as_ref().map(|p| p.display().to_string()),
        Some("vadd.xo".to_string()),
    );
}

#[test]
fn generate_floorplan_args_parse() {
    let args = GenerateFloorplanArgs::try_parse_from([
        "generate-floorplan",
        "--input",
        "a.cpp",
        "--top",
        "T",
        "--platform",
        "xilinx_u250",
        "--device-config",
        "dev.json",
        "--floorplan-config",
        "fp.json",
    ])
    .expect("parse");
    assert_eq!(args.top, "T");
    let synth_args = args.synth_args();
    // Composites force flatten_hierarchy / enable_synth_util / gen_ab_graph
    // per Python `kwargs[...] = True` assignments in `meta.py`.
    assert!(synth_args.gen_ab_graph);
    assert!(synth_args.enable_synth_util);
    assert!(args.analyze_args().flatten_hierarchy);
}

// --- Regression fixture for `generate-floorplan` + --enable-synth-util ---
//
// Before the post-synth-util port landed, `synth_args().enable_synth_util = true`
// aborted the composite with `CliError::InvalidArg("... requires Vivado on PATH ...")`.
// These helpers stand up a minimal analyze-output fixture + a combined
// HLS/Vivado mock runner so the synth step runs end-to-end with the
// post-synth-util branch live.

/// Dispatch by `inv.program`: `vitis_hls` stages the HLS
/// output tree, `vivado` writes a canned hierarchical
/// utilization `.rpt` to the `tclargs[1]` path we recorded.
struct HlsAndVivadoStub {
    hls_q: Mutex<Vec<String>>,
}

impl HlsAndVivadoStub {
    fn stage_hls_output(&self, inv: &ToolInvocation) {
        let cwd = inv.cwd.clone().expect("HLS sets cwd");
        // Route by the kernel source the runner embedded in env, not
        // by FIFO queue order: with parallel HLS dispatch the two
        // `run_hls` invocations race, and queue-order pop would
        // misroute "Add" output into "VecAdd"'s stage tree (and
        // vice versa), causing the csynth.xml harvester to miss.
        // `TAPA_KERNEL_PATH_0` is the canonical top-name carrier.
        let top = inv.env.get("TAPA_KERNEL_PATH_0").map_or_else(
            || self.hls_q.lock().unwrap().remove(0),
            |p| {
                std::path::Path::new(p)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("UNKNOWN")
                    .to_string()
            },
        );
        // Mirror the queue pop so the existing fixture list stays
        // non-empty — some callers still rely on the side effect.
        let _ = self.hls_q.lock().unwrap().pop();
        let syn = cwd.join("project").join(&top).join("syn");
        std::fs::create_dir_all(syn.join("report")).unwrap();
        std::fs::create_dir_all(syn.join("verilog")).unwrap();
        std::fs::write(
            syn.join("report").join(format!("{top}_csynth.xml")),
            br#"<?xml version="1.0"?>
<profile>
  <UserAssignments>
    <TopModelName>X</TopModelName>
    <Part>xcvu37p</Part>
    <TargetClockPeriod>3.33</TargetClockPeriod>
  </UserAssignments>
  <PerformanceEstimates>
    <SummaryOfTimingAnalysis>
      <EstimatedClockPeriod>1.0</EstimatedClockPeriod>
    </SummaryOfTimingAnalysis>
  </PerformanceEstimates>
</profile>"#,
        )
        .unwrap();
        std::fs::write(
            syn.join("verilog").join(format!("{top}.v")),
            format!(
                "module {top}(\n  input wire ap_clk,\n  \
                 input wire ap_rst_n,\n  input wire ap_start,\n  \
                 output wire ap_done,\n  output wire ap_idle,\n  \
                 output wire ap_ready\n);\nendmodule\n"
            ),
        )
        .unwrap();
    }

    fn stage_vivado_rpt(inv: &ToolInvocation) {
        let tclargs_pos = inv
            .args
            .iter()
            .position(|a| a == "-tclargs")
            .expect("vivado must receive -tclargs");
        let rpt_path = &inv.args[tclargs_pos + 2];
        let rpt = "Hierarchical Utilization Report\n\
            | Device : xcu250\n\
            +---+----+----+---+----+------+------+\n\
            | Instance | Total LUTs | FFs | DSP Blocks | URAM | RAMB36 | RAMB18 |\n\
            +---+----+----+---+----+------+------+\n\
            | Add | 11 | 22 | 3 | 4 | 5 | 6 |\n\
            +---+----+----+---+----+------+------+\n";
        if let Some(parent) = std::path::Path::new(rpt_path).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(rpt_path, rpt).unwrap();
    }
}

impl ToolRunner for HlsAndVivadoStub {
    fn run(&self, inv: &ToolInvocation) -> tapa_xilinx::Result<ToolOutput> {
        match inv.program.as_str() {
            "vitis_hls" => {
                self.stage_hls_output(inv);
                Ok(ToolOutput::default())
            }
            "vivado" => {
                Self::stage_vivado_rpt(inv);
                Ok(ToolOutput::default())
            }
            other => panic!("unexpected program: {other}"),
        }
    }
}

fn seed_vadd_work_dir(work: &std::path::Path) {
    use indexmap::IndexMap;
    use serde_json::json;
    use tapa_task_graph::{Design, TaskTopology};
    use crate::state::{design as design_io, settings as settings_io};

    let mut tasks = IndexMap::new();
    tasks.insert(
        "Add".to_string(),
        TaskTopology {
            name: "Add".to_string(),
            level: "lower".to_string(),
            code: "void Add() {}\n".to_string(),
            ports: Vec::new(),
            tasks: IndexMap::new(),
            fifos: IndexMap::new(),
            target: Some("hls".to_string()),
            is_slot: false,
            self_area: IndexMap::new(),
            total_area: IndexMap::new(),
            clock_period: "0".to_string(),
        },
    );
    let mut child_tasks = IndexMap::new();
    child_tasks.insert("Add".to_string(), json!([{"args": {}, "step": 0}]));
    tasks.insert(
        "VecAdd".to_string(),
        TaskTopology {
            name: "VecAdd".to_string(),
            level: "upper".to_string(),
            code: "void VecAdd() {}\n".to_string(),
            ports: Vec::new(),
            tasks: child_tasks,
            fifos: IndexMap::new(),
            target: Some("hls".to_string()),
            is_slot: false,
            self_area: IndexMap::new(),
            total_area: IndexMap::new(),
            clock_period: "3.33".to_string(),
        },
    );
    let design = Design {
        top: "VecAdd".to_string(),
        target: "xilinx-hls".to_string(),
        tasks,
        slot_task_name_to_fp_region: None,
    };
    design_io::store_design(work, &design).expect("store design");
    let mut settings = settings_io::Settings::new();
    settings.insert("target".to_string(), json!("xilinx-hls"));
    settings_io::store_settings(work, &settings).expect("store settings");

    // `synth_args()` forces `gen_ab_graph=true` which reads
    // `cpp_arg_pre_assignments` from the floorplan config.
    std::fs::write(
        work.join("fp.json"),
        br#"{"cpp_arg_pre_assignments": {}}"#,
    )
    .expect("seed fp.json");
}

/// Regression: `generate-floorplan` forces `enable_synth_util =
/// true`. This test drives the native synth step through a
/// combined HLS+Vivado mock so the enable-synth-util path runs to
/// completion and updates `design.tasks[top].total_area`.
#[test]
fn generate_floorplan_synth_step_with_enable_synth_util() {
    use crate::globals::GlobalArgs;
    use crate::state::design as design_io;

    let tmp = tempfile::tempdir().expect("tempdir");
    let work = tmp.path();
    seed_vadd_work_dir(work);

    let globals = GlobalArgs::try_parse_from([
        "tapa",
        "--work-dir",
        work.to_str().expect("utf-8"),
    ])
    .expect("parse globals");
    let ctx = CliContext::from_globals(&globals);

    let fp_args = GenerateFloorplanArgs::try_parse_from([
        "generate-floorplan",
        "--input",
        "a.cpp",
        "--top",
        "VecAdd",
        "--part-num",
        "xcvu37p-fsvh2892-2L-e",
        "--clock-period",
        "3.33",
        "--device-config",
        "dev.json",
        "--floorplan-config",
        work.join("fp.json").to_str().expect("utf-8"),
    ])
    .expect("parse");
    let synth_args = fp_args.synth_args();
    assert!(
        synth_args.enable_synth_util,
        "generate-floorplan must force --enable-synth-util",
    );

    let runner = HlsAndVivadoStub {
        hls_q: Mutex::new(vec!["Add".to_string(), "VecAdd".to_string()]),
    };
    synth::run_native(&synth_args, &ctx, &runner)
        .expect("synth with enable_synth_util must no longer reject");

    // The post-synth-util fold must have rewritten `Add.total_area`.
    let reloaded =
        design_io::load_design(work).expect("reload design after synth");
    let add = reloaded.tasks.get("Add").expect("Add present");
    assert_eq!(
        add.total_area.get("LUT"),
        Some(&serde_json::json!(11)),
        "post-synth-util must update LUT from the Vivado rpt",
    );
    // BRAM_18K = RAMB36*2 + RAMB18 = 5*2 + 6 = 16
    assert_eq!(
        add.total_area.get("BRAM_18K"),
        Some(&serde_json::json!(16)),
    );
}
