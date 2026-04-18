//! Per-task out-of-context Vivado synth + hierarchical utilization
//! parsing. Ports `tapa/program/synthesis.py::ProgramSynthesisMixin
//! ::generate_post_synth_util` (the `worker` inner function plus the
//! `ThreadPoolExecutor.map` result fold) and
//! `tapa/backend/report/xilinx/rtl/generator.py::ReportDirUtil` (the
//! `REPORT_UTIL_COMMANDS` TCL template plus the hdl-dir / rpt-path
//! staging logic).
//!
//! Given the work directory layout produced by `generate_rtl_tree`
//! (`<work_dir>/rtl/*.v`) and the per-task C++ sources written by
//! `extract_hls_sources` (`<work_dir>/cpp/<task>.cpp`), for each unique
//! child task of the top task this module:
//!
//!   1. Consults the mtime of `<work_dir>/report/<task>.hier.util.rpt`
//!      and skips the re-synth when the report is newer than the
//!      matching `<work_dir>/cpp/<task>.cpp` (Python parity with the
//!      `os.path.getmtime(...) > rpt_mtime` guard).
//!   2. Otherwise builds an out-of-context `synth_design` TCL, drives
//!      it through [`run_vivado`], and requires that the `.rpt` now
//!      exists and is strictly newer than it was before.
//!   3. Parses the hierarchical utilization `.rpt` via
//!      [`parse_utilization_rpt`] and updates the task's `total_area`
//!      dict with the Python formula:
//!      `BRAM_18K = RAMB36*2 + RAMB18`, `DSP = "DSP Blocks"`,
//!      `FF = FFs`, `LUT = "Total LUTs"`, `URAM = URAM`.
//!
//! Serial execution only; the Python `jobs` flag is accepted for API
//! parity but currently unused. A rayon-based fan-out is a follow-up.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use tapa_task_graph::Design;
use tapa_xilinx::{parse_utilization_rpt, run_vivado, ToolRunner, UtilizationReport, VivadoJob};

use crate::error::{CliError, Result};

use super::cpp_extract::cpp_path_for;

