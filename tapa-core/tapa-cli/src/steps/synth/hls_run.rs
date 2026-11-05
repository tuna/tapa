//! Per-task Vitis HLS invocation with parallel dispatch and
//! settings- and mtime-based skipping.

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

#[derive(Debug)]
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

/// Mutually exclusive ways to obtain task RTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlsReuse {
    Never,
    IfFresh,
    Required,
    Fixtures,
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
    pub reuse: HlsReuse,
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
pub fn run_hls_for_tasks(
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
        let settings_path = layout.hdl_dir.with_file_name("settings");
        let cache_key = job.cache_key()?;
        let settings_match =
            fs::read(&settings_path).is_ok_and(|saved| saved == cache_key.as_bytes());
        // Fixtures are explicitly supplied test inputs. Real cache entries
        // require the settings recorded after a successful synthesis run.
        if (options.reuse != HlsReuse::Never && settings_match)
            || options.reuse == HlsReuse::Fixtures
        {
            let hdl_files = list_hdl_files(&layout.hdl_dir)?;
            if hdl_files.iter().any(|path| is_verilog(path))
                && hdl_files_are_newer_than(&hdl_files, sources.freshness_deps())
            {
                match cached_hls_output(&layout, task_name, hdl_files, options) {
                    Ok(output) => {
                        log::info!("reusing HLS output for `{task_name}`");
                        plan.push((task_name.clone(), layout, Work::Skip(output)));
                        continue;
                    }
                    Err(error)
                        if matches!(options.reuse, HlsReuse::Required | HlsReuse::Fixtures) =>
                    {
                        return Err(error);
                    }
                    Err(error) => log::info!("regenerating HLS for `{task_name}`: {error}"),
                }
            }
        }

        if matches!(options.reuse, HlsReuse::Required | HlsReuse::Fixtures) {
            return Err(CliError::Codegen(format!(
                "cached HLS output for `{task_name}` is missing or stale (including settings changes); \
                 rerun without `--require-hls-cache` or `--hls-fixtures` to regenerate it"
            )));
        }

        fs::create_dir_all(&layout.reports_dir)?;
        fs::create_dir_all(&layout.hdl_dir)?;

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
    prune_inactive_hls_outputs(work_dir, design)?;
    Ok(out)
}

// Retained projects are diagnostics, but reports and RTL for removed or
// non-HLS tasks must no longer be visible to packaging or later cache lookups.
fn prune_inactive_hls_outputs(work_dir: &Path, design: &Design) -> Result<()> {
    let root = work_dir.join("hls");
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || design
                .tasks
                .get(entry.file_name().to_string_lossy().as_ref())
                .is_some_and(|task| task.synth == SynthTarget::Hls)
        {
            continue;
        }
        for name in ["report", "verilog"] {
            let path = entry.path().join(name);
            if path.exists() {
                fs::remove_dir_all(path)?;
            }
        }
        let settings = entry.path().join("settings");
        if settings.exists() {
            fs::remove_file(settings)?;
        }
    }
    Ok(())
}

fn dispatch_plan(
    runner: &dyn ToolRunner,
    plan: &[(String, TaskHlsLayout, Work)],
    worker_count: usize,
) -> Result<Vec<Result<Option<HlsOutput>>>> {
    crate::util::run_in_pool(worker_count, "HLS worker", CliError::Codegen, || {
        plan.par_iter()
            .map(|(_, _, work)| work.execute(runner))
            .collect()
    })
}

