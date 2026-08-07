//! Per-task out-of-context Vivado synth and hierarchical utilization
//! parsing.
//!
//! Given the work directory layout produced by `generate_rtl_tree`
//! (`<work_dir>/rtl/*.v`) and the per-task C++ sources written by
//! `extract_hls_sources` (`<work_dir>/cpp/<task>.cpp`), for each unique
//! child task of the top task this module:
//!
//!   1. Consults the mtime of `<work_dir>/report/<task>.hier.util.rpt`
//!      and skips the re-synth when the report is newer than the
//!      matching `<work_dir>/cpp/<task>.cpp`.
//!   2. Otherwise builds an out-of-context `synth_design` TCL, drives
//!      it through [`run_vivado`], and requires that the `.rpt` now
//!      exists and is strictly newer than it was before.
//!   3. Parses the hierarchical utilization `.rpt` via
//!      [`parse_utilization_rpt`] and updates the task's `total_area`
//!      dict with the formula:
//!      `BRAM_18K = RAMB36*2 + RAMB18`, `DSP = "DSP Blocks"`,
//!      `FF = FFs`, `LUT = "Total LUTs"`, `URAM = URAM`.
//!
//! Independent Vivado runs honor the requested `jobs` limit; parsed
//! results are folded into the design in task order.

use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tapa_ir::{Area, Design};
use tapa_xilinx::{parse_utilization_rpt, run_vivado, ToolRunner, UtilizationReport, VivadoJob};

use crate::error::{CliError, Result};

use super::resolve_worker_count;

use super::cpp_extract::cpp_path_for;

/// Render the report-utilization TCL. The template substitutes
/// `{part_num}`, `{synth_args}`, and `{report_util_args}` before
/// invocation; literal TCL braces are escaped in the template source.
fn render_report_util_tcl(part_num: &str, synth_args: &str, report_util_args: &str) -> String {
    crate::util::render_template(
        "report_util",
        include_str!("templates/report_util.tcl.j2"),
        minijinja::context! {
            part_num,
            synth_args,
            report_util_args,
        },
    )
}

/// Drive per-task out-of-context Vivado synthesis against
/// `<work_dir>/rtl` and fold each hierarchical utilization result into
/// the corresponding task's `total_area` map.
pub(super) fn emit_post_synth_util(
    work_dir: &Path,
    design: &mut Design,
    part_num: &str,
    jobs: Option<u32>,
    runner: &dyn ToolRunner,
) -> Result<()> {
    let rtl_dir = work_dir.join("rtl");
    let report_dir = work_dir.join("report");
    fs::create_dir_all(&report_dir)?;

    let module_names: Vec<String> = top_task_child_names(design);
    if module_names.is_empty() {
        return Ok(());
    }
    let worker_count = resolve_worker_count(jobs, module_names.len());
    let results: Vec<Result<UtilizationReport>> = crate::util::run_in_pool(
        worker_count,
        "post-synth utilization",
        CliError::Codegen,
        || {
            module_names
                .par_iter()
                .map(|module_name| {
                    run_and_parse_one(runner, work_dir, &rtl_dir, module_name, part_num)
                })
                .collect()
        },
    )?;

    // Indexed parallel iteration preserves `module_names` order.
    // Fold only after all workers finish so design mutation and error
    // selection remain deterministic regardless of completion order.
    for result in results {
        apply_total_area(design, &result?);
    }
    Ok(())
}

fn run_and_parse_one(
    runner: &dyn ToolRunner,
    work_dir: &Path,
    rtl_dir: &Path,
    module_name: &str,
    part_num: &str,
) -> Result<UtilizationReport> {
    let rpt_path = post_syn_rpt_path(work_dir, module_name);
    let cpp_path = cpp_path_for(work_dir, module_name);
    let prev_mtime = optional_mtime(&rpt_path);

    if should_run_vivado(&cpp_path, prev_mtime) {
        run_one(runner, rtl_dir, &rpt_path, module_name, part_num)?;
        if !report_is_fresh(&rpt_path, prev_mtime) {
            return Err(CliError::Codegen(format!(
                "post-synth util: Vivado returned success but the \
                 utilization report for `{module_name}` was not \
                 (re)written at {}",
                rpt_path.display(),
            )));
        }
    }

    let text = fs::read_to_string(&rpt_path)?;
    parse_utilization_rpt(&text).map_err(CliError::from)
}

/// Child-task names of the top task — the unique set of
/// instantiated task names directly under `design.top`.
///
/// Uses `BTreeMap` (alphabetical) order so iteration is deterministic.
fn top_task_child_names(design: &Design) -> Vec<String> {
    design
        .tasks
        .get(&design.top)
        .map(|t| t.tasks.keys().cloned().collect())
        .unwrap_or_default()
}

