//! Per-task Vitis HLS invocation with parallel dispatch and
//! mtime-based skipping.

use camino::Utf8PathBuf;
use rayon::prelude::*;
use std::fs;
use std::path::Path;

use tapa_ir::{Design, SynthTarget};
use tapa_xilinx::{run_hls_with_retry, run_hls_with_retry_in_stage, HlsJob, HlsOutput, ToolRunner};

use crate::error::{CliError, Result};
use crate::steps::synth::cpp_extract::cpp_path_for;

/// Up to 3 attempts total. Vitis HLS occasionally fails with a
/// transient `Pre-synthesis failed.` diagnostic that re-runs clean,
/// so the retry wrapper keys off that substring.
const HLS_MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub struct TaskHlsLayout {
    pub reports_dir: Utf8PathBuf,
    pub hdl_dir: Utf8PathBuf,
}

impl TaskHlsLayout {
    pub fn new(work_dir: &Path, task_name: &str) -> Self {
        let base = work_dir.join("hls").join(task_name);
        Self {
            reports_dir: Utf8PathBuf::from_path_buf(base.join("report"))
                .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())),
            hdl_dir: Utf8PathBuf::from_path_buf(base.join("verilog"))
                .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlsRunOptions {
    pub part_num: String,
    pub clock_period: String,
    pub other_configs: String,
    pub cflags: Vec<String>,
    pub skip_based_on_mtime: bool,
    /// Number of HLS runs executed in parallel. `None` or 1 → serial.
    pub jobs: Option<u32>,
    /// When `--keep-hls-work-dir` is set,
    /// `run_hls` stages under `<work_dir>/hls/<task>/project` (kept
    /// on disk) instead of a tempdir so the Vitis project + logs
    /// survive after a failure.
    pub keep_work_dir: bool,
}

/// Run HLS for every task that targets HLS — **all** tasks
/// (not just leaves) — the upper-task shell is needed by codegen so
/// the parent module's port surface is parseable. Tasks whose
/// `target == "ignore"` are skipped (promotes them to
/// `gen_templates`).
pub fn run_hls_for_leaves(
    runner: &dyn ToolRunner,
    work_dir: &Path,
    design: &Design,
    options: &HlsRunOptions,
) -> Result<Vec<(String, TaskHlsLayout, HlsOutput)>> {
    // Plan pass: enumerate every task that needs HLS, resolve its
    // layout + cpp source, and either record a cache-hit short-circuit
    // or a live Vitis job. This keeps the parallel loop straightforward
    // (just dispatch the live jobs) and preserves the original output
    // order even when jobs run out-of-order.
    let mut plan: Vec<(String, TaskHlsLayout, Work)> = Vec::new();
    for (task_name, task) in &design.tasks {
        if task.synth == SynthTarget::Ignore {
            continue;
        }
        let layout = TaskHlsLayout::new(work_dir, task_name);

        let cpp_source = cpp_path_for(work_dir, task_name);
        let cpp_source = Utf8PathBuf::from_path_buf(cpp_source)
            .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
        if !cpp_source.is_file() {
            return Err(CliError::InvalidArg(format!(
                "missing extracted C++ source `{}` for task `{task_name}`",
                cpp_source.as_str(),
            )));
        }

        // Check freshness before creating `layout.hdl_dir`, and require
        // at least one `.v` file so an empty directory is not a cache
        // hit.
        if options.skip_based_on_mtime && layout.hdl_dir.is_dir() {
            let verilog_files = list_verilog_files(&layout.hdl_dir)?;
            if hdl_files_are_newer_than(&verilog_files, &cpp_source) {
                log::info!(
                    "skipping HLS for `{task_name}` (mtime cache hit at {})",
                    layout.hdl_dir.as_str(),
                );
                // `reports_dir` must still exist for downstream
                // readers; the skip path does not touch `hdl_dir`.
                fs::create_dir_all(&layout.reports_dir)?;
                // Reload existing csynth data so downstream report
                // generation and design.json keep correct metrics.
                let csynth =
                    find_and_parse_csynth(&layout.reports_dir, task_name).unwrap_or_else(|e| {
                        log::warn!(
                            "could not reload cached csynth for `{task_name}`: {e}; using defaults"
                        );
                        tapa_xilinx::CsynthReport::default()
                    });
                let report_paths = walkdir::WalkDir::new(&layout.reports_dir)
                    .into_iter()
                    .filter_map(std::result::Result::ok)
                    .filter(|e| e.file_type().is_file())
                    .map(|e| {
                        Utf8PathBuf::from_path_buf(e.path().to_path_buf())
                            .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()))
                    })
                    .collect();
                plan.push((
                    task_name.clone(),
                    layout,
                    Work::Skip(HlsOutput {
                        csynth,
                        verilog_files,
                        report_paths,
                        stdout: String::new(),
                        stderr: String::new(),
                    }),
                ));
                continue;
            }
        }