impl Work {
    fn execute(&self, runner: &dyn ToolRunner) -> Result<Option<HlsOutput>> {
        let job = match self {
            Self::Skip(_) => return Ok(None),
            Self::RunInStage(job, _) | Self::RunFresh(job) => job,
        };
        let settings_path = job.hdl_out_dir.with_file_name("settings");
        // A failed attempt may have partially replaced output. Never leave
        // the previous run's settings certifying that output as a cache hit.
        if settings_path.exists() {
            fs::remove_file(&settings_path)?;
        }
        let out = match self {
            Self::RunInStage(job, stage_dir) => {
                run_hls_with_retry_in_stage(runner, job, HLS_MAX_ATTEMPTS, stage_dir)?
            }
            Self::RunFresh(job) => run_hls_with_retry(runner, job, HLS_MAX_ATTEMPTS)?,
            Self::Skip(_) => unreachable!(),
        };
        crate::state::json::write_bytes_atomic(
            settings_path
                .parent()
                .expect("task directory")
                .as_std_path(),
            "settings",
            job.cache_key()?.as_bytes(),
        )?;
        Ok(Some(out))
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

/// Per-task HLS cflags: the task manifest's rewritten-tree include
/// roots (the tree root, then one `-I` per include dir; the empty
/// entry is the tree root itself, already covered), then the user's
/// graph cflags, one `-D` per guard define, and the shared vendor
/// tail (`HlsRunOptions::cflags`). The rewritten roots must precede
/// user include paths so quoted and unchanged angle includes resolve
/// to mirrored headers instead of same-named originals. Staging has
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
    let mut flags = Vec::new();
    flags.push(format!("-I{}", tree_root.display()));
    for dir in &task.include_dirs {
        if !dir.is_empty() {
            flags.push(format!("-I{}", tree_root.join(dir).display()));
        }
    }
    flags.extend_from_slice(user);
    for define in &task.defines {
        flags.push(format!("-D{define}"));
    }
    flags.extend_from_slice(tail);
    flags
}

fn hdl_files_are_newer_than(hdl_files: &[Utf8PathBuf], deps: &[Utf8PathBuf]) -> bool {
    if hdl_files.is_empty() || deps.is_empty() {
        return false;
    }
    // Every emitted HDL file must postdate the newest dependency;
    // a metadata gap on any input conservatively reports stale.
    let mut newest_dep = std::time::SystemTime::UNIX_EPOCH;
    for dep in deps {
        let Ok(t) = fs::metadata(dep).and_then(|meta| meta.modified()) else {
            return false;
        };
        newest_dep = newest_dep.max(t);
    }
    hdl_files.iter().all(|hdl| {
        fs::metadata(hdl)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|hdl_t| hdl_t > newest_dep)
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
                "cannot inspect HDL directory `{}`: {e}",
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

fn cached_hls_output(
    layout: &TaskHlsLayout,
    task_name: &str,
    hdl_files: Vec<Utf8PathBuf>,
    options: &HlsRunOptions,
) -> Result<HlsOutput> {
    let csynth = find_and_parse_csynth(&layout.reports_dir, task_name)?;
    let clock = ClockPeriod::from_nanoseconds_str(&csynth.target_clock_period_ns);
    if csynth.part != options.part_num || clock.ok() != Some(options.clock_period) {
        return Err(CliError::Codegen(format!(
            "cached HLS report for `{task_name}` does not match requested part {} and clock {} ns",
            options.part_num, options.clock_period,
        )));
    }
    let report_paths = list_hdl_files(&layout.reports_dir)?;
    Ok(HlsOutput {
        csynth,
        verilog_files: hdl_files,
        report_paths,
        stdout: String::new(),
        stderr: String::new(),
    })
}

/// Reload and parse the task's top-level csynth report on an mtime-skip
/// cache hit — the exact file the live HLS harvest parses, resolved by
/// [`tapa_xilinx::csynth_report_path`].
fn find_and_parse_csynth(
    reports_dir: &camino::Utf8Path,
    task_name: &str,
) -> Result<tapa_xilinx::CsynthReport> {
    let report_xml = tapa_xilinx::csynth_report_path(reports_dir, task_name);
    let bytes = fs::read(&report_xml).map_err(|e| {
        CliError::Codegen(format!(
            "cannot read cached csynth report `{}`: {e}",
            report_xml.as_str(),
        ))
    })?;
    tapa_xilinx::parse_csynth_xml(&bytes).map_err(|e| {
        CliError::Codegen(format!(
            "cannot parse cached csynth report `{}`: {e}",
            report_xml.as_str()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use tapa_xilinx::{MockToolRunner, ToolOutput};

    use crate::steps::synth::fixtures;

    fn seed_rewritten(work: &Path) {
        let tree = work.join(crate::tapacc::REWRITTEN_DIR);
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("Add.cpp"), b"int main(){}\n").unwrap();
        fixtures::write_manifest(work, &["Add.cpp"]);
    }

    fn seed_cached_settings(work: &Path) {
        let layout = TaskHlsLayout::new(work, "Add");
        let job = HlsJob::builder()
            .task_name("Add".into())
            .top_name("Add".into())
            .srcs(vec![crate::util::utf8(work.join("rewritten/Add.cpp"))])
            .cflags(vec![format!("-I{}", work.join("rewritten").display())])
            .target_part("xcvu37p".into())
            .clock_period("3.33".into())
            .reports_out_dir(layout.reports_dir.clone())
            .hdl_out_dir(layout.hdl_dir.clone())
            .build();
        fs::write(
            layout.hdl_dir.with_file_name("settings"),
            job.cache_key().unwrap(),
        )
        .unwrap();
        fs::create_dir_all(&layout.reports_dir).unwrap();
        fs::write(
            layout.reports_dir.join("Add_csynth.xml"),
            csynth_xml("Add", 1, 0),
        )
        .unwrap();
    }

    fn skip_hls_options() -> HlsRunOptions {
        HlsRunOptions {
            part_num: "xcvu37p".to_string(),
            clock_period: ClockPeriod::from_picoseconds(3330),
            other_configs: String::new(),
            cflags: Vec::new(),
            reuse: HlsReuse::IfFresh,
            jobs: Some(1),
            keep_work_dir: false,
        }
    }

    fn leaf_design() -> Design {
        let mut tasks = BTreeMap::new();
        tasks.insert("Add".to_string(), crate::testutil::task("Add"));
        Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: "Add".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        }
    }

    /// The manifest→cflags contract: tree root and one `-I` per
    /// include dir (empty = root, skipped), then user cflags and one
    /// `-D` per guard, then the shared tail — whose closing
    /// `-I<extra-runtime-include>` must remain the final token
    /// (Vitis glues tokens onto paths ending in `include`).
    #[test]
    fn task_cflags_compose_tree_includes_user_defines_then_tail() {
        let task = Task {
            include_dirs: vec![
                String::new(),
                "_external/ab12cd34".to_string(),
                "common".to_string(),
            ],
            defines: vec!["TAPA_TASK_DEF_Add".to_string()],
            ..crate::testutil::task("Add")
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
                "-I/work/rewritten".to_string(),
                "-I/work/rewritten/_external/ab12cd34".to_string(),
                "-I/work/rewritten/common".to_string(),
                "-std=c++14".to_string(),
                "-I/app".to_string(),
                "-DTAPA_TASK_DEF_Add".to_string(),
                "-isystem/tapa-lib".to_string(),
                "-I/extra-runtime-include".to_string(),
            ]
        );
    }

    /// User `-I` paths previously won over the mirror. Exercise real
    /// compiler lookup for both include spellings; tapacc deliberately
    /// leaves the angle spelling unchanged.
    #[test]
    fn task_cflags_select_mirror_for_quoted_and_angle_includes() {
        use std::process::Command;

        let tmp = tempfile::tempdir().expect("tempdir");
        let original_include = tmp.path().join("original/include");
        let tree = tmp.path().join("rewritten");
        let mirror_include = tree.join("include");
        let mirror_src = tree.join("src");
        fs::create_dir_all(&original_include).unwrap();
        fs::create_dir_all(&mirror_include).unwrap();
        fs::create_dir_all(&mirror_src).unwrap();

        for (header, marker) in [("quoted.h", "MIRROR_QUOTED"), ("angle.h", "MIRROR_ANGLE")] {
            fs::write(
                original_include.join(header),
                format!("#error \"original {header} selected\"\n"),
            )
            .unwrap();
            fs::write(mirror_include.join(header), format!("#define {marker} 1\n")).unwrap();
        }
        let source = mirror_src.join("Add.cpp");
        fs::write(
            &source,
            r#"#include "quoted.h"
#include <angle.h>
#if !defined(MIRROR_QUOTED) || !defined(MIRROR_ANGLE)
#error "rewritten headers were not selected"
#endif
"#,
        )
        .unwrap();

        let task = Task {
            srcs: vec!["src/Add.cpp".to_string()],
            include_dirs: vec![String::new(), "include".to_string()],
            ..crate::testutil::task("Add")
        };
        let flags = task_cflags(
            &[format!("-I{}", original_include.display())],
            &tree,
            &task,
            &[],
        );
        let output = Command::new("g++")
            .arg("-fsyntax-only")
            .args(&flags)
            .arg(&source)
            .output()
            .expect("run C++ compiler for include-selection regression");
        assert!(
            output.status.success(),
            "quoted and angle includes must select mirrored headers; flags: {flags:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
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

        let opts = skip_hls_options();

        // Ignore the run result (no csynth.xml staged, so harvest
        // fails) — what we care about is that the runner was called
        // at all, which proves the stale-skip bug is gone.
        let _ = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &opts,
        );
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
    }

    #[test]
    fn required_cache_miss_does_not_run_hls() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        seed_rewritten(work);
        let design = leaf_design();
        let runner = MockToolRunner::new();
        let opts = HlsRunOptions {
            reuse: HlsReuse::Required,
            ..skip_hls_options()
        };
        let error = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &opts,
        )
        .expect_err("a required cache miss must fail");
        assert!(error.to_string().contains("missing or stale"), "{error}");
        assert!(runner.calls().is_empty(), "a strict miss must not run HLS");
    }

    #[test]
    fn required_cache_rejects_a_missing_csynth_report() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        seed_rewritten(work);
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        seed_cached_settings(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(
            hdl.join("Add.v"),
            b"module Add(); endmodule
",
        )
        .unwrap();
        fs::remove_file(work.join("hls/Add/report/Add_csynth.xml")).unwrap();
        let design = leaf_design();
        let runner = MockToolRunner::new();
        let opts = HlsRunOptions {
            reuse: HlsReuse::Required,
            ..skip_hls_options()
        };
        let error = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &opts,
        )
        .expect_err("a required report must be present");
        assert!(
            error.to_string().contains("cached csynth report"),
            "{error}"
        );
        assert!(
            runner.calls().is_empty(),
            "a strict report miss must not run HLS"
        );
    }

    #[test]
    fn cache_records_successful_settings_and_rejects_configuration_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path();
        seed_rewritten(work);
        let design = leaf_design();
        let sources = fixtures::staged(work, &design);
        let runner = MockToolRunner::new();
        runner.push_ok("vitis_hls", ToolOutput::default());
        let syn = crate::util::utf8(work.join("hls/Add/project/project/Add/syn"));
        runner.attach_download(
            syn.join("verilog/Add.v"),
            b"module Add(); endmodule".to_vec(),
        );
        runner.attach_download(
            syn.join("report/Add_csynth.xml"),
            csynth_xml("Add", 1, 0).into_bytes(),
        );
        let original = HlsRunOptions {
            keep_work_dir: true,
            ..skip_hls_options()
        };
        std::thread::sleep(std::time::Duration::from_millis(10));
        run_hls_for_tasks(&runner, work, &design, &sources, &original).unwrap();
        let required = HlsRunOptions {
            reuse: HlsReuse::Required,
            ..original.clone()
        };
        run_hls_for_tasks(&runner, work, &design, &sources, &required).expect("unchanged cache");

        for change in 0..5 {
            let mut changed = required.clone();
            let mut graph = design.clone();
            match change {
                0 => changed.part_num = "xcu250".into(),
                1 => changed.clock_period = ClockPeriod::from_picoseconds(2000),
                2 => changed.other_configs = "config_compile -pipeline_loops 0".into(),
                3 => changed.cflags.push("-DNEW_FLAG".into()),
                _ => graph.cflags.push("-DUSER_FLAG".into()),
            }
            run_hls_for_tasks(&runner, work, &graph, &sources, &changed)
                .expect_err("changed synthesis settings must invalidate the cache");
        }
        assert_eq!(runner.calls().len(), 1, "strict misses must never run HLS");

        // A failed regeneration cannot certify the old or partial outputs.
        runner.push_failure("vitis_hls", 1, "ERROR: synthesis failed");
        let rerun = HlsRunOptions {
            reuse: HlsReuse::Never,
            ..original
        };
        run_hls_for_tasks(&runner, work, &design, &sources, &rerun)
            .expect_err("must reject incompatible output");
        assert!(!work.join("hls/Add/settings").exists());
        run_hls_for_tasks(&runner, work, &design, &sources, &required)
            .expect_err("must reject incompatible output");

        // Explicit fixtures do not silently become a certified cache entry.
        let fixtures = HlsRunOptions {
            reuse: HlsReuse::Fixtures,
            ..required
        };
        run_hls_for_tasks(&runner, work, &design, &sources, &fixtures).unwrap();
        assert!(!work.join("hls/Add/settings").exists());
        let wrong_part = HlsRunOptions {
            part_num: "xcu250".into(),
            ..fixtures
        };
        run_hls_for_tasks(&runner, work, &design, &sources, &wrong_part)
            .expect_err("must reject incompatible output");
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
        seed_cached_settings(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();

        let removed = work.join("hls/Removed");
        for directory in ["report", "verilog", "project"] {
            fs::create_dir_all(removed.join(directory)).unwrap();
            fs::write(removed.join(directory).join("old.txt"), "old output").unwrap();
        }
        fs::write(removed.join("settings"), "old settings").unwrap();
        let design = leaf_design();
        // Runner with no queued responses: any call fails loudly.
        let runner = MockToolRunner::new();

        let opts = skip_hls_options();
        let out = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &opts,
        )
        .expect("cache hit path must succeed");
        assert_eq!(out.len(), 1);
        let (_, _, hls_out) = &out[0];
        assert!(
            !hls_out.verilog_files.is_empty(),
            "cache hit must carry the existing HDL files forward",
        );
        assert!(runner.calls().is_empty(), "cache hit must not call Vitis");
        assert!(!removed.join("report").exists());
        assert!(!removed.join("verilog").exists());
        assert!(!removed.join("settings").exists());
        assert!(removed.join("project/old.txt").exists());
    }