/// Return `<work_dir>/report/<module>.hier.util.rpt`.
fn post_syn_rpt_path(work_dir: &Path, module_name: &str) -> PathBuf {
    work_dir
        .join("report")
        .join(format!("{module_name}.hier.util.rpt"))
}

fn optional_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Re-run Vivado if the C++ source is strictly newer than the cached
/// report. When either mtime is unreadable we err on the side of
/// running (a missing report counts as infinitely old).
fn should_run_vivado(cpp_path: &Path, rpt_mtime: Option<SystemTime>) -> bool {
    let Ok(cpp_meta) = fs::metadata(cpp_path) else {
        return true;
    };
    let Ok(cpp_mtime) = cpp_meta.modified() else {
        return true;
    };
    match rpt_mtime {
        None => true,
        Some(prev) => cpp_mtime > prev,
    }
}

/// After Vivado returns success, require the report to exist and be
/// strictly newer than it was before the run.
fn report_is_fresh(rpt_path: &Path, prev_mtime: Option<SystemTime>) -> bool {
    let Some(new_mtime) = optional_mtime(rpt_path) else {
        return false;
    };
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
    let abs_hdl =
        crate::util::utf8(fs::canonicalize(rtl_dir).unwrap_or_else(|_| rtl_dir.to_path_buf()));
    let abs_rpt = if rpt_path.is_absolute() {
        crate::util::utf8(rpt_path)
    } else {
        match std::env::current_dir() {
            Ok(cwd) => crate::util::utf8(cwd.join(rpt_path)),
            Err(_) => crate::util::utf8(rpt_path),
        }
    };

    let tcl = build_report_util_tcl(module_name, part_num);
    let mut job = VivadoJob::new(tcl);
    job.tclargs = vec![abs_hdl.as_str().to_string(), abs_rpt.as_str().to_string()];
    job.uploads = vec![abs_hdl];
    if let Some(parent) = abs_rpt.parent() {
        job.downloads = vec![parent.to_path_buf()];
    }
    run_vivado(runner, &job)?;
    Ok(())
}

/// Format the out-of-context synthesis and hierarchical utilization
/// arguments into the report-utilization TCL template:
///
/// ```text
/// -mode out_of_context -top <module> -part <part_num>
/// -hierarchical
/// ```
fn build_report_util_tcl(module_name: &str, part_num: &str) -> String {
    let synth_args = format!("-mode out_of_context -top {module_name} -part {part_num}");
    let report_util_args = "-hierarchical";
    render_report_util_tcl(part_num, &synth_args, report_util_args)
}

