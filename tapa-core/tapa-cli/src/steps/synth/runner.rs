//! `run_native` orchestrator for `tapa synth`.
//!
//! Threads device resolution → state persistence → cpp-extract →
//! HLS runs → RTL codegen → final state persistence. Also
//! owns the unsupported-flag gating, the HLS cflag construction, and
//! the recursive Verilog-file walker that feeds the codegen step.

use serde_json::Value;
use tapa_ir::Task;
use tapa_xilinx::{CsynthReport, ToolRunner};

use crate::context::CliContext;
use crate::error::Result;
use crate::state::work as work_io;
use crate::tapacc::cflags::{get_remote_hls_cflags, get_tapacc_cflags};
use crate::tapacc::discover::find_resource;

use super::cpp_extract::extract_hls_sources;
use super::device_resolve::resolve_device_info;
use super::hls_run::{run_hls_for_leaves, HlsRunOptions};
use super::post_synth_util::emit_post_synth_util;
use super::report::write_top_report;
use super::rtl_codegen::{generate_rtl_tree, write_templates_info, TaskHdlInputs};
use super::SynthArgs;

/// Validate the flag surface, resolve the device, persist settings,
/// then drive cpp-extract → HLS → codegen for the leaf tasks.
#[allow(
    clippy::too_many_lines,
    reason = "orchestrator function; refactored extract would bounce values through another builder without adding clarity"
)]
pub fn run_native(args: &SynthArgs, ctx: &CliContext, runner: &dyn ToolRunner) -> Result<()> {
    // The flow target is typed and validated by the state parse itself, so
    // there is nothing left for this step to re-check: synthesis is Xilinx
    // HLS for every supported flow.
    let mut state = work_io::load(&ctx.work_dir)?;

    let device = resolve_device_info(args)?;
    state.flow.part_num = Some(device.part_num.clone());
    state.flow.platform.clone_from(&args.platform);
    state.flow.clock_period = Some(device.clock_period.clone());
    // A synthesis rerun may change RTL, area, device, or timing inputs. Make
    // every result derived from the previous RTL inactive before the long HLS
    // runs, so an interrupted rerun cannot be packed or floorplanned as if it
    // had completed successfully.
    state.flow.synthed = false;
    state.floorplan = None;
    work_io::store(&ctx.work_dir, &state)?;

    extract_hls_sources(&ctx.work_dir, &state.graph)?;

    let opts = HlsRunOptions {
        part_num: device.part_num.clone(),
        clock_period: device.clock_period.clone(),
        other_configs: args.other_hls_configs.clone(),
        cflags: build_hls_cflags(&state.graph.cflags, ctx.remote_config.is_some()),
        skip_based_on_mtime: args.skip_hls_based_on_mtime,
        jobs: args.jobs,
        keep_work_dir: args.keep_hls_work_dir,
    };
    let hls_results = run_hls_for_leaves(runner, &ctx.work_dir, &state.graph, &opts)?;

    let mut hdl_inputs: TaskHdlInputs = TaskHdlInputs::new();
    for (task_name, layout, out) in &hls_results {
        let mut files = out.verilog_files.clone();
        files.extend(super::hls_run::list_hdl_files(&layout.hdl_dir)?);
        files.sort();
        files.dedup();
        hdl_inputs.insert(task_name.clone(), files);

        if let Some(task) = state.graph.tasks.get_mut(task_name) {
            apply_hls_metrics(task, &out.csynth);
        }
    }
    generate_rtl_tree(&ctx.work_dir, &state.graph, &hdl_inputs, None)?;

    // Per-task OOC Vivado synth → hierarchical utilization → `total_area`.
    // When disabled (the default), reports and topology consumers derive
    // effective totals recursively from the HLS-populated `self_area` maps.
    if args.enable_synth_util {
        emit_post_synth_util(
            &ctx.work_dir,
            &mut state.graph,
            &device.part_num,
            args.jobs,
            runner,
        )?;
    }

    // Emit `report.{json,yaml}` at the work-dir root once area data is
    // final. Both pack paths (`xilinx-vitis` `.xo` and `xilinx-hls`
    // `.zip`) bundle the YAML at archive root.
    write_top_report(
        &ctx.work_dir,
        &state.graph,
        &args.override_report_schema_version,
    )?;

    write_templates_info(&ctx.work_dir, &state.graph)?;
    state.flow.synthed = true;
    work_io::store(&ctx.work_dir, &state)?;

    Ok(())
}