    #[test]
    fn newer_mirrored_header_defeats_mtime_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        seed_rewritten(work);
        let tree = work.join(crate::tapacc::REWRITTEN_DIR);
        fs::write(tree.join("Add.h"), "#define VALUE 1\n").unwrap();
        fixtures::write_manifest(work, &["Add.cpp", "Add.h"]);

        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        seed_cached_settings(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(tree.join("Add.h"), "#define VALUE 2\n").unwrap();

        let design = leaf_design();
        let runner = MockToolRunner::new();
        runner.push_ok("vitis_hls", ToolOutput::default());
        let _ = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &skip_hls_options(),
        );
        assert_eq!(
            runner.calls().len(),
            1,
            "a header newer than HDL must invalidate the HLS cache"
        );
    }

    #[test]
    fn unrelated_newer_file_does_not_defeat_mtime_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        seed_rewritten(work);
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        seed_cached_settings(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(
            work.join(crate::tapacc::REWRITTEN_DIR).join("notes.txt"),
            "not TAPA-owned\n",
        )
        .unwrap();

        let design = leaf_design();
        let runner = MockToolRunner::new();
        run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &skip_hls_options(),
        )
        .expect("unowned files must not affect freshness");
        assert!(runner.calls().is_empty(), "cache hit must not call Vitis");
    }

    #[test]
    fn manifest_rewrite_after_file_removal_defeats_mtime_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        seed_rewritten(work);
        let tree = work.join(crate::tapacc::REWRITTEN_DIR);
        fs::write(tree.join("removed.h"), "#define OLD 1\n").unwrap();
        fixtures::write_manifest(work, &["Add.cpp", "removed.h"]);

        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        seed_cached_settings(work);
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(hdl.join("Add.v"), b"module Add(); endmodule\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::remove_file(tree.join("removed.h")).unwrap();
        fixtures::write_manifest(work, &["Add.cpp"]);

        let design = leaf_design();
        let runner = MockToolRunner::new();
        runner.push_ok("vitis_hls", ToolOutput::default());
        let _ = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &skip_hls_options(),
        );
        assert_eq!(
            runner.calls().len(),
            1,
            "the manifest mtime must retain file-removal freshness information"
        );
    }

    #[test]
    fn overwritten_hdl_file_refreshes_cache_without_touching_parent_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = tmp.path();
        let hdl = work.join("hls").join("Add").join("verilog");
        fs::create_dir_all(&hdl).unwrap();
        seed_cached_settings(work);
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

        let opts = skip_hls_options();
        let runner = MockToolRunner::new();
        let design = leaf_design();
        let out = run_hls_for_tasks(
            &runner,
            work,
            &design,
            &fixtures::staged(work, &design),
            &opts,
        )
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
             <Part>xcvu37p</Part><TargetClockPeriod>3.33</TargetClockPeriod></UserAssignments>\n\
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