/// Apply the total-area formula to `design.tasks[instance]`:
///
/// - `BRAM_18K = RAMB36 * 2 + RAMB18`
/// - `DSP      = "DSP Blocks"`
/// - `FF       = FFs`
/// - `LUT      = "Total LUTs"`
/// - `URAM     = URAM`
///
/// Missing or non-integer cells become `0`, keeping the area map
/// well-formed. An instance absent from `design.tasks` is ignored; the
/// report's top row normally names the `-top` module passed to Vivado.
fn apply_total_area(design: &mut Design, util: &UtilizationReport) {
    let Some(task) = design.tasks.get_mut(&util.instance) else {
        return;
    };
    let ramb36 = get_metric_int(util, "RAMB36");
    let ramb18 = get_metric_int(util, "RAMB18");
    let bram = ramb36.saturating_mul(2).saturating_add(ramb18);
    let dsp = get_metric_int(util, "DSP Blocks");
    let ff = get_metric_int(util, "FFs");
    let lut = get_metric_int(util, "Total LUTs");
    let uram = get_metric_int(util, "URAM");

    task.total_area = Some(Area {
        lut: lut.unsigned_abs(),
        ff: ff.unsigned_abs(),
        bram_18k: bram.unsigned_abs(),
        dsp: dsp.unsigned_abs(),
        uram: uram.unsigned_abs(),
    });
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
    use tapa_ir::ClockPeriod;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use std::collections::BTreeMap;

    use tapa_ir::{SynthTarget, Task, TaskInstance, TaskLevel};
    use tapa_xilinx::{MockToolRunner, ToolInvocation, ToolOutput, XilinxError};

    fn single_instance() -> Vec<TaskInstance> {
        vec![TaskInstance {
            name: None,
            args: BTreeMap::new(),
            step: 0,
        }]
    }

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
                self_area: None,
                total_area: None,
                clock_period: None,
            },
        );
        let mut child_tasks = BTreeMap::new();
        child_tasks.insert("Add".to_string(), single_instance());
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
                self_area: None,
                total_area: None,
                clock_period: Some(ClockPeriod::from_picoseconds(3330)),
            },
        );
        Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "VecAdd".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        }
    }

    fn two_child_design() -> Design {
        let mut design = vadd_design();
        let mut mul = design.tasks["Add"].clone();
        mul.code = "void Mul() {}\n".to_string();
        design.tasks.insert("Mul".to_string(), mul);
        design
            .tasks
            .get_mut("VecAdd")
            .expect("VecAdd present")
            .tasks
            .insert("Mul".to_string(), single_instance());
        design
    }

    struct ConcurrentVivadoRunner {
        expected: usize,
        active: AtomicUsize,
        max_active: AtomicUsize,
        entered: Mutex<usize>,
        entered_cv: Condvar,
    }

    impl ConcurrentVivadoRunner {
        fn new(expected: usize) -> Self {
            Self {
                expected,
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                entered: Mutex::new(0),
                entered_cv: Condvar::new(),
            }
        }
    }

    impl ToolRunner for ConcurrentVivadoRunner {
        fn run(&self, inv: &ToolInvocation) -> tapa_xilinx::Result<ToolOutput> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            {
                let mut entered = self.entered.lock().unwrap();
                *entered += 1;
                self.entered_cv.notify_all();
                while *entered < self.expected {
                    let (next, timeout) = self
                        .entered_cv
                        .wait_timeout(entered, Duration::from_secs(1))
                        .unwrap();
                    entered = next;
                    if timeout.timed_out() {
                        break;
                    }
                }
            }

            let rpt_path = inv.args.last().ok_or_else(|| XilinxError::ToolFailure {
                program: inv.program.clone(),
                code: -1,
                stderr: "missing report path argument".to_string(),
            })?;
            let instance = Path::new(rpt_path)
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.split('.').next())
                .ok_or_else(|| XilinxError::ToolFailure {
                    program: inv.program.clone(),
                    code: -1,
                    stderr: format!("invalid report path: {rpt_path}"),
                })?;
            fs::write(rpt_path, sample_rpt(instance))?;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::default())
        }
    }

    fn setup_work_dir(dir: &Path, cpp_contents: &[(&str, &str)]) {
        fs::create_dir_all(dir.join("cpp")).expect("mkdir cpp");
        fs::create_dir_all(dir.join("rtl")).expect("mkdir rtl");
        for (name, body) in cpp_contents {
            fs::write(dir.join("cpp").join(format!("{name}.cpp")), body).expect("write cpp");
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
            crate::util::utf8(rpt_path.clone()),
            sample_rpt("Add").into_bytes(),
        );

        emit_post_synth_util(work, &mut design, "xcu250-figd2104-2L-e", None, &runner)
            .expect("emit_post_synth_util");

        let add = design.tasks.get("Add").expect("Add task present");
        assert_eq!(add.total_area.expect("total area").lut, 100);
        assert_eq!(add.total_area.expect("total area").ff, 200);
        assert_eq!(add.total_area.expect("total area").dsp, 3);
        assert_eq!(add.total_area.expect("total area").uram, 1);
        // BRAM_18K = RAMB36*2 + RAMB18 = 4*2 + 5 = 13
        assert_eq!(add.total_area.expect("total area").bram_18k, 13);

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
        assert_eq!(add.total_area.expect("total area").lut, 100);
    }

    #[test]
    fn post_synth_util_honors_parallel_jobs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        setup_work_dir(
            work,
            &[("Add", "void Add() {}\n"), ("Mul", "void Mul() {}\n")],
        );
        let mut design = two_child_design();
        let runner = ConcurrentVivadoRunner::new(2);

        emit_post_synth_util(work, &mut design, "xcu250-figd2104-2L-e", Some(2), &runner)
            .expect("parallel post-synth utilization");

        assert_eq!(
            runner.max_active.load(Ordering::SeqCst),
            2,
            "--jobs=2 must allow two concurrent Vivado runs"
        );
        for task in ["Add", "Mul"] {
            assert_eq!(
                design.tasks[task].total_area.expect("total area").lut,
                100,
                "result for {task} must be folded into the matching task"
            );
        }
    }

    #[test]
    fn post_synth_worker_count_honors_limit_and_task_count() {
        assert_eq!(resolve_worker_count(Some(1), 4), 1);
        assert_eq!(resolve_worker_count(Some(2), 4), 2);
        assert_eq!(resolve_worker_count(Some(8), 3), 3);
        assert_eq!(
            resolve_worker_count(Some(0), 3),
            resolve_worker_count(None, 3)
        );
    }

    #[test]
    fn build_report_util_tcl_substitutes_placeholders() {
        let tcl = build_report_util_tcl("Add", "xcvu37p-fsvh2892-2L-e");
        assert!(tcl.contains("set_part xcvu37p-fsvh2892-2L-e"));
        assert!(tcl.contains("-mode out_of_context -top Add -part xcvu37p-fsvh2892-2L-e"));
        assert!(tcl.contains("report_utilization -file $rpt_file -hierarchical"));
        // No leftover placeholders.
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