        fs::create_dir_all(&layout.reports_dir)?;
        fs::create_dir_all(&layout.hdl_dir)?;

        let job = HlsJob::builder()
            .task_name(task_name.clone())
            .cpp_source(cpp_source)
            .cflags(options.cflags.clone())
            .target_part(options.part_num.clone())
            .top_name(task_name.clone())
            .clock_period(options.clock_period.clone())
            .reports_out_dir(layout.reports_dir.clone())
            .hdl_out_dir(layout.hdl_dir.clone())
            .other_configs(options.other_configs.clone())
            .build();

        // `--keep-hls-work-dir`: stage under
        // `<work_dir>/hls/<task>/project` so the Vitis project + logs
        // survive the run for post-mortem inspection. The retry
        // wrapper reuses that single dir across attempts (a
        // partially-failed `project/` may contaminate the next
        // attempt, but the operator opted in).
        //
        // Default path: hand the job off to `run_hls_with_retry`,
        // which allocates a *fresh* `tempfile::tempdir()` for every
        // attempt. Each transient `Pre-synthesis failed.` retry starts
        // from a clean project tree.
        let work = if options.keep_work_dir {
            let persistent =
                Utf8PathBuf::from_path_buf(work_dir.join("hls").join(task_name).join("project"))
                    .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
            // Clear any leftover from a previous run so the first
            // attempt doesn't trip Vitis's project-already-open logic.
            if persistent.exists() {
                let _ = fs::remove_dir_all(&persistent);
            }
            fs::create_dir_all(&persistent)?;
            Work::RunInStage(job, persistent)
        } else {
            Work::RunFresh(job)
        };

        plan.push((task_name.clone(), layout, work));
    }

    let worker_count = resolve_worker_count(options.jobs, &plan);
    let results: Vec<Result<Option<HlsOutput>>> = dispatch_plan(runner, &plan, worker_count);

    // No explicit cleanup: `RunFresh` lets `run_hls_with_retry` own
    // its per-attempt tempdir and drop it. `RunInStage` is kept on
    // disk intentionally under `<work_dir>/hls/<task>/project`.

    // Assemble output in the original plan order, surfacing the first
    // error.
    let mut out = Vec::with_capacity(plan.len());
    for ((task_name, layout, work), result) in plan.into_iter().zip(results) {
        let hls_out = match work {
            Work::Skip(pre) => pre,
            Work::RunInStage(..) | Work::RunFresh(_) => result?.expect("Run must yield Some"),
        };
        out.push((task_name, layout, hls_out));
    }
    Ok(out)
}

fn resolve_worker_count(jobs: Option<u32>, plan: &[(String, TaskHlsLayout, impl Sized)]) -> usize {
    // A positive `--jobs N` wins; zero retains the historical "use the
    // default" behavior. Cap by live work so we never spawn more workers
    // than jobs to dispatch.
    let desired = match jobs {
        None | Some(0) => default_hls_workers(),
        Some(jobs) => jobs as usize,
    };
    desired.min(plan.len().max(1))
}

/// Use the host's available logical parallelism as the default worker
/// count.
fn default_hls_workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

fn dispatch_plan(
    runner: &dyn ToolRunner,
    plan: &[(String, TaskHlsLayout, impl PlanEntry)],
    worker_count: usize,
) -> Vec<Result<Option<HlsOutput>>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count.max(1))
        .build()
        .expect("rayon thread pool builds");
    pool.install(|| {
        plan.par_iter()
            .map(|(_, _, work)| work.execute(runner))
            .collect()
    })
}

