//! Per-task Vitis HLS invocation with parallel dispatch and
//! mtime-based skipping.

use camino::Utf8PathBuf;
use rayon::prelude::*;
use std::fs;
use std::path::Path;

use tapa_ir::{ClockPeriod, Design, SynthTarget, Task};
use tapa_xilinx::{run_hls_with_retry, run_hls_with_retry_in_stage, HlsJob, HlsOutput, ToolRunner};

use crate::error::{CliError, Result};
use crate::steps::synth::hls_sources::TaskSources;

use super::resolve_worker_count;

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
            reports_dir: crate::util::utf8(base.join("report")),
            hdl_dir: crate::util::utf8(base.join("verilog")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlsRunOptions {
    pub part_num: String,
    pub clock_period: ClockPeriod,
    pub other_configs: String,
    /// Shared cflags tail (vendor/tapa includes + target defines,
    /// closing with `-I<tapa-extra-runtime-include>`). Per-task user
    /// and manifest flags are prepended by [`task_cflags`].
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
    sources: &TaskSources,
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

        // Non-empty for every task: `stage_hls_sources` rejects empty
        // manifests before any job is planned.
        let srcs = sources.srcs(task_name);

        // Check freshness before creating `layout.hdl_dir`, and require
        // at least one `.v` file so a directory containing only generator
        // sidecars is not a cache hit.
        if options.skip_based_on_mtime && layout.hdl_dir.is_dir() {
            let hdl_files = list_hdl_files(&layout.hdl_dir)?;
            if hdl_files.iter().any(|path| is_verilog(path))
                && hdl_files_are_newer_than(&hdl_files, srcs)
            {
                log::info!(
                    "skipping HLS for `{task_name}` (mtime cache hit at {})",
                    layout.hdl_dir.as_str(),
                );
                // `reports_dir` must still exist for downstream
                // readers; the skip path does not touch `hdl_dir`.
                fs::create_dir_all(&layout.reports_dir)?;
                // Reload existing csynth data so downstream report
                // generation and the persisted graph keep correct metrics.
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
                    .map(|e| crate::util::utf8(e.path()))
                    .collect();
                plan.push((
                    task_name.clone(),
                    layout,
                    Work::Skip(HlsOutput {
                        csynth,
                        verilog_files: hdl_files,
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
            .srcs(srcs.to_vec())
            .cflags(task_cflags(
                &design.cflags,
                &work_dir.join(crate::tapacc::REWRITTEN_DIR),
                task,
                &options.cflags,
            ))
            .target_part(options.part_num.clone())
            .top_name(task_name.clone())
            .clock_period(options.clock_period.to_string())
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
                crate::util::utf8(work_dir.join("hls").join(task_name).join("project"));
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

    let worker_count = resolve_worker_count(options.jobs, plan.len());
    let results: Vec<Result<Option<HlsOutput>>> = dispatch_plan(runner, &plan, worker_count)?;

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

fn dispatch_plan(
    runner: &dyn ToolRunner,
    plan: &[(String, TaskHlsLayout, Work)],
    worker_count: usize,
) -> Result<Vec<Result<Option<HlsOutput>>>> {
    crate::util::run_in_pool(worker_count.max(1), "HLS worker", CliError::Codegen, || {
        plan.par_iter()
            .map(|(_, _, work)| work.execute(runner))
            .collect()
    })
}

impl Work {
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

/// Internal work state for the plan pass. Kept module-private.
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

/// Per-task HLS cflags: the user's graph cflags, then the task
/// manifest's tree-relative pieces — the rewritten tree root first,
/// then one `-I` per include dir (the empty entry is the tree root
/// itself, already covered), then one `-D` per guard define — then
/// the shared vendor tail (`HlsRunOptions::cflags`). Staging has
/// already asserted the include dirs and defines are Tcl-safe.
///
/// The tail goes last on purpose: it ends with
/// `-I<tapa-extra-runtime-include>`, and Vitis 2025.2's unified flow
/// glues a `-cflags` token ending in the exact string `include` to
/// whatever follows it (losing both), so that flag must close the
/// string. Manifest include dirs must therefore not end in `include`
/// either: the external-bucket dirs the manifest emits would hit the same
/// glue.
fn task_cflags(user: &[String], tree_root: &Path, task: &Task, tail: &[String]) -> Vec<String> {
    let mut flags = user.to_vec();
    flags.push(format!("-I{}", tree_root.display()));
    for dir in &task.include_dirs {
        if !dir.is_empty() {
            flags.push(format!("-I{}", tree_root.join(dir).display()));
        }
    }
    for define in &task.defines {
        flags.push(format!("-D{define}"));
    }
    flags.extend_from_slice(tail);
    flags
}

fn hdl_files_are_newer_than(hdl_files: &[Utf8PathBuf], srcs: &[Utf8PathBuf]) -> bool {
    if hdl_files.is_empty() || srcs.is_empty() {
        return false;
    }
    // Every emitted HDL file must postdate the newest source file;
    // a metadata gap on any input conservatively reports stale.
    let mut newest_src: Option<std::time::SystemTime> = None;
    for src in srcs {
        let Ok(meta) = fs::metadata(src) else {
            return false;
        };
        let Ok(t) = meta.modified() else {
            return false;
        };
        newest_src = Some(newest_src.map_or(t, |prev: std::time::SystemTime| prev.max(t)));
    }
    let Some(newest_src) = newest_src else {
        return false;
    };
    hdl_files.iter().all(|hdl| {
        fs::metadata(hdl)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|hdl_t| hdl_t > newest_src)
    })
}

pub fn list_hdl_files(dir: &camino::Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for ent in walkdir::WalkDir::new(dir) {
        let ent = ent.map_err(|e| {
            CliError::Codegen(format!(
                "failed to inspect HDL directory `{}`: {e}",
                dir.as_str()
            ))
        })?;
        if ent.file_type().is_file() {
            let p = ent.path().to_path_buf();
            out.push(crate::util::utf8(p));
        }
    }
    out.sort();
    Ok(out)
}

fn is_verilog(path: &camino::Utf8Path) -> bool {
    path.extension() == Some("v")
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
        CliError::Codegen(format!(
            "missing cached csynth report `{}`: {e}",
            report_xml.as_str(),
        ))
    })?;
    tapa_xilinx::parse_csynth_xml(&bytes).map_err(|e| {
        CliError::Codegen(format!(
            "parse cached csynth `{}`: {e}",
            report_xml.as_str()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use tapa_ir::{Design, SynthTarget, Task, TaskLevel};
    use tapa_xilinx::{MockToolRunner, ToolInvocation, ToolOutput};

    /// Stage `design`'s manifest against `work`, mirroring what
    /// `run_native` does before the HLS plan pass.
    fn staged(work: &Path, design: &Design) -> TaskSources {
        crate::steps::synth::hls_sources::stage_hls_sources(work, design).expect("stage sources")
    }

    fn seed_rewritten(work: &Path) {
        let tree = work.join(crate::tapacc::REWRITTEN_DIR);
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("Add.cpp"), b"int main(){}\n").unwrap();
    }

    fn leaf_task(name: &str) -> Task {
        Task {
            level: TaskLevel::Lower,
            srcs: vec![format!("{name}.cpp")],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            ports: Vec::new(),
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            readable_name: String::new(),
            synth: SynthTarget::Hls,
            self_area: None,
            total_area: None,
            clock_period: None,
        }
    }

    fn leaf_design() -> Design {
        let mut tasks = BTreeMap::new();
        tasks.insert("Add".to_string(), leaf_task("Add"));
        Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Add".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        }
    }

    /// The manifest→cflags contract: user cflags, then
    /// tree root, one `-I` per include dir (empty = root, skipped),
    /// one `-D` per guard, then the shared tail — whose closing
    /// `-I<extra-runtime-include>` must remain the final token
    /// (Vitis glues tokens onto paths ending in `include`).
    #[test]
    fn task_cflags_compose_user_tree_includes_defines_then_tail() {
        let task = Task {
            include_dirs: vec![
                String::new(),
                "_external/ab12cd34".to_string(),
                "common".to_string(),
            ],
            defines: vec!["TAPA_TASK_DEF_Add".to_string()],
            ..leaf_task("Add")
        };
        let flags = task_cflags(
            &["-std=c++14".to_string(), "-I/app".to_string()],
            Path::new("/work/rewritten"),
            &task,
            &[
                "-isystem/tapa-lib".to_string(),
                "-I/extra-runtime-include".to_string(),
            ],
        );
        assert_eq!(
            flags,
            vec![
                "-std=c++14".to_string(),
                "-I/app".to_string(),
                "-I/work/rewritten".to_string(),
                "-I/work/rewritten/_external/ab12cd34".to_string(),
                "-I/work/rewritten/common".to_string(),
                "-DTAPA_TASK_DEF_Add".to_string(),
                "-isystem/tapa-lib".to_string(),
                "-I/extra-runtime-include".to_string(),
            ]
        );
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
            resolve_worker_count(Some(0), plan.len()),
            resolve_worker_count(None, plan.len())
        );
        assert_eq!(resolve_worker_count(Some(1), plan.len()), 1);
        assert_eq!(resolve_worker_count(Some(8), plan.len()), 3);
    }

    /// A cache hit requires an existing HDL directory containing at
    /// least one Verilog file.
    #[test]
    fn fresh_hdl_dir_does_not_falsely_look_cached() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();

        // Seed only the rewritten source; no `hls/Add/verilog/` at all.
        seed_rewritten(work);

        let design = leaf_design();
        // Mock runner that records a call → proves the skip branch was
        // NOT taken (otherwise Vitis HLS never runs).
        let runner = MockToolRunner::new();
        runner.push_ok("vitis_hls", ToolOutput::default());

        let opts = HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: ClockPeriod::from_picoseconds(3330),
            other_configs: String::new(),
            cflags: Vec::new(),
            skip_based_on_mtime: true,
            jobs: Some(1),
            keep_work_dir: false,
        };

        // Ignore the run result (no csynth.xml staged, so harvest
        // fails) — what we care about is that the runner was called
        // at all, which proves the stale-skip bug is gone.
        let _ = run_hls_for_leaves(&runner, work, &design, &staged(work, &design), &opts);
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
        seed_rewritten(work);

        // Pre-populate the HDL dir with a `.v` file; ensure its mtime
        // is strictly newer than the source.
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();

        let design = leaf_design();
        // Runner with no queued responses: any call fails loudly.
        let runner = MockToolRunner::new();

        let opts = HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: ClockPeriod::from_picoseconds(3330),
            other_configs: String::new(),
            cflags: Vec::new(),
            skip_based_on_mtime: true,
            jobs: Some(1),
            keep_work_dir: false,
        };
        let out = run_hls_for_leaves(&runner, work, &design, &staged(work, &design), &opts)
            .expect("cache hit path must succeed");
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
        seed_rewritten(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); wire fresh; endmodule\n").unwrap();

        let dir_mtime_after = fs::metadata(&hdl).unwrap().modified().unwrap();
        assert_eq!(
            dir_mtime_after, dir_mtime_before,
            "overwriting an existing HDL file should leave the directory mtime unchanged"
        );

        let opts = HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: ClockPeriod::from_picoseconds(3330),
            other_configs: String::new(),
            cflags: Vec::new(),
            skip_based_on_mtime: true,
            jobs: Some(1),
            keep_work_dir: false,
        };
        let runner = MockToolRunner::new();
        let design = leaf_design();
        let out = run_hls_for_leaves(&runner, work, &design, &staged(work, &design), &opts)
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
        let reports = crate::util::utf8(tmp.path().join("report"));
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
            report.area.ff, 843,
            "must read the task's own area, not the sub-loop's",
        );
        assert_eq!(report.area.bram_18k, 1);
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
        seed_rewritten(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); wire fresh; endmodule\n").unwrap();

        let files = list_hdl_files(&crate::util::utf8(hdl)).unwrap();
        let srcs = [crate::util::utf8(
            work.join(crate::tapacc::REWRITTEN_DIR).join("Add.cpp"),
        )];
        assert!(
            !hdl_files_are_newer_than(&files, &srcs),
            "one stale emitted file must invalidate the HLS cache"
        );
    }
}
