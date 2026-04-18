//! `run_native` orchestrator for `tapa synth`.
//!
//! Threads device resolution → settings persistence → cpp-extract →
//! HLS runs → RTL codegen → final settings/design persistence. Also
//! owns the unsupported-flag gating, the HLS cflag construction, and
//! the recursive Verilog-file walker that feeds the codegen step.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tapa_xilinx::ToolRunner;

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, settings as settings_io};

use super::cpp_extract::extract_hls_sources;
use super::device_resolve::resolve_device_info;
use super::gen_ab_graph::emit_ab_graph;
use super::gen_graphir::emit_graphir;
use super::grouping_constraints::emit_grouping_constraints;
use super::hls_run::{run_hls_for_leaves, HlsRunOptions};
use super::rtl_codegen::{generate_rtl_tree, write_templates_info, TaskHdlInputs};
use super::SynthArgs;

/// Native synth: validate the flag surface, resolve the device, persist
/// settings, then drive cpp-extract → HLS → codegen for the leaf tasks.
pub(super) fn run_native(
    args: &SynthArgs,
    ctx: &CliContext,
    runner: &dyn ToolRunner,
) -> Result<()> {
    reject_unsupported_flags(args)?;
    validate_optional_flag_combos(args)?;

    let design = design_io::load_design(&ctx.work_dir)?;
    let mut settings = settings_io::load_settings(&ctx.work_dir)?;
    let target = settings
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or(&design.target)
        .to_string();
    if !matches!(target.as_str(), "xilinx-vitis" | "xilinx-hls") {
        return Err(CliError::InvalidArg(format!(
            "native synth only supports `xilinx-vitis` / `xilinx-hls` targets; got `{target}`. \
             Rerun with `TAPA_STEP_SYNTH_PYTHON=1` for AIE / other targets."
        )));
    }

    let device = resolve_device_info(args)?;
    settings.insert("part_num".to_string(), json!(&device.part_num));
    settings.insert(
        "platform".to_string(),
        args.platform
            .as_ref()
            .map_or(Value::Null, |p| Value::String(p.clone())),
    );
    settings.insert(
        "clock_period".to_string(),
        json!(&device.clock_period),
    );
    settings_io::store_settings(&ctx.work_dir, &settings)?;

    extract_hls_sources(&ctx.work_dir, &design)?;

    let opts = HlsRunOptions {
        part_num: device.part_num,
        clock_period: device.clock_period,
        other_configs: args.other_hls_configs.clone(),
        cflags: build_hls_cflags(),
        skip_based_on_mtime: args.skip_hls_based_on_mtime,
    };
    let hls_results = run_hls_for_leaves(runner, &ctx.work_dir, &design, &opts)?;

    let mut hdl_inputs: TaskHdlInputs = TaskHdlInputs::new();
    for (task_name, layout, out) in &hls_results {
        let mut files = out.verilog_files.clone();
        files.extend(walk_verilog_files(&layout.hdl_dir));
        files.sort();
        files.dedup();
        hdl_inputs.insert(task_name.clone(), files);
    }
    generate_rtl_tree(&ctx.work_dir, &design, &hdl_inputs)?;

    // Post-codegen side effects that mirror the tail of Python's
    // `_execute_synth`: nonpipeline-fifos → grouping_constraints.json,
    // gen-ab-graph → ab_graph.json, gen-graphir → graphir.json. Order
    // matches Python (constraints → ab → graphir). Each branch is a
    // no-op when its flag is not set.
    if let Some(fifos_path) = args.nonpipeline_fifos.as_ref() {
        emit_grouping_constraints(&ctx.work_dir, &design, fifos_path)?;
    }
    if args.gen_ab_graph {
        let Some(fp_cfg) = args.floorplan_config.as_ref() else {
            return Err(CliError::InvalidArg(
                "`--gen-ab-graph` requires `--floorplan-config <FILE>` \
                 (the source of `cpp_arg_pre_assignments`)"
                    .to_string(),
            ));
        };
        emit_ab_graph(&ctx.work_dir, &design, fp_cfg)?;
    }
    if args.gen_graphir {
        let (Some(dev_cfg), Some(fp_path)) =
            (args.device_config.as_ref(), args.floorplan_path.as_ref())
        else {
            return Err(CliError::InvalidArg(
                "`--gen-graphir` requires both `--device-config <FILE>` and \
                 `--floorplan-path <FILE>`"
                    .to_string(),
            ));
        };
        emit_graphir(&ctx.work_dir, &design, dev_cfg, fp_path)?;
    }

    write_templates_info(&ctx.work_dir, &design)?;
    settings.insert("synthed".to_string(), Value::Bool(true));
    settings_io::store_settings(&ctx.work_dir, &settings)?;
    design_io::store_design(&ctx.work_dir, &design)?;

    let mut flow = ctx.flow.borrow_mut();
    flow.settings = Some(settings);
    flow.design = Some(design);
    flow.pipelined.insert("synth".to_string(), true);
    drop(flow);

    Ok(())
}