trait PlanEntry: Sync {
    fn execute(&self, runner: &dyn ToolRunner) -> Result<Option<HlsOutput>>;
}

impl PlanEntry for Work {
    fn execute(&self, runner: &dyn ToolRunner) -> Result<Option<HlsOutput>> {
        match self {
            Self::Skip(_) => Ok(None),
            Self::RunInStage(job, stage_dir) => {
                let out = run_hls_with_retry_in_stage(runner, job, HLS_MAX_ATTEMPTS, stage_dir)
                    .map_err(CliError::from)?;
                Ok(Some(out))
            }
            Self::RunFresh(job) => {
                // Each retry gets a fresh temporary staging directory.
                let out =
                    run_hls_with_retry(runner, job, HLS_MAX_ATTEMPTS).map_err(CliError::from)?;
                Ok(Some(out))
            }
        }
    }
}

/// Internal work state for the plan pass. Kept module-private —
/// `PlanEntry` is the caller-visible marker.
#[allow(
    clippy::large_enum_variant,
    reason = "Work is held briefly; \
    boxing adds allocations without removing the size difference \
    between the large `HlsJob + PathBuf` variant and the trivial Skip"
)]
enum Work {
    Skip(HlsOutput),
    /// `--keep-hls-work-dir`: persistent project under
    /// `<work_dir>/hls/<task>/project` reused across retries.
    RunInStage(HlsJob, Utf8PathBuf),
    /// Default: each retry attempt gets its own fresh tempdir so a
    /// partially-failed `project/` cannot contaminate the next try.
    RunFresh(HlsJob),
}

fn hdl_files_are_newer_than(verilog_files: &[Utf8PathBuf], cpp_source: &camino::Utf8Path) -> bool {
    if verilog_files.is_empty() {
        return false;
    }
    let Ok(cpp_meta) = fs::metadata(cpp_source) else {
        return false;
    };
    let Ok(cpp_t) = cpp_meta.modified() else {
        return false;
    };
    verilog_files.iter().all(|hdl| {
        fs::metadata(hdl)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|hdl_t| hdl_t > cpp_t)
    })
}

fn list_verilog_files(dir: &camino::Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for ent in walkdir::WalkDir::new(dir) {
        let ent = ent.map_err(|e| {
            CliError::InvalidArg(format!(
                "failed to inspect cached HDL directory `{}`: {e}",
                dir.as_str()
            ))
        })?;
        if ent.file_type().is_file() && ent.path().extension().and_then(|s| s.to_str()) == Some("v")
        {
            let p = ent.path().to_path_buf();
            out.push(
                Utf8PathBuf::from_path_buf(p)
                    .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())),
            );
        }
    }
    out.sort();
    Ok(out)
}