/// Apply the raw metrics reported by HLS to a task.
///
/// `clock_period` stores the achieved estimate, not the requested target.
/// `total_area` is cleared because it is either re-populated by optional
/// out-of-context synthesis or derived recursively from `self_area` by report
/// and topology consumers.
fn apply_hls_metrics(task: &mut Task, report: &CsynthReport) {
    task.clock_period
        .clone_from(&report.estimated_clock_period_ns);
    task.self_area.clear();
    task.total_area.clear();
    for (key, value) in &report.area {
        if let Ok(value) = value.parse::<i64>() {
            task.self_area.insert(key.clone(), Value::from(value));
        } else {
            task.self_area
                .insert(key.clone(), Value::String(value.clone()));
        }
    }
}

/// Build the HLS CFLAGS: the analyzer-stored `graph_cflags` (so
/// `-isystem <tapa-lib>` etc. are forwarded into HLS) followed by the
/// `-DTAPA_TARGET_*` defines and a `-I <tapa-extra-runtime-include>` entry
/// when the resource can be resolved ("WORKAROUND: Vitis HLS requires -I or
/// gflags cannot be found..." branch).
fn build_hls_cflags(graph_cflags: &[String], remote: bool) -> Vec<String> {
    // HLS cflags = the analyzer-stored graph cflags followed by
    // `get_tapacc_cflags()`, so HLS sees the user's own `-I` / `-D`
    // entries plus the tapa-lib / vendor-include resolution.
    let mut flags: Vec<String> = graph_cflags.to_vec();
    // Remote HLS substitutes `get_tapacc_cflags()` (local
    // vendor/stdlib paths) with `get_remote_hls_cflags()` when
    // `~/.taparc` is active — the
    // remote host ships its own vendor headers via `settings64.sh`,
    // and a host-specific macOS `__assert_rtn` remap is added so
    // macOS → Linux remote synth does not mis-expand assert() into
    // `__assert_rtn(func,file,line,expr)`.
    if remote {
        flags.extend(get_remote_hls_cflags());
    } else {
        flags.extend(get_tapacc_cflags(false));
    }
    flags.push("-DTAPA_TARGET_DEVICE_".to_string());
    flags.push("-DTAPA_TARGET_XILINX_HLS_".to_string());
    // Vitis HLS requires `-I` (not `-isystem`) to locate gflags during build.
    if let Ok(p) = find_resource("tapa-extra-runtime-include") {
        flags.push(format!("-I{}", p.display()));
    }
    flags
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::similar_names,
        reason = "the `args`/`argv` pair appears throughout the dispatcher; \
                  matching the production names keeps tests legible"
    )]

    use super::*;

    use std::path::Path;
    use std::sync::Mutex;

    use clap::Parser;
    use serde_json::json;
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{Area, FloorplanResult, SynthTarget, Target, TaskGraph, TaskLevel};
    use tapa_xilinx::{ToolInvocation, ToolOutput};

    use crate::globals::GlobalArgs;
    use crate::state::work::WorkState;

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
            // Route by TAPA_KERNEL_PATH_0's cpp basename so parallel
            // HLS dispatch (new default) stages each task under the
            // correct `project/<task>/syn/` tree. Queue is still
            // consulted for the verilog body content, keyed on top.
            let inferred_top = inv.env.get("TAPA_KERNEL_PATH_0").and_then(|p| {
                std::path::Path::new(p)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            });
            let mut q = self.responses.lock().expect("poisoned");
            let (top, body) = if let Some(name) = inferred_top {
                let idx = q.iter().position(|(t, _)| t == &name).unwrap_or_else(|| {
                    let queued_tops = q.iter().map(|(top, _)| top.as_str()).collect::<Vec<_>>();
                    panic!(
                        "StubHls: no response queued for inferred top `{name}`; queued \
                         tops: {queued_tops:?}"
                    );
                });
                let (_, body) = q.remove(idx);
                (name, body)
            } else {
                let (top, body) = q.first().cloned().expect("StubHls: no response queued");
                q.remove(0);
                (top, body)
            };
            let syn = cwd.join("project").join(&top).join("syn");
            fs_err::create_dir_all(syn.join("report")).expect("mkdir report");
            fs_err::create_dir_all(syn.join("verilog")).expect("mkdir verilog");
            fs_err::write(
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
            .expect("csynth.xml");
            fs_err::write(syn.join("verilog").join(format!("{top}.v")), body).expect("write v");
            Ok(ToolOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn hls_metrics_use_estimated_clock_and_clear_stale_total() {
        let mut task = Task {
            level: TaskLevel::Lower,
            code: "void Add() {}\n".to_string(),
            ports: Vec::new(),
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            readable_name: String::new(),
            synth: SynthTarget::Hls,
            self_area: IndexMap::from([("LUT".to_string(), json!(999))]),
            total_area: IndexMap::from([("LUT".to_string(), json!(999))]),
            clock_period: "9.99".to_string(),
        };
        let report = CsynthReport {
            top: "Add".to_string(),
            part: "xcvu37p".to_string(),
            target_clock_period_ns: "3.33".to_string(),
            estimated_clock_period_ns: "1.25".to_string(),
            area: IndexMap::from([
                ("LUT".to_string(), "42".to_string()),
                ("FF".to_string(), "21".to_string()),
            ]),
        };

        apply_hls_metrics(&mut task, &report);

        assert_eq!(task.clock_period, "1.25");
        assert_eq!(task.self_area.get("LUT"), Some(&json!(42)));
        assert_eq!(task.self_area.get("FF"), Some(&json!(21)));
        assert!(
            task.total_area.is_empty(),
            "stale post-synthesis totals must not survive a new HLS run"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "integration test with many assertions"
    )]
    fn synth_writes_full_pipeline_artifacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path();
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Add".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: "void Add() {}\n".to_string(),
                ports: Vec::new(),
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let mut child_tasks = BTreeMap::new();
        child_tasks.insert(
            "Add".to_string(),
            vec![tapa_ir::TaskInstance {
                name: None,
                args: BTreeMap::new(),
                step: 0,
            }],
        );
        tasks.insert(
            "VecAdd".to_string(),
            Task {
                level: TaskLevel::Upper,
                code: "void VecAdd() {}\n".to_string(),
                ports: Vec::new(),
                tasks: child_tasks,
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "3.33".to_string(),
            },
        );
        let mut state = WorkState::new(TaskGraph {
            top: "VecAdd".to_string(),
            target: Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        });
        state.flow.synthed = true;
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 3),
            regions: BTreeMap::from([("Add_0".to_string(), "SLOT_X0Y0_TO_SLOT_X0Y0".to_string())]),
            routes: Vec::new(),
            slot_usage: BTreeMap::from([("SLOT_X0Y0_TO_SLOT_X0Y0".to_string(), Area::default())]),
        });
        work_io::store(work, &state).expect("store state");

        // Two HLS invocations: the leaf `Add` and the upper-task shell
        // `VecAdd`. Iteration order is `BTreeMap` alphabetical order,
        // which matches the sorted order `tapa analyze` writes.
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

        assert!(work.join("tapa.json").is_file(), "tapa.json must persist");
        assert!(
            work.join("templates_info.json").is_file(),
            "templates_info.json must persist"
        );
        assert!(
            work.join("hls/Add/verilog").is_dir(),
            "hls/Add/verilog must exist"
        );
        assert!(work.join("rtl").is_dir(), "rtl directory must exist");
        assert!(
            work.join("rtl/VecAdd.v").is_file(),
            "rtl/VecAdd.v must be emitted"
        );
        assert!(
            work.join("rtl/VecAdd_fsm.v").is_file(),
            "rtl/VecAdd_fsm.v must be emitted (upper task FSM)",
        );

        let templates = std::fs::read_to_string(work.join("templates_info.json")).expect("read");
        assert_eq!(templates, "{}");

        // The one state file carries both the resolved flow settings and the
        // graph that synth annotated in place.
        let persisted = work_io::load(work).expect("load persisted state");
        assert!(persisted.flow.synthed, "synthed flag must persist");
        assert!(
            persisted.floorplan.is_none(),
            "synth must invalidate a floorplan derived from the previous RTL",
        );
        assert_eq!(
            persisted.flow.part_num.as_deref(),
            Some("xcvu37p-fsvh2892-2L-e"),
        );
        assert_eq!(persisted.flow.clock_period.as_deref(), Some("3.33"));
        assert_eq!(
            persisted.flow.platform, None,
            "no --platform was passed, so none must be recorded",
        );
        assert_eq!(
            persisted.graph.target,
            Target::XilinxHls,
            "the flow target must survive synth's rewrite",
        );
        assert_eq!(persisted.graph.tasks["Add"].clock_period, "1.0");
        assert_eq!(persisted.graph.tasks["VecAdd"].clock_period, "1.0");
        let report: Value = serde_json::from_str(
            &std::fs::read_to_string(work.join("report.json")).expect("read report"),
        )
        .expect("parse report");
        assert_eq!(report["performance"]["clock_period"], "1.0");
    }
}