/// Flags that still require tooling we do not drive from Rust yet. Only
/// `--enable-synth-util` lands here: it spawns per-task Vivado synth runs
/// via `ReportDirUtil` in Python (`ProgramSynthesisMixin.generate_post_synth_util`),
/// and there is no Rust-side Vivado harness at this layer. `--nonpipeline-fifos`,
/// `--gen-ab-graph`, `--gen-graphir`, and `--floorplan-path` are all
/// wired natively (see `emit_grouping_constraints`, `emit_ab_graph`,
/// `emit_graphir`, and `apply_floorplan` via the `floorplan` step).
fn reject_unsupported_flags(args: &SynthArgs) -> Result<()> {
    if args.enable_synth_util {
        return Err(CliError::InvalidArg(
            "`--enable-synth-util` requires Vivado on PATH to drive per-task \
             out-of-context synth and parse the hierarchical utilization report \
             (Python `ProgramSynthesisMixin.generate_post_synth_util`). The \
             native port has no Vivado harness at this layer, so this branch \
             is unavailable until that harness lands."
                .to_string(),
        ));
    }
    Ok(())
}

/// Secondary validation for flag combinations that are accepted natively
/// but depend on sibling flags. Surfaces up front so failures happen
/// before the (expensive) HLS loop runs.
fn validate_optional_flag_combos(args: &SynthArgs) -> Result<()> {
    if args.gen_ab_graph && args.floorplan_config.is_none() {
        return Err(CliError::InvalidArg(
            "`--gen-ab-graph` requires `--floorplan-config <FILE>` \
             (source of `cpp_arg_pre_assignments`)"
                .to_string(),
        ));
    }
    if args.gen_graphir
        && (args.device_config.is_none() || args.floorplan_path.is_none())
    {
        return Err(CliError::InvalidArg(
            "`--gen-graphir` requires both `--device-config <FILE>` and \
             `--floorplan-path <FILE>`"
                .to_string(),
        ));
    }
    Ok(())
}

/// Build the HLS CFLAGS that `_build_hls_cflags` constructs in Python.
/// At this stage we only emit the `-DTAPA_TARGET_*` defines; the full
/// vendor-include resolution is intentionally out of scope.
///
/// **Limitation**: production `tapa.cflags` from `graph.json` `cflags`
/// is not threaded through; designs needing extra `-I` includes need
/// `TAPA_STEP_SYNTH_PYTHON=1` until a follow-up loads the analyzer's
/// stored cflags from `<work_dir>/graph.json`.
fn build_hls_cflags() -> Vec<String> {
    vec![
        "-DTAPA_TARGET_DEVICE_".to_string(),
        "-DTAPA_TARGET_XILINX_HLS_".to_string(),
    ]
}