/// Ports `tapa/backend/report/xilinx/rtl/generator.py::REPORT_UTIL_COMMANDS`.
///
/// The Python template uses a Python-side `{part_num}` / `{synth_args}`
/// / `{report_util_args}` / `{set_parallel}` substitution before
/// feeding Vivado, and passes `hdl_dir` / `rpt_file` in as `argv[0]` /
/// `argv[1]`. We do the same here — the `{...}` placeholders below are
/// replaced before the string hits Vivado; all other `{...}` pairs in
/// the TCL itself are escaped as `{{...}}` in the Python source.
const REPORT_UTIL_TCL: &str = "\
set hdl_dir [lindex $argv 0]
set rpt_file [lindex $argv 1]
set_param general.maxThreads 1
set_part {part_num}
read_verilog [ glob $hdl_dir/*.v ]
set ips [ glob -nocomplain $hdl_dir/*/*.xci ]
if { $ips ne \"\" } {
  import_ip $ips
  upgrade_ip [get_ips *]
  generate_target synthesis [ get_files *.xci ]
}
foreach tcl_file [glob -nocomplain $hdl_dir/*.tcl] {
  source $tcl_file
}
synth_design {synth_args}
opt_design
report_utilization -file $rpt_file {report_util_args}
";

/// Drive per-task out-of-context Vivado synth against `<work_dir>/rtl`
/// and fold the hierarchical utilization result into each task's
/// `total_area` dict on `design`. Ports
/// `ProgramSynthesisMixin.generate_post_synth_util`.
pub(super) fn emit_post_synth_util(
    work_dir: &Path,
    design: &mut Design,
    part_num: &str,
    _jobs: Option<u32>,
    runner: &dyn ToolRunner,
) -> Result<()> {
    let rtl_dir = work_dir.join("rtl");
    let report_dir = work_dir.join("report");
    fs::create_dir_all(&report_dir)?;

    let module_names: Vec<String> = top_task_child_names(design);
    for module_name in &module_names {
        let rpt_path = post_syn_rpt_path(work_dir, module_name);
        let cpp_path = cpp_path_for(work_dir, module_name);
        let prev_mtime = optional_mtime(&rpt_path);

        if should_run_vivado(&cpp_path, prev_mtime) {
            run_one(runner, &rtl_dir, &rpt_path, module_name, part_num)?;
            if !report_is_fresh(&rpt_path, prev_mtime) {
                return Err(CliError::InvalidArg(format!(
                    "post-synth util: Vivado returned success but the \
                     utilization report for `{module_name}` was not \
                     (re)written at {}",
                    rpt_path.display(),
                )));
            }
        }

        let text = fs::read_to_string(&rpt_path)?;
        let util = parse_utilization_rpt(&text)?;
        apply_total_area(design, &util);
    }
    Ok(())
}

/// Child-task names of the top task. Mirrors Python's
/// `{x.task.name for x in self.top_task.instances}` — the unique set of
/// instantiated task names directly under `design.top`.
///
/// Uses `IndexMap` insertion order so the iteration is deterministic;
/// Python's `set` is unordered but `ThreadPoolExecutor.map` doesn't
/// depend on iteration order — neither does the fold since each task's
/// `total_area` is written independently.
fn top_task_child_names(design: &Design) -> Vec<String> {
    design
        .tasks
        .get(&design.top)
        .map(|t| t.tasks.keys().cloned().collect())
        .unwrap_or_default()
}

/// `<work_dir>/report/<module>.hier.util.rpt` — ports
/// `ProgramDirectoryMixin.get_post_syn_rpt_path`.
fn post_syn_rpt_path(work_dir: &Path, module_name: &str) -> PathBuf {
    work_dir
        .join("report")
        .join(format!("{module_name}.hier.util.rpt"))
}

fn optional_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Python parity: re-run Vivado if the C++ source is strictly newer
/// than the cached report. When either mtime is unreadable we err on
/// the side of running — matching Python's `os.path.getmtime(...) >
/// rpt_path_mtime` with `rpt_path_mtime = 0.0` when the file is absent.
fn should_run_vivado(cpp_path: &Path, rpt_mtime: Option<SystemTime>) -> bool {
    let Ok(cpp_meta) = fs::metadata(cpp_path) else { return true };
    let Ok(cpp_mtime) = cpp_meta.modified() else { return true };
    match rpt_mtime {
        None => true,
        Some(prev) => cpp_mtime > prev,
    }
}

/// After Vivado returns success, the report must exist and be strictly
/// newer than it was before the run. Python raises `ValueError` on
/// failure; we surface the same condition as an `InvalidArg` caller-side.
fn report_is_fresh(rpt_path: &Path, prev_mtime: Option<SystemTime>) -> bool {
    let Some(new_mtime) = optional_mtime(rpt_path) else { return false };
    match prev_mtime {
        None => true,
        Some(prev) => new_mtime > prev,
    }
}

fn run_one(
    runner: &dyn ToolRunner,
    rtl_dir: &Path,
    rpt_path: &Path,
    module_name: &str,
    part_num: &str,
) -> Result<()> {
    if let Some(parent) = rpt_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let abs_hdl = fs::canonicalize(rtl_dir).unwrap_or_else(|_| rtl_dir.to_path_buf());
    let abs_rpt = if rpt_path.is_absolute() {
        rpt_path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(rpt_path),
            Err(_) => rpt_path.to_path_buf(),
        }
    };

    let tcl = build_report_util_tcl(module_name, part_num);
    let mut job = VivadoJob::new(tcl);
    job.tclargs = vec![
        abs_hdl.display().to_string(),
        abs_rpt.display().to_string(),
    ];
    job.uploads = vec![abs_hdl];
    if let Some(parent) = abs_rpt.parent() {
        job.downloads = vec![parent.to_path_buf()];
    }
    run_vivado(runner, &job)?;
    Ok(())
}

/// Format the `synth_args` / `report_util_args` / `part_num` into the
/// `REPORT_UTIL_TCL` template. Python always passes
/// `synth_kwargs={"mode": "out_of_context"}` and lets `ReportDirUtil`
/// append `-top <module> -part <part>`, plus the
/// `report_util_kwargs.setdefault("hierarchical", "")` for the utilization
/// report — so the rendered arg strings are:
///
/// ```text
/// -mode out_of_context -top <module> -part <part_num>
/// -hierarchical
/// ```
#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "{part_num}/{synth_args}/{report_util_args} are literal TCL template placeholders, not format-args"
)]
fn build_report_util_tcl(module_name: &str, part_num: &str) -> String {
    let synth_args =
        format!("-mode out_of_context -top {module_name} -part {part_num}");
    let report_util_args = "-hierarchical";
    REPORT_UTIL_TCL
        .replace("{part_num}", part_num)
        .replace("{synth_args}", &synth_args)
        .replace("{report_util_args}", report_util_args)
}

/// Apply the Python total-area formula to `design.tasks[instance]`:
///
/// - `BRAM_18K = RAMB36 * 2 + RAMB18`
/// - `DSP      = "DSP Blocks"`
/// - `FF       = FFs`
/// - `LUT      = "Total LUTs"`
/// - `URAM     = URAM`
///
/// Missing / non-integer cells fall through as `0` to match Python's
/// eventual `int(utilization[...])` — which would itself raise, so the
/// permissive fallback surfaces a well-formed dict instead of aborting
/// the whole synth. If the instance from the report doesn't match any
/// task in `design.tasks` we silently skip it (Python would raise
/// `KeyError`; in practice the hierarchical report's top row is always
/// the `-top` module we passed in, i.e. the task name).
fn apply_total_area(design: &mut Design, util: &UtilizationReport) {
    let Some(task) = design.tasks.get_mut(&util.instance) else { return };
    let ramb36 = get_metric_int(util, "RAMB36");
    let ramb18 = get_metric_int(util, "RAMB18");
    let bram = ramb36.saturating_mul(2).saturating_add(ramb18);
    let dsp = get_metric_int(util, "DSP Blocks");
    let ff = get_metric_int(util, "FFs");
    let lut = get_metric_int(util, "Total LUTs");
    let uram = get_metric_int(util, "URAM");

    task.total_area.clear();
    task.total_area.insert("BRAM_18K".to_string(), Value::from(bram));
    task.total_area.insert("DSP".to_string(), Value::from(dsp));
    task.total_area.insert("FF".to_string(), Value::from(ff));
    task.total_area.insert("LUT".to_string(), Value::from(lut));
    task.total_area.insert("URAM".to_string(), Value::from(uram));
}

fn get_metric_int(util: &UtilizationReport, key: &str) -> i64 {
    util.metrics
        .get(key)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use indexmap::IndexMap;
    use serde_json::json;
    use tapa_task_graph::TaskTopology;
    use tapa_xilinx::{MockToolRunner, ToolOutput};

    fn sample_rpt(instance: &str) -> String {
        format!(
            "Hierarchical Utilization Report\n\
             | Device : xcu250\n\
             +------+-------+-------+-------------+-----+------+------+\n\
             | Instance | Total LUTs | FFs | DSP Blocks | URAM | RAMB36 | RAMB18 |\n\
             +------+-------+-------+-------------+-----+------+------+\n\
             | {instance} | 100 | 200 | 3 | 1 | 4 | 5 |\n\
             +------+-------+-------+-------------+-----+------+------+\n"
        )
    }

    fn vadd_design() -> Design {
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
            .insert("Add".to_string(), json!([{"args": {}, "step": 0}]));
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
        Design {
            top: "VecAdd".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        }
    }

    fn setup_work_dir(dir: &Path, cpp_contents: &[(&str, &str)]) {
        fs::create_dir_all(dir.join("cpp")).expect("mkdir cpp");
        fs::create_dir_all(dir.join("rtl")).expect("mkdir rtl");
        for (name, body) in cpp_contents {
            fs::write(dir.join("cpp").join(format!("{name}.cpp")), body)
                .expect("write cpp");
        }
    }

    /// Canned Vivado run: writes `sample_rpt(instance)` to the
    /// expected download path so the post-run freshness check passes.
    #[test]
    fn post_synth_util_updates_total_area() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        setup_work_dir(work, &[("Add", "void Add() {}\n")]);
        let mut design = vadd_design();

        let runner = MockToolRunner::new();
        runner.push_ok("vivado", ToolOutput::default());
        let rpt_path = work.join("report").join("Add.hier.util.rpt");
        runner.attach_download(
            rpt_path.clone(),
            sample_rpt("Add").into_bytes(),
        );

        emit_post_synth_util(work, &mut design, "xcu250-figd2104-2L-e", None, &runner)
            .expect("emit_post_synth_util");

        let add = design.tasks.get("Add").expect("Add task present");
        assert_eq!(add.total_area.get("LUT"), Some(&json!(100)));
        assert_eq!(add.total_area.get("FF"), Some(&json!(200)));
        assert_eq!(add.total_area.get("DSP"), Some(&json!(3)));
        assert_eq!(add.total_area.get("URAM"), Some(&json!(1)));
        // BRAM_18K = RAMB36*2 + RAMB18 = 4*2 + 5 = 13
        assert_eq!(add.total_area.get("BRAM_18K"), Some(&json!(13)));

        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "exactly one Vivado run expected");
        assert_eq!(calls[0].program, "vivado");
        assert!(
            rpt_path.is_file(),
            "mock download should have staged the rpt on disk",
        );
    }

    /// When the report mtime is newer than the .cpp source, the run is
    /// skipped — so a runner with no queued responses would error out
    /// if the call happened, proving the skip.
    #[test]
    fn post_synth_util_skips_stale_report() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        setup_work_dir(work, &[("Add", "void Add() {}\n")]);
        let mut design = vadd_design();

        // Seed the rpt first, then bump the cpp mtime into the past so
        // the rpt is strictly newer. On systems where touch granularity
        // is coarse, we also sleep a tick.
        fs::create_dir_all(work.join("report")).expect("mkdir report");
        let rpt_path = work.join("report").join("Add.hier.util.rpt");
        let cpp_path = work.join("cpp").join("Add.cpp");
        // Re-stamp cpp to an old time, then touch the rpt to now.
        std::thread::sleep(Duration::from_millis(10));
        fs::write(&rpt_path, sample_rpt("Add")).expect("seed rpt");
        // Verify ordering: rpt must be strictly newer than cpp.
        let cpp_mtime = fs::metadata(&cpp_path).and_then(|m| m.modified()).unwrap();
        let rpt_mtime = fs::metadata(&rpt_path).and_then(|m| m.modified()).unwrap();
        assert!(
            rpt_mtime > cpp_mtime,
            "seed invariant: rpt must be newer than cpp for skip test",
        );

        // MockToolRunner with no queued responses: any `.run(...)` call
        // surfaces a `ToolFailure`, so a pass here proves the skip.
        let runner = MockToolRunner::new();
        emit_post_synth_util(work, &mut design, "xcu250-figd2104-2L-e", None, &runner)
            .expect("stale-report skip path must succeed");

        assert!(runner.calls().is_empty(), "Vivado must not be invoked");
        // But the rpt is still parsed and applied.
        let add = design.tasks.get("Add").expect("Add task");
        assert_eq!(add.total_area.get("LUT"), Some(&json!(100)));
    }

    #[test]
    fn build_report_util_tcl_substitutes_placeholders() {
        let tcl = build_report_util_tcl("Add", "xcvu37p-fsvh2892-2L-e");
        assert!(tcl.contains("set_part xcvu37p-fsvh2892-2L-e"));
        assert!(tcl.contains("-mode out_of_context -top Add -part xcvu37p-fsvh2892-2L-e"));
        assert!(tcl.contains("report_utilization -file $rpt_file -hierarchical"));
        // No leftover Python-style placeholders.
        assert!(!tcl.contains("{part_num}"));
        assert!(!tcl.contains("{synth_args}"));
        assert!(!tcl.contains("{report_util_args}"));
    }

    #[test]
    fn top_task_child_names_covers_direct_children() {
        let design = vadd_design();
        let names = top_task_child_names(&design);
        assert_eq!(names, vec!["Add".to_string()]);
    }
}
