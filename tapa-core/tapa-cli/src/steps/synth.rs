//! `tapa synth` — native Rust port of `tapa/steps/synth.py`.
//!
//! For the vadd-style happy path (`--platform <p>`, no DSE / graphir /
//! floorplan, leaf children + one upper top), this module drives the
//! full Vitis HLS + RTL codegen pipeline natively:
//!
//!   1. Resolve the device (part / clock / platform) via
//!      `tapa_xilinx::parse_device_info` and persist into
//!      `<work_dir>/settings.json`.
//!   2. Extract per-task C++ from `design.json` to `<work_dir>/cpp/`
//!      (mirrors `tapa/program/hls.py::ProgramHlsMixin._extract_cpp`).
//!   3. Run Vitis HLS for each leaf task via `tapa_xilinx::run_hls`,
//!      harvesting Verilog into `<work_dir>/hls/<task>/verilog/`
//!      (mirrors `ProgramHlsMixin.run_hls` / `_run_hls_task`).
//!   4. Drive `tapa_codegen::generate_rtl` to instrument upper tasks
//!      and emit `<work_dir>/rtl/{<task>.v, <task>_fsm.v, ...}`
//!      (mirrors `tapa/codegen/program_rtl.py::generate_task_rtl` +
//!      `generate_top_rtl`).
//!   5. Persist `<work_dir>/templates_info.json` and re-store the
//!      design + settings (`synthed=true`).
//!
//! Feature flags that require ports we have not yet landed
//! (`--gen-ab-graph`, `--gen-graphir`, `--floorplan-path`,
//! `--nonpipeline-fifos`, `--enable-synth-util`) still surface a typed
//! [`CliError::InvalidArg`] up front. The Python bridge remains
//! reachable behind `TAPA_STEP_SYNTH_PYTHON=1`.

use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::{json, Value};
use tapa_xilinx::{
    parse_device_info as xilinx_parse_device_info, DeviceInfo, LocalToolRunner, ToolRunner,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{design as design_io, settings as settings_io};
use crate::steps::python_bridge;

mod cpp_extract;
mod hls_run;
mod rtl_codegen;

use cpp_extract::extract_hls_sources;
use hls_run::{run_hls_for_leaves, HlsRunOptions};
use rtl_codegen::{generate_rtl_tree, write_templates_info, TaskHdlInputs};

#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors the click flag surface in tapa/steps/synth.py — every bool \
              is a distinct user-facing flag, so collapsing into an enum would \
              break parity"
)]
#[derive(Debug, Clone, Parser)]
#[command(name = "synth", about = "Synthesize the TAPA program into RTL code.")]
pub struct SynthArgs {
    #[arg(long = "part-num", value_name = "PART")]
    pub part_num: Option<String>,

    #[arg(short = 'p', long = "platform", value_name = "PLATFORM")]
    pub platform: Option<String>,

    #[arg(long = "clock-period", value_name = "NS")]
    pub clock_period: Option<f64>,

    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<u32>,

    #[arg(long = "keep-hls-work-dir", default_value_t = false)]
    pub keep_hls_work_dir: bool,

    #[arg(long = "remove-hls-work-dir", conflicts_with = "keep_hls_work_dir")]
    pub remove_hls_work_dir: bool,

    #[arg(long = "skip-hls-based-on-mtime", default_value_t = false)]
    pub skip_hls_based_on_mtime: bool,

    #[arg(long = "no-skip-hls-based-on-mtime", conflicts_with = "skip_hls_based_on_mtime")]
    pub no_skip_hls_based_on_mtime: bool,

    #[arg(long = "other-hls-configs", default_value = "")]
    pub other_hls_configs: String,

    #[arg(long = "enable-synth-util", default_value_t = false)]
    pub enable_synth_util: bool,

    #[arg(long = "disable-synth-util", conflicts_with = "enable_synth_util")]
    pub disable_synth_util: bool,

    #[arg(long = "override-report-schema-version", default_value = "")]
    pub override_report_schema_version: String,

    #[arg(long = "nonpipeline-fifos", value_name = "FILE")]
    pub nonpipeline_fifos: Option<PathBuf>,

    #[arg(long = "gen-ab-graph", default_value_t = false)]
    pub gen_ab_graph: bool,

    #[arg(long = "no-gen-ab-graph", conflicts_with = "gen_ab_graph")]
    pub no_gen_ab_graph: bool,

