//! Per-task Vitis HLS invocation, ported from
//! `tapa/program/hls.py::ProgramHlsMixin._run_hls_task` + `.run_hls`.

use std::fs;
use std::path::{Path, PathBuf};

use tapa_task_graph::Design;
use tapa_xilinx::{run_hls_with_retry, run_hls_with_retry_in_stage, HlsJob, HlsOutput, ToolRunner};

use crate::error::{CliError, Result};
use crate::steps::synth::cpp_extract::cpp_path_for;

/// Python parity: `tapa/program/hls.py` uses `_HLS_MAX_RETRIES = 2`
/// → up to 3 attempts total. Vitis HLS occasionally fails with a
/// transient `Pre-synthesis failed.` diagnostic that re-runs clean,
/// so the retry wrapper keys off that substring.
const HLS_MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub struct TaskHlsLayout {
    pub reports_dir: PathBuf,
    pub hdl_dir: PathBuf,
}

impl TaskHlsLayout {
    pub fn new(work_dir: &Path, task_name: &str) -> Self {
        let base = work_dir.join("hls").join(task_name);
        Self {
            reports_dir: base.join("report"),
            hdl_dir: base.join("verilog"),
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
    /// Mirror of the click `--jobs N` flag. Python parity:
    /// `ThreadPoolExecutor(max_workers=jobs)`. `None` or 1 → serial.
    pub jobs: Option<u32>,
    /// Mirror of the click `--keep-hls-work-dir` flag. When true,
    /// `run_hls` stages under `<work_dir>/hls/<task>/project` (kept
    /// on disk) instead of a tempdir so the Vitis project + logs
    /// survive after a failure. Matches Python's
    /// `ProgramHlsMixin.run_hls(work_dir=...)`.
    pub keep_work_dir: bool,
}

/// Run HLS for every task that targets HLS. Mirrors Python's
/// `ProgramHlsMixin.run_hls`, which iterates **all** `_tasks.values()`
/// (not just leaves) — the upper-task shell is needed by codegen so
/// the parent module's port surface is parseable. Tasks whose
/// `target == "ignore"` are skipped (Python promotes them to
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
        if task.target.as_deref() == Some("ignore") {
            continue;
        }
        let layout = TaskHlsLayout::new(work_dir, task_name);

        let cpp_source = cpp_path_for(work_dir, task_name);
        if !cpp_source.is_file() {
            return Err(CliError::InvalidArg(format!(
                "missing extracted C++ source `{}` for task `{task_name}`",
                cpp_source.display(),
            )));
        }

        // Cache-hit check BEFORE creating `layout.hdl_dir`: the
        // earlier `create_dir_all` raced with
        // `hdl_dir_is_newer_than`, making every task on a clean work
        // dir look cached against the just-extracted `.cpp` source
        // and skipping the run with an empty `verilog_files` list.
        // Also require at least one `.v` file, so an empty leftover
        // directory does not trip the skip either.
        if options.skip_based_on_mtime
            && layout.hdl_dir.is_dir()
            && hdl_dir_is_newer_than(&layout.hdl_dir, &cpp_source)
        {
            let verilog_files = list_verilog_files(&layout.hdl_dir)?;
            if !verilog_files.is_empty() {
                log::info!(
                    "skipping HLS for `{task_name}` (mtime cache hit at {})",
                    layout.hdl_dir.display(),
                );
                // `reports_dir` must still exist for downstream
                // readers; the skip path does not touch `hdl_dir`.
                fs::create_dir_all(&layout.reports_dir)?;
                plan.push((
                    task_name.clone(),
                    layout,
                    Work::Skip(HlsOutput {
                        csynth: tapa_xilinx::CsynthReport::default(),
                        verilog_files,
                        report_paths: Vec::new(),
                        stdout: String::new(),
                        stderr: String::new(),
                    }),
                ));
                continue;
            }
        }

        fs::create_dir_all(&layout.reports_dir)?;
        fs::create_dir_all(&layout.hdl_dir)?;

        let job = HlsJob {
            task_name: task_name.clone(),
            cpp_source,
            cflags: options.cflags.clone(),
            target_part: options.part_num.clone(),
            top_name: task_name.clone(),
            clock_period: options.clock_period.clone(),
            reports_out_dir: layout.reports_dir.clone(),
            hdl_out_dir: layout.hdl_dir.clone(),
            other_configs: options.other_configs.clone(),
            ..HlsJob::default()
        };

        // `--keep-hls-work-dir`: stage under
        // `<work_dir>/hls/<task>/project` so the Vitis project + logs
        // survive the run for post-mortem inspection. The retry
        // wrapper reuses that single dir across attempts (a
        // partially-failed `project/` may contaminate the next
        // attempt, but the operator opted in).
        //
        // Default path: hand the job off to `run_hls_with_retry`,
        // which allocates a *fresh* `tempfile::tempdir()` for every
        // attempt. Mirrors the retired Python flow where each
        // transient `Pre-synthesis failed.` retry started from a
        // clean project tree.
        let work = if options.keep_work_dir {
            let persistent = work_dir.join("hls").join(task_name).join("project");
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
    let results: Vec<Result<Option<HlsOutput>>> =
        dispatch_plan(runner, &plan, worker_count);

    // No explicit cleanup: `RunFresh` lets `run_hls_with_retry` own
    // its per-attempt tempdir and drop it. `RunInStage` is kept on
    // disk intentionally under `<work_dir>/hls/<task>/project`.

    // Assemble output in the original plan order, surfacing the first
    // error.
    let mut out = Vec::with_capacity(plan.len());
    for ((task_name, layout, work), result) in plan.into_iter().zip(results) {
        let hls_out = match work {
            Work::Skip(pre) => pre,
            Work::RunInStage(..) | Work::RunFresh(_) => {
                result?.expect("Run must yield Some")
            }
        };
        out.push((task_name, layout, hls_out));
    }
    Ok(out)
}

fn resolve_worker_count(jobs: Option<u32>, plan: &[(String, TaskHlsLayout, impl Sized)]) -> usize {
    // Python parity: `tapa/program/hls.py` evaluates
    // `jobs = jobs or cpu_count(logical=False)` before
    // `ThreadPoolExecutor(max_workers=jobs)`, so the default on a
    // multi-core machine synthesizes tasks in parallel. Mirror that:
    // explicit `--jobs N` wins; otherwise pick the host's physical
    // core count (falling back to 1 if unavailable). Cap by live work
    // so we never spawn more workers than jobs to dispatch.
    let desired = jobs.map_or_else(default_hls_workers, |j| j.max(1) as usize);
    desired.min(plan.len().max(1))
}

/// Python's `psutil.cpu_count(logical=False)` equivalent. `std`'s
/// `available_parallelism` returns logical cores; that's a safe upper
/// bound that still parallelizes well for HLS (IO-bound per task),
/// and it avoids pulling in `num_cpus` as a new dep.
fn default_hls_workers() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

fn dispatch_plan(
    runner: &dyn ToolRunner,
    plan: &[(String, TaskHlsLayout, impl PlanEntry)],
    worker_count: usize,
) -> Vec<Result<Option<HlsOutput>>> {
    // Parallel dispatch via `std::thread::scope`. The `ToolRunner`
    // trait carries `Send + Sync`, so we can share `&dyn ToolRunner`
    // across threads without `Arc`.
    let len = plan.len();
    let results: std::sync::Mutex<Vec<Option<Result<Option<HlsOutput>>>>> =
        std::sync::Mutex::new((0..len).map(|_| None).collect());
    let next = std::sync::atomic::AtomicUsize::new(0);

    std::thread::scope(|s| {
        for _ in 0..worker_count.max(1) {
            s.spawn(|| loop {
                let idx = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if idx >= len {
                    return;
                }
                let (_, _, work) = &plan[idx];
                let r = work.execute(runner);
                let mut guard = results.lock().unwrap();
                guard[idx] = Some(r);
            });
        }
    });

    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|r| r.unwrap_or(Ok(None)))
        .collect()
}

trait PlanEntry: Sync {
    fn execute(&self, runner: &dyn ToolRunner) -> Result<Option<HlsOutput>>;
}

impl PlanEntry for Work {
    fn execute(&self, runner: &dyn ToolRunner) -> Result<Option<HlsOutput>> {
        match self {
            Self::Skip(_) => Ok(None),
            Self::RunInStage(job, stage_dir) => {
                let out = run_hls_with_retry_in_stage(
                    runner,
                    job,
                    HLS_MAX_ATTEMPTS,
                    stage_dir,
                )
                .map_err(CliError::from)?;
                Ok(Some(out))
            }
            Self::RunFresh(job) => {
                // `run_hls_with_retry` allocates a fresh
                // `tempfile::tempdir()` per attempt — mirrors the
                // Python non-keep retry path.
                let out = run_hls_with_retry(runner, job, HLS_MAX_ATTEMPTS)
                    .map_err(CliError::from)?;
                Ok(Some(out))
            }
        }
    }
}

/// Internal work state for the plan pass. Kept module-private —
/// `PlanEntry` is the caller-visible marker.
#[allow(clippy::large_enum_variant, reason = "Work is held briefly; \
    boxing adds allocations without removing the size difference \
    between the large `HlsJob + PathBuf` variant and the trivial Skip")]
enum Work {
    Skip(HlsOutput),
    /// `--keep-hls-work-dir`: persistent project under
    /// `<work_dir>/hls/<task>/project` reused across retries.
    RunInStage(HlsJob, PathBuf),
    /// Default: each retry attempt gets its own fresh tempdir so a
    /// partially-failed `project/` cannot contaminate the next try.
    RunFresh(HlsJob),
}

fn hdl_dir_is_newer_than(hdl_dir: &Path, cpp_source: &Path) -> bool {
    let Ok(hdl_meta) = fs::metadata(hdl_dir) else { return false };
    let Ok(cpp_meta) = fs::metadata(cpp_source) else { return false };
    let (Ok(hdl_t), Ok(cpp_t)) = (hdl_meta.modified(), cpp_meta.modified()) else {
        return false;
    };
    hdl_t > cpp_t
}

fn list_verilog_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let p = ent.path();
        if p.extension().and_then(|s| s.to_str()) == Some("v") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use indexmap::IndexMap;
    use tapa_task_graph::{Design, TaskTopology};
    use tapa_xilinx::{MockToolRunner, ToolInvocation, ToolOutput};

    fn leaf_design() -> Design {
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Add".to_string(),
            TaskTopology {
                name: "Add".to_string(),
                level: "lower".to_string(),
                code: String::new(),
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
        Design {
            top: "Add".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        }
    }

    /// Regression test: with `--skip-hls-based-on-mtime`
    /// on a clean work dir, `create_dir_all(hdl_dir)` used to run
    /// BEFORE the cache freshness check, so a freshly-created `hdl_dir`
    /// had a newer mtime than the extracted `.cpp` and every task
    /// looked cached — with an empty `verilog_files` list. The runner
    /// must now require an EXISTING `hdl_dir` that ALREADY contains
    /// at least one `.v` file before honoring the skip.
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
        let out = run_hls_for_leaves(&runner, work, &design, &opts)
            .expect("cache hit path must succeed");
        assert_eq!(out.len(), 1);
        let (_, _, hls_out) = &out[0];
        assert!(
            !hls_out.verilog_files.is_empty(),
            "cache hit must carry the existing HDL files forward",
        );
        assert!(runner.calls().is_empty(), "cache hit must not call Vitis");
    }
}