/// Reload and parse the task's top-level csynth report on an mtime-skip
/// cache hit.
///
/// Reads `<task>_csynth.xml` (falling back to `<task>.csynth.xml`), the
/// exact file the live HLS harvest parses. A task's report dir also holds
/// sub-module reports — e.g. `<task>_Pipeline_VITIS_LOOP_*_csynth.xml` —
/// which likewise end in `_csynth.xml` but carry only the sub-loop's
/// (smaller) area. Selecting the first `*_csynth.xml` in `read_dir` order
/// could therefore reload a sub-module's area, making the skip path
/// disagree with a full HLS run and breaking `.xo` reproducibility.
fn find_and_parse_csynth(
    reports_dir: &camino::Utf8Path,
    task_name: &str,
) -> Result<tapa_xilinx::CsynthReport> {
    let primary = reports_dir.join(format!("{task_name}_csynth.xml"));
    let fallback = reports_dir.join(format!("{task_name}.csynth.xml"));
    let report_xml = if primary.is_file() { primary } else { fallback };
    let bytes = fs::read(&report_xml).map_err(|e| {
        CliError::InvalidArg(format!(
            "missing cached csynth report `{}`: {e}",
            report_xml.as_str(),
        ))
    })?;
    tapa_xilinx::parse_csynth_xml(&bytes).map_err(|e| {
        CliError::InvalidArg(format!(
            "parse cached csynth `{}`: {e}",
            report_xml.as_str()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{Design, SynthTarget, Task, TaskLevel};
    use tapa_xilinx::{MockToolRunner, ToolInvocation, ToolOutput};

    fn leaf_design() -> Design {
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Add".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: String::new(),
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
        Design {
            top: "Add".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        }
    }

    #[test]
    fn worker_count_treats_zero_as_default_and_caps_at_live_work() {
        let plan = (0..3)
            .map(|idx| {
                (
                    format!("task_{idx}"),
                    TaskHlsLayout::new(Path::new("work"), &format!("task_{idx}")),
                    (),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            resolve_worker_count(Some(0), &plan),
            resolve_worker_count(None, &plan)
        );
        assert_eq!(resolve_worker_count(Some(1), &plan), 1);
        assert_eq!(resolve_worker_count(Some(8), &plan), 3);
    }

    /// A cache hit requires an existing HDL directory containing at
    /// least one Verilog file.
    #[test]
    fn fresh_hdl_dir_does_not_falsely_look_cached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();

        // Seed only `cpp/Add.cpp`; no `hls/Add/verilog/` at all.
        fs::create_dir_all(work.join("cpp")).unwrap();
        fs::write(work.join("cpp").join("Add.cpp"), b"int main(){}\n").unwrap();

        let design = leaf_design();
        // Mock runner that records a call → proves the skip branch was
        // NOT taken (otherwise Vitis HLS never runs).
        let runner = MockToolRunner::new();
        runner.push_ok("vitis_hls", ToolOutput::default());

        let opts = HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: "3.33".to_string(),
            other_configs: String::new(),
            cflags: Vec::new(),
            skip_based_on_mtime: true,
            jobs: Some(1),
            keep_work_dir: false,
        };

        // Ignore the run result (no csynth.xml staged, so harvest
        // fails) — what we care about is that the runner was called
        // at all, which proves the stale-skip bug is gone.
        let _ = run_hls_for_leaves(&runner, work, &design, &opts);
        let calls = runner.calls();
        assert_eq!(
            calls.len(),
            1,
            "fresh hdl_dir must not be treated as a cache hit; \
             runner should have been called exactly once, got: {calls:?}",
        );
        assert_eq!(
            calls[0].program, "vitis_hls",
            "the one call must be the Vitis HLS invocation",
        );
        let _ = ToolInvocation::default(); // silence unused import in some builds
    }

    /// Cache path still works: when `hdl_dir` already contains a `.v`
    /// file that is newer than the `.cpp`, the runner must skip HLS.
    #[test]
    fn populated_hdl_dir_honors_skip_based_on_mtime() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        fs::create_dir_all(work.join("cpp")).unwrap();
        fs::write(work.join("cpp").join("Add.cpp"), b"int main(){}\n").unwrap();

        // Pre-populate the HDL dir with a `.v` file; ensure its mtime
        // is strictly newer than the `.cpp`.
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();

        let design = leaf_design();
        // Runner with no queued responses: any call fails loudly.
        let runner = MockToolRunner::new();

        let opts = HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: "3.33".to_string(),
            other_configs: String::new(),
            cflags: Vec::new(),
            skip_based_on_mtime: true,
            jobs: Some(1),
            keep_work_dir: false,
        };
        let out =
            run_hls_for_leaves(&runner, work, &design, &opts).expect("cache hit path must succeed");
        assert_eq!(out.len(), 1);
        let (_, _, hls_out) = &out[0];
        assert!(
            !hls_out.verilog_files.is_empty(),
            "cache hit must carry the existing HDL files forward",
        );
        assert!(runner.calls().is_empty(), "cache hit must not call Vitis");
    }

    #[test]
    fn overwritten_hdl_file_refreshes_cache_without_touching_parent_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();
        let dir_mtime_before = fs::metadata(&hdl).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::create_dir_all(work.join("cpp")).unwrap();
        fs::write(work.join("cpp").join("Add.cpp"), b"int main(){}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); wire fresh; endmodule\n").unwrap();

        let dir_mtime_after = fs::metadata(&hdl).unwrap().modified().unwrap();
        assert_eq!(
            dir_mtime_after, dir_mtime_before,
            "overwriting an existing HDL file should leave the directory mtime unchanged"
        );

        let opts = HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: "3.33".to_string(),
            other_configs: String::new(),
            cflags: Vec::new(),
            skip_based_on_mtime: true,
            jobs: Some(1),
            keep_work_dir: false,
        };
        let runner = MockToolRunner::new();
        let out = run_hls_for_leaves(&runner, work, &leaf_design(), &opts)
            .expect("fresh emitted file must produce a cache hit");
        assert_eq!(out.len(), 1);
        assert!(
            runner.calls().is_empty(),
            "freshness must use the emitted file mtime, not its parent directory"
        );
    }

    /// A csynth.xml carrying a distinguishable `FF`/`BRAM_18K` so a test
    /// can tell which report was reloaded.
    fn csynth_xml(top: &str, ff: u32, bram: u32) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n<profile>\n\
             <UserAssignments><TopModelName>{top}</TopModelName>\
             <Part>xcu250</Part><TargetClockPeriod>3.33</TargetClockPeriod></UserAssignments>\n\
             <PerformanceEstimates><SummaryOfTimingAnalysis>\
             <EstimatedClockPeriod>2.431</EstimatedClockPeriod>\
             </SummaryOfTimingAnalysis></PerformanceEstimates>\n\
             <AreaEstimates><Resources>\
             <BRAM_18K>{bram}</BRAM_18K><FF>{ff}</FF><LUT>0</LUT>\
             </Resources></AreaEstimates>\n</profile>\n"
        )
    }

    /// The mtime-skip reload must read the task's own `<task>_csynth.xml`,
    /// not a sibling `<task>_Pipeline_*_csynth.xml` sub-module report that
    /// also ends in `_csynth.xml` but reports only the sub-loop's area.
    /// Picking the wrong one made the skip path disagree with a full HLS
    /// run and broke `.xo` reproducibility.
    #[test]
    fn reload_prefers_task_report_over_submodule_report() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reports = Utf8PathBuf::from_path_buf(tmp.path().join("report"))
            .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
        fs::create_dir_all(&reports).unwrap();
        // Sub-module report (smaller area) shares the `_csynth.xml` suffix.
        fs::write(
            reports.join("Mmap2Stream_Pipeline_VITIS_LOOP_27_1_csynth.xml"),
            csynth_xml("Mmap2Stream_Pipeline_VITIS_LOOP_27_1", 102, 0),
        )
        .unwrap();
        // Task top report — the one the live harvest parses.
        fs::write(
            reports.join("Mmap2Stream_csynth.xml"),
            csynth_xml("Mmap2Stream", 843, 1),
        )
        .unwrap();

        let report = find_and_parse_csynth(&reports, "Mmap2Stream").expect("reload task report");
        assert_eq!(report.top, "Mmap2Stream", "must load the task top report");
        assert_eq!(
            report.area.get("FF").map(String::as_str),
            Some("843"),
            "must read the task's own area, not the sub-loop's",
        );
        assert_eq!(report.area.get("BRAM_18K").map(String::as_str), Some("1"));
    }

    #[test]
    fn every_emitted_hdl_file_must_be_newer_than_cpp() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();
        fs::write(
            hdl.join("stale_helper.v"),
            b"module stale_helper(); endmodule\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::create_dir_all(work.join("cpp")).unwrap();
        fs::write(work.join("cpp").join("Add.cpp"), b"int main(){}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); wire fresh; endmodule\n").unwrap();

        let files = list_verilog_files(
            &Utf8PathBuf::from_path_buf(hdl)
                .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())),
        )
        .unwrap();
        let cpp = Utf8PathBuf::from_path_buf(work.join("cpp").join("Add.cpp"))
            .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
        assert!(
            !hdl_files_are_newer_than(&files, &cpp),
            "one stale emitted file must invalidate the HLS cache"
        );
    }
}