    #[arg(long = "gen-graphir", default_value_t = false)]
    pub gen_graphir: bool,

    #[arg(long = "floorplan-config", value_name = "FILE")]
    pub floorplan_config: Option<PathBuf>,

    #[arg(long = "device-config", value_name = "FILE")]
    pub device_config: Option<PathBuf>,

    #[arg(long = "floorplan-path", value_name = "FILE")]
    pub floorplan_path: Option<PathBuf>,
}

fn opt_str(out: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push(flag.to_string());
        out.push(v.to_string());
    }
}

fn opt_path(out: &mut Vec<String>, flag: &str, value: Option<&PathBuf>) {
    if let Some(v) = value {
        out.push(flag.to_string());
        out.push(v.display().to_string());
    }
}

pub fn to_python_argv(args: &SynthArgs) -> Vec<String> {
    let mut out = Vec::<String>::new();
    opt_str(&mut out, "--part-num", args.part_num.as_deref());
    opt_str(&mut out, "--platform", args.platform.as_deref());
    if let Some(c) = args.clock_period {
        out.push("--clock-period".to_string());
        out.push(c.to_string());
    }
    if let Some(j) = args.jobs {
        out.push("--jobs".to_string());
        out.push(j.to_string());
    }
    out.push(if args.keep_hls_work_dir {
        "--keep-hls-work-dir"
    } else {
        "--remove-hls-work-dir"
    }
    .to_string());
    out.push(if args.skip_hls_based_on_mtime {
        "--skip-hls-based-on-mtime"
    } else {
        "--no-skip-hls-based-on-mtime"
    }
    .to_string());
    out.push("--other-hls-configs".to_string());
    out.push(args.other_hls_configs.clone());
    out.push(if args.enable_synth_util {
        "--enable-synth-util"
    } else {
        "--disable-synth-util"
    }
    .to_string());
    out.push("--override-report-schema-version".to_string());
    out.push(args.override_report_schema_version.clone());
    opt_path(&mut out, "--nonpipeline-fifos", args.nonpipeline_fifos.as_ref());
    out.push(if args.gen_ab_graph {
        "--gen-ab-graph"
    } else {
        "--no-gen-ab-graph"
    }
    .to_string());
    if args.gen_graphir {
        out.push("--gen-graphir".to_string());
    }
    opt_path(&mut out, "--floorplan-config", args.floorplan_config.as_ref());
    opt_path(&mut out, "--device-config", args.device_config.as_ref());
    opt_path(&mut out, "--floorplan-path", args.floorplan_path.as_ref());
    out
}

/// Top-level dispatcher: route to the Python bridge when the user has
/// opted in via `TAPA_STEP_SYNTH_PYTHON=1`, otherwise execute the full
/// native HLS + codegen pipeline using a `LocalToolRunner` for HLS.
pub fn run(args: &SynthArgs, ctx: &mut CliContext) -> Result<()> {
    if python_bridge::is_enabled("synth") {
        return python_bridge::run("synth", &to_python_argv(args), ctx);
    }
    let runner = LocalToolRunner::new();
    run_native(args, ctx, &runner)
}