fn walk_verilog_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.is_dir() {
            out.extend(walk_verilog_files(&path));
        } else if path.extension().and_then(|s| s.to_str()) == Some("v") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    use std::sync::Mutex;

    use clap::Parser;
    use indexmap::IndexMap;
    use tapa_task_graph::{Design, TaskTopology};
    use tapa_xilinx::{ToolInvocation, ToolOutput};

    use crate::globals::GlobalArgs;

    fn parse_synth(extra: &[&str]) -> SynthArgs {
        let mut argv = vec!["synth"];
        argv.extend_from_slice(extra);
        SynthArgs::try_parse_from(argv).expect("parse synth args")
    }

    fn ctx_with_work_dir(work_dir: &Path) -> CliContext {
        let globals = GlobalArgs::try_parse_from([
            "tapa",
            "--work-dir",
            work_dir.to_str().expect("utf-8 work dir"),
        ])
        .expect("parse globals");
        CliContext::from_globals(&globals)
    }

    /// `ToolRunner` stub that pre-stages a well-formed
    /// `project/<top>/syn/{report,verilog}` tree under `cwd` so
    /// `tapa_xilinx::run_hls`'s harvester succeeds.
    struct StubHls {
        responses: Mutex<Vec<(String, String)>>,
    }

    impl StubHls {
        fn new(responses: Vec<(String, String)>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl ToolRunner for StubHls {
        fn run(&self, inv: &ToolInvocation) -> tapa_xilinx::Result<ToolOutput> {
            let cwd = inv.cwd.clone().expect("HLS sets cwd");
            let mut q = self.responses.lock().expect("poisoned");
            let (top, body) = q.first().cloned().expect("StubHls: no response queued");
            q.remove(0);
            let syn = cwd.join("project").join(&top).join("syn");
            std::fs::create_dir_all(syn.join("report")).expect("mkdir report");
            std::fs::create_dir_all(syn.join("verilog")).expect("mkdir verilog");
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
            ).expect("csynth.xml");
            std::fs::write(syn.join("verilog").join(format!("{top}.v")), body)
                .expect("write v");
            Ok(ToolOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn reject_case(extra: &[&str], needle: &str) {
        let args = parse_synth(extra);
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ctx_with_work_dir(dir.path());
        let runner = StubHls::new(Vec::new());
        let err = run_native(&args, &ctx, &runner).expect_err("must reject early");
        assert!(
            matches!(err, CliError::InvalidArg(ref m) if m.contains(needle)),
            "error must contain `{needle}`; got: {err}",
        );
    }

    /// `--enable-synth-util` is the only remaining branch that routes
    /// to a typed `InvalidArg`: the message names Vivado (the concrete
    /// native gap) rather than the retired `TAPA_STEP_SYNTH_PYTHON=1`.
    #[test]
    fn enable_synth_util_surfaces_native_gap() {
        reject_case(&["--platform", "xilinx_u250", "--enable-synth-util"], "Vivado");
    }

    #[test]
    fn gen_ab_graph_without_floorplan_config_rejected_early() {
        reject_case(
            &["--platform", "xilinx_u250", "--gen-ab-graph"],
            "--floorplan-config",
        );
    }

    #[test]
    fn gen_graphir_without_device_or_floorplan_path_rejected_early() {
        reject_case(
            &["--platform", "xilinx_u250", "--gen-graphir"],
            "--device-config",
        );
    }

    #[test]
    fn native_synth_writes_full_pipeline_artifacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path();
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
        child_tasks
            .insert("Add".to_string(), serde_json::json!([{"args": {}, "step": 0}]));
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

        // Two HLS invocations: the leaf `Add` and the upper-task shell
        // `VecAdd`. Iteration order matches `IndexMap` insertion order,
        // which mirrors Python's `Task` topological sort.
        let stub_module = |name: &str| -> String {
            format!(
                "module {name}(\n  input wire ap_clk,\n  input wire ap_rst_n,\n  \
                 input wire ap_start,\n  output wire ap_done,\n  output wire ap_idle,\n  \
                 output wire ap_ready\n);\nendmodule\n"
            )
        };
        let runner = StubHls::new(vec![
            ("Add".into(), stub_module("Add")),
            ("VecAdd".into(), stub_module("VecAdd")),
        ]);
        let ctx = ctx_with_work_dir(work);
        let args = parse_synth(&[
            "--part-num",
            "xcvu37p-fsvh2892-2L-e",
            "--clock-period",
            "3.33",
        ]);
        run_native(&args, &ctx, &runner).expect("native synth must succeed end-to-end");

        assert!(work.join("design.json").is_file(), "design.json must persist");
        assert!(work.join("settings.json").is_file(), "settings.json must persist");
        assert!(work.join("templates_info.json").is_file(), "templates_info.json must persist");
        assert!(work.join("hls/Add/verilog").is_dir(), "hls/Add/verilog must exist");
        assert!(work.join("rtl").is_dir(), "rtl directory must exist");
        assert!(work.join("rtl/VecAdd.v").is_file(), "rtl/VecAdd.v must be emitted");
        assert!(
            work.join("rtl/VecAdd_fsm.v").is_file(),
            "rtl/VecAdd_fsm.v must be emitted (upper task FSM)",
        );

        let settings = settings_io::load_settings(work).expect("load");
        assert_eq!(settings.get("synthed"), Some(&Value::Bool(true)));
        assert_eq!(
            settings.get("part_num"),
            Some(&json!("xcvu37p-fsvh2892-2L-e")),
        );
        assert_eq!(settings.get("clock_period"), Some(&json!("3.33")));
        assert_eq!(settings.get("platform"), Some(&Value::Null));

        let templates = std::fs::read_to_string(work.join("templates_info.json")).expect("read");
        assert_eq!(templates, "{}");

        let flow = ctx.flow.borrow();
        assert!(flow.design.is_some());
        assert!(flow.settings.is_some());
        assert_eq!(flow.pipelined.get("synth"), Some(&true));
    }
}
