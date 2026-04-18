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
use crate::state::{design as design_io, graph as graph_io, settings as settings_io};
use crate::tapacc::cflags::get_tapacc_cflags;
use crate::tapacc::discover::find_resource;

use super::cpp_extract::extract_hls_sources;
use super::device_resolve::resolve_device_info;
use super::gen_ab_graph::emit_ab_graph;
use super::gen_graphir::emit_graphir;
use super::grouping_constraints::emit_grouping_constraints;
use super::hls_run::{run_hls_for_leaves, HlsRunOptions};
use super::post_synth_util::emit_post_synth_util;
use super::rtl_codegen::{generate_rtl_tree, write_templates_info, TaskHdlInputs};
use super::SynthArgs;

/// Native synth: validate the flag surface, resolve the device, persist
/// settings, then drive cpp-extract → HLS → codegen for the leaf tasks.
pub fn run_native(
    args: &SynthArgs,
    ctx: &CliContext,
    runner: &dyn ToolRunner,
) -> Result<()> {
    validate_optional_flag_combos(args)?;

    let mut design = design_io::load_design(&ctx.work_dir)?;
    let mut settings = settings_io::load_settings(&ctx.work_dir)?;
    let target = settings
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or(&design.target)
        .to_string();
    if !matches!(target.as_str(), "xilinx-vitis" | "xilinx-hls") {
        return Err(CliError::InvalidArg(format!(
            "native synth only supports the `xilinx-vitis` and `xilinx-hls` \
             targets; got `{target}`. AIE / Intel / software-emulation \
             targets are not yet ported."
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
        part_num: device.part_num.clone(),
        clock_period: device.clock_period.clone(),
        other_configs: args.other_hls_configs.clone(),
        cflags: build_hls_cflags(&ctx.work_dir),
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

    // Per-task OOC Vivado synth → hierarchical utilization → `total_area`.
    // Mirrors `ProgramSynthesisMixin.generate_post_synth_util`. When
    // disabled (the Python default), the HLS-populated self/total areas
    // survive untouched.
    if args.enable_synth_util {
        emit_post_synth_util(
            &ctx.work_dir,
            &mut design,
            &device.part_num,
            args.jobs,
            runner,
        )?;
    }

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

/// Validation for flag combinations that are accepted natively
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
///
/// Mirrors `tapa/program/hls.py::ProgramHlsMixin._build_hls_cflags`:
/// loads the analyzer-stored cflags tuple from
/// `<work_dir>/graph.json::cflags` (so `-isystem <tapa-lib>` etc. are
/// forwarded into HLS), then appends the `-DTAPA_TARGET_*` defines and
/// a `-I <tapa-extra-runtime-include>` entry when the resource can be
/// resolved (Python's "WORKAROUND: Vitis HLS requires -I or gflags
/// cannot be found..." branch).
fn build_hls_cflags(work_dir: &Path) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    // Python parity: `tapa/core.py::TapaProgram.__init__` builds
    // `self.cflags = " ".join((*graph.cflags, *get_tapacc_cflags()))`.
    // We mirror that here so HLS sees the user's own `-I` / `-D`
    // entries plus the tapa-lib / vendor-include resolution.
    //
    // Missing `graph.json` is tolerated so unit tests that seed only
    // `design.json` + `settings.json` still drive the runner.
    if let Ok(graph) = graph_io::load_graph(work_dir) {
        if let Some(arr) = graph.get("cflags").and_then(Value::as_array) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    flags.push(s.to_string());
                }
            }
        }
    }
    flags.extend(get_tapacc_cflags(false));
    flags.push("-DTAPA_TARGET_DEVICE_".to_string());
    flags.push("-DTAPA_TARGET_XILINX_HLS_".to_string());
    // Python `_build_hls_cflags` workaround: Vitis HLS requires `-I`
    // (not `-isystem`) to locate gflags during build.
    if let Ok(p) = find_resource("tapa-extra-runtime-include") {
        flags.push(format!("-I{}", p.display()));
    }
    flags
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