/// Native synth: validate the flag surface, resolve the device, persist
/// settings, then drive cpp-extract → HLS → codegen for the leaf tasks.
fn run_native(args: &SynthArgs, ctx: &CliContext, runner: &dyn ToolRunner) -> Result<()> {
    reject_unsupported_flags(args)?;

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

fn reject_unsupported_flags(args: &SynthArgs) -> Result<()> {
    if args.nonpipeline_fifos.is_some() {
        return Err(CliError::InvalidArg(
            "`--nonpipeline-fifos` requires the Python `grouping_constraints.json` \
             generator; rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.gen_ab_graph {
        return Err(CliError::InvalidArg(
            "`--gen-ab-graph` requires the Python AutoBridge graph generator; \
             rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.gen_graphir {
        return Err(CliError::InvalidArg(
            "`--gen-graphir` requires the Python GraphIR project conversion; \
             rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.floorplan_path.is_some() {
        return Err(CliError::InvalidArg(
            "`--floorplan-path` requires the Python floorplan-aware codegen path; \
             rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    if args.enable_synth_util {
        return Err(CliError::InvalidArg(
            "`--enable-synth-util` requires the post-synth utility report path \
             (`Program.generate_post_synth_util`) which is not yet ported. \
             Rerun with `TAPA_STEP_SYNTH_PYTHON=1`."
                .to_string(),
        ));
    }
    Ok(())
}

fn resolve_device_info(args: &SynthArgs) -> Result<DeviceInfo> {
    let part_override = args.part_num.as_deref();
    let clock_override_owned = args.clock_period.map(|c| format!("{c}"));
    let clock_override = clock_override_owned.as_deref();

    if let Some(platform) = args.platform.as_deref() {
        let resolved = resolve_platform_dir(platform).ok_or_else(|| {
            CliError::InvalidArg(format!(
                "cannot find the specified platform `{platform}`; are you sure it has \
                 been installed, e.g., in `/opt/xilinx/platforms`?",
            ))
        })?;
        return Ok(xilinx_parse_device_info(
            &resolved,
            part_override,
            clock_override,
        )?);
    }

    let Some(part_num) = part_override else {
        return Err(CliError::InvalidArg(
            "cannot determine the target part number; please either specify \
             `--platform` so the target part number can be extracted from it, or \
             specify `--part-num` directly."
                .to_string(),
        ));
    };
    let Some(clock_period) = clock_override else {
        return Err(CliError::InvalidArg(
            "cannot determine the target clock period; please either specify \
             `--platform` so the target clock period can be extracted from it, or \
             specify `--clock-period` directly."
                .to_string(),
        ));
    };
    Ok(DeviceInfo {
        part_num: part_num.to_string(),
        clock_period: clock_period.to_string(),
    })
}

fn resolve_platform_dir(platform: &str) -> Option<PathBuf> {
    let raw = Path::new(platform);
    let parent = raw.parent().map(Path::to_path_buf).unwrap_or_default();
    let basename = raw.file_name().map_or_else(
        || platform.to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let normalized = basename.replace([':', '.'], "_");
    let direct = if parent.as_os_str().is_empty() {
        PathBuf::from(&normalized)
    } else {
        parent.join(&normalized)
    };
    if direct.is_dir() {
        return Some(direct);
    }
    for root in platform_roots() {
        let candidate = root.join("platforms").join(&normalized);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn platform_roots() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/opt/xilinx")];
    if let Ok(p) = std::env::var("XILINX_VITIS") {
        out.push(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("XILINX_SDX") {
        out.push(PathBuf::from(p));
    }
    out
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

    #[test]
    fn argv_round_trips_python_shape() {
        let args = parse_synth(&["--platform", "xilinx_u250", "--clock-period", "3.33"]);
        let argv = to_python_argv(&args);
        assert!(argv.contains(&"--platform".to_string()));
        assert!(argv.contains(&"xilinx_u250".to_string()));
        assert!(argv.contains(&"--clock-period".to_string()));
    }

    #[test]
    fn unsupported_flag_surfaces_invalid_arg() {
        let args = parse_synth(&[
            "--platform",
            "xilinx_u250",
            "--gen-graphir",
        ]);
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ctx_with_work_dir(dir.path());
        let runner = StubHls::new(Vec::new());
        let err = run_native(&args, &ctx, &runner).expect_err("must reject gen-graphir");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("--gen-graphir")));
    }

    #[test]
    fn part_num_without_clock_errors() {
        let args = parse_synth(&["--part-num", "xcvu37p"]);
        let err = resolve_device_info(&args).expect_err("missing clock");
        assert!(matches!(err, CliError::InvalidArg(ref m) if m.contains("clock period")));
    }

    #[test]
    fn part_num_and_clock_resolve_without_platform() {
        let args = parse_synth(&["--part-num", "xcvu37p-fsvh2892-2L-e", "--clock-period", "3.33"]);
        let info = resolve_device_info(&args).expect("must resolve");
        assert_eq!(info.part_num, "xcvu37p-fsvh2892-2L-e");
        assert_eq!(info.clock_period, "3.33");
    }

    #[test]
    fn resolve_platform_dir_normalizes_separators() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = "weird_platform:1.0";
        let normalized = "weird_platform_1_0";
        let target = dir.path().join(normalized);
        std::fs::create_dir_all(&target).expect("mkdir");
        let qualified = dir.path().join(raw);
        let resolved = resolve_platform_dir(qualified.to_str().expect("utf-8"))
            .expect("must resolve normalized basename");
        assert_eq!(resolved, target);
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
