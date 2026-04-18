//! Vitis HLS orchestration.
//!
//! Ports `tapa/backend/xilinx_hls.py::RunHls`, `tapa/program/hls.py`,
//! and `tapa/program/hls_runner.py`: TCL emission, invocation via a
//! `ToolRunner`, report parsing, and a bounded retry wrapper keyed on
//! transient-failure substrings lifted verbatim from the Python set.

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolOutput, ToolRunner};
use crate::tools::hls::report::{parse_csynth_xml, CsynthReport};

pub mod report;

/// Substrings the default transient predicate keys off.
///
/// Kept for fixture-driven tests and custom predicates. The real
/// production predicate (`is_transient_hls_output`) matches Python's
/// logic in `tapa/program/hls.py`: stdout contains `Pre-synthesis
/// failed.` without a subsequent `\nERROR:` line.
pub const DEFAULT_TRANSIENT_HLS_PATTERNS: &[&str] = &[
    "Pre-synthesis failed.",
    "TCP connection closed",
    "License checkout failed",
    "Connection reset by peer",
    "No license available",
    "FLEXnet Licensing error",
];

/// Production retry predicate ported verbatim from
/// `tapa/program/hls.py::_run_hls_task`: a Vitis HLS invocation is
/// considered transient iff its stdout contains `Pre-synthesis
/// failed.` and does **not** contain `\nERROR:`.
#[must_use]
pub fn is_transient_hls_output(stdout: &str, _stderr: &str) -> bool {
    stdout.contains("Pre-synthesis failed.") && !stdout.contains("\nERROR:")
}

#[derive(Debug, Clone)]
pub struct HlsJob {
    pub task_name: String,
    pub cpp_source: PathBuf,
    pub cflags: Vec<String>,
    pub target_part: String,
    pub top_name: String,
    pub clock_period: String,
    pub reports_out_dir: PathBuf,
    pub hdl_out_dir: PathBuf,
    /// Additional files the runner needs to stage up (remote tar-pipe
    /// uploads).
    pub uploads: Vec<PathBuf>,
    /// Files the runner must stage down after the tool exits.
    pub downloads: Vec<PathBuf>,
    /// Optional HLS `other_configs` TCL fragment. Appended verbatim.
    pub other_configs: String,
    /// Solution name; defaults to the task name when empty.
    pub solution_name: String,
    /// Reset level for `config_rtl` (ports Python's `reset_low` toggle
    /// in `_build_rtl_config`); defaults to `low` to match the Python
    /// `RunHls` default.
    pub reset_low: bool,
    /// Enable `-module_auto_prefix` on the `config_rtl` line. Defaults
    /// to `true`, matching `HlsConfig(auto_prefix=True)` in Python.
    pub auto_prefix: bool,
    /// Optional override. When `None`, the production
    /// `is_transient_hls_output` predicate is used.
    pub transient_patterns: Option<Arc<Vec<String>>>,
}

impl Default for HlsJob {
    fn default() -> Self {
        Self {
            task_name: String::new(),
            cpp_source: PathBuf::new(),
            cflags: Vec::new(),
            target_part: String::new(),
            top_name: String::new(),
            clock_period: String::new(),
            reports_out_dir: PathBuf::new(),
            hdl_out_dir: PathBuf::new(),
            uploads: Vec::new(),
            downloads: Vec::new(),
            other_configs: String::new(),
            solution_name: String::new(),
            reset_low: true,
            auto_prefix: true,
            transient_patterns: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlsOutput {
    pub csynth: CsynthReport,
    pub verilog_files: Vec<PathBuf>,
    pub report_paths: Vec<PathBuf>,
    pub stdout: String,
    pub stderr: String,
}

/// Build the Vitis HLS TCL script for the given job.
///
/// Ports the `HLS_COMMANDS` template in
/// `tapa/backend/xilinx_hls.py`: `open_project` → `set_top` →
/// `add_files` → `open_solution` → `set_part` → `create_clock` →
/// `config_compile` → `config_interface` → `{config}` →
/// `{other_configs}` → `config_rtl` → `csynth_design` → `exit`.
/// Port of `tapa/backend/xilinx_hls.py::_build_rtl_config`.
fn build_rtl_config(reset_low: bool, auto_prefix: bool) -> String {
    let mut line = format!(
        "config_rtl -reset_level {}",
        if reset_low { "low" } else { "high" }
    );
    if auto_prefix {
        // Python matches on `hls == "vitis_hls"` → `-module_auto_prefix`.
        line.push_str(" -module_auto_prefix");
    }
    line
}

/// Collect every `-I<dir>` / `-isystem<dir>` destination from the
/// job's CFLAGS that points at an existing absolute directory. These
/// need to be uploaded verbatim so the remote `vitis_hls` resolves
/// sibling headers the same way the local run would. Mirrors
/// `tapa/backend/xilinx_hls.py::_build_kernel_env`.
fn kernel_include_dirs(cflags: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in cflags {
        let trimmed = raw.trim();
        let dir_str = if let Some(rest) = trimmed.strip_prefix("-isystem") {
            rest.trim()
        } else if let Some(rest) = trimmed.strip_prefix("-I") {
            rest.trim()
        } else {
            continue;
        };
        if dir_str.is_empty() {
            continue;
        }
        let p = PathBuf::from(dir_str);
        if p.is_absolute() && p.is_dir() {
            out.push(p);
        }
    }
    out
}

/// Kernel metadata the TCL template loops over. Mirrors Python's
/// `TAPA_KERNEL_COUNT / TAPA_KERNEL_PATH_$i / TAPA_KERNEL_CFLAGS_$i`
/// env contract from `tapa/backend/xilinx_hls.py`. Keeping the
/// per-task paths in env entries (instead of baking them into the
/// TCL body) lets the remote runner rewrite them through its
/// rootfs-mirroring path-rewriter just like every other absolute
/// local path.
fn kernel_env_entries(job: &HlsJob) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    env.push(("TAPA_KERNEL_COUNT".into(), "1".into()));
    env.push((
        "TAPA_KERNEL_PATH_0".into(),
        job.cpp_source.display().to_string(),
    ));
    let cflags = job
        .cflags
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    env.push(("TAPA_KERNEL_CFLAGS_0".into(), cflags));
    env
}

#[must_use]
pub fn build_hls_tcl(job: &HlsJob) -> String {
    let solution = if job.solution_name.is_empty() {
        job.top_name.as_str()
    } else {
        job.solution_name.as_str()
    };
    let other_configs = if job.other_configs.is_empty() {
        String::new()
    } else {
        format!("{}\n", job.other_configs)
    };
    let rtl_config = build_rtl_config(job.reset_low, job.auto_prefix);
    // Per-kernel paths/CFLAGS come from the remote-safe
    // `TAPA_KERNEL_*` env entries — the body no longer hard-codes
    // local absolute paths, so the TCL uploads cleanly to any
    // host without a post-upload rewrite pass.
    format!(
        "cd [pwd]\n\
         open_project \"project\"\n\
         set_top {top}\n\
         for {{set i 0}} {{$i < $::env(TAPA_KERNEL_COUNT)}} {{incr i}} {{\n\
             set kpath [set ::env(TAPA_KERNEL_PATH_$i)]\n\
             set kcflags [set ::env(TAPA_KERNEL_CFLAGS_$i)]\n\
             add_files \"$kpath\" -cflags \"$kcflags\"\n\
         }}\n\
         open_solution \"{solution}\"\n\
         set_part {{{part}}}\n\
         create_clock -period {clock} -name default\n\
         config_compile -name_max_length 253\n\
         config_interface -m_axi_addr64\n\
         {other}\
         set_param hls.enable_hidden_option_error false\n\
         {rtl}\n\
         config_rtl -enableFreeRunPipeline=false\n\
         config_rtl -disableAutoFreeRunPipeline=true\n\
         csynth_design\n\
         exit\n",
        top = job.top_name,
        solution = solution,
        part = job.target_part,
        clock = job.clock_period,
        other = other_configs,
        rtl = rtl_config,
    )
}

fn is_transient(job: &HlsJob, stdout: &str, stderr: &str) -> bool {
    match job.transient_patterns.as_deref() {
        Some(v) => v
            .iter()
            .any(|p| stdout.contains(p.as_str()) || stderr.contains(p.as_str())),
        None => is_transient_hls_output(stdout, stderr),
    }
}

/// Run a single Vitis HLS invocation inside `stage_dir`. The runner
/// executes with cwd set to `stage_dir`; after the tool exits, the
/// `project/<solution>/syn/` subtree lives at
/// `stage_dir/project/<solution>/syn` on local runners or inside
/// the runner's remote work dir on remote runners. The caller is
/// responsible for invoking `runner.harvest` before touching the
/// artifacts on disk.
fn run_hls_attempt(
    runner: &dyn ToolRunner,
    job: &HlsJob,
    stage_dir: &std::path::Path,
) -> Result<ToolOutput> {
    let tcl = build_hls_tcl(job);
    let tcl_path = stage_dir.join("run_hls.tcl");
    std::fs::write(&tcl_path, tcl.as_bytes())?;
    let mut inv = ToolInvocation::new("vitis_hls")
        .arg("-f")
        .arg(tcl_path.display().to_string());
    inv.cwd = Some(stage_dir.to_path_buf());
    // Uploads: TCL, the kernel source, every `-I` / `-isystem`
    // include directory referenced by the cflags, plus any caller
    // extras. Mirrors the upload set `tapa/backend/xilinx_hls.py::
    // _build_kernel_env` returns.
    inv.uploads.push(tcl_path);
    if let Some(src_dir) = job.cpp_source.parent() {
        if src_dir.is_absolute() && src_dir.is_dir() {
            inv.uploads.push(src_dir.to_path_buf());
        } else {
            inv.uploads.push(job.cpp_source.clone());
        }
    } else {
        inv.uploads.push(job.cpp_source.clone());
    }
    inv.uploads.extend(kernel_include_dirs(&job.cflags));
    inv.uploads.extend(job.uploads.iter().cloned());

    // Kernel metadata via env entries — the `TAPA_*` prefix passes
    // the remote-env forwarding allowlist, and the remote runner's
    // path rewriter remaps absolute local paths in the values to
    // their rootfs counterparts.
    for (k, v) in kernel_env_entries(job) {
        inv.env.insert(k, v);
    }

    // Ask the runner to bring the HLS project tree — at least the
    // `syn/{report,verilog}` subtree — back onto the local filesystem
    // alongside everything else the caller requested. On local runs
    // the files are already under `cwd` (stage_dir); on remote runs
    // the runner tar-pipes the rootfs mirror back in place.
    inv.downloads.push(stage_dir.to_path_buf());
    inv.downloads.extend(job.downloads.iter().cloned());
    runner.run(&inv)
}

/// One Vitis HLS invocation — raw `ToolOutput` form. Kept as a
/// compatibility shim for callers that only need the raw exit
/// status (e.g. the per-task retry predicate in Python-equivalent
/// harnesses); it creates a throw-away staging dir and does *not*
/// harvest the syn subtree. Prefer [`run_hls`] or
/// [`run_hls_with_retry`] in production code.
pub fn run_hls_raw(runner: &dyn ToolRunner, job: &HlsJob) -> Result<ToolOutput> {
    let stage = tempfile::tempdir()?;
    run_hls_attempt(runner, job, stage.path())
}

/// Name of the HLS solution subdirectory — mirrors the TCL template's
/// `open_solution "<solution>"` value.
fn solution_name(job: &HlsJob) -> String {
    if job.solution_name.is_empty() {
        job.top_name.clone()
    } else {
        job.solution_name.clone()
    }
}

/// Copy every regular file under `src` into `dest`, recreating the
/// directory layout as it goes. Does nothing if `src` does not exist.
fn copy_tree(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    for ent in std::fs::read_dir(src)? {
        let ent = ent?;
        let sub_src = ent.path();
        let file_type = ent.file_type()?;
        let sub_dest = dest.join(ent.file_name());
        if file_type.is_dir() {
            std::fs::create_dir_all(&sub_dest)?;
            copy_tree(&sub_src, &sub_dest)?;
        } else if file_type.is_file() {
            if let Some(parent) = sub_dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&sub_src, &sub_dest)?;
        }
    }
    Ok(())
}

fn harvest_and_stage(
    _runner: &dyn ToolRunner,
    job: &HlsJob,
    stage_dir: &std::path::Path,
    out: ToolOutput,
) -> Result<HlsOutput> {
    // The runner already ensured the HLS project tree is on the
    // local filesystem (Local: it wrote directly under `cwd`; Remote:
    // `run_once` downloaded `cwd` back into place via the rootfs
    // mirror). We just need to copy the caller-facing slices out.
    let solution = solution_name(job);
    let syn_rel: PathBuf = ["project", &solution, "syn"].iter().collect();

    // Copy the real HLS artifacts into the caller-visible output dirs.
    let syn_abs = stage_dir.join(&syn_rel);
    std::fs::create_dir_all(&job.reports_out_dir)?;
    std::fs::create_dir_all(&job.hdl_out_dir)?;
    copy_tree(&syn_abs.join("report"), &job.reports_out_dir).map_err(|e| {
        XilinxError::HlsReportParse(format!(
            "stage reports {} → {}: {e}",
            syn_abs.join("report").display(),
            job.reports_out_dir.display()
        ))
    })?;
    copy_tree(&syn_abs.join("verilog"), &job.hdl_out_dir).map_err(|e| {
        XilinxError::HlsReportParse(format!(
            "stage verilog {} → {}: {e}",
            syn_abs.join("verilog").display(),
            job.hdl_out_dir.display()
        ))
    })?;

    let report_xml = job
        .reports_out_dir
        .join(format!("{}_csynth.xml", job.top_name));
    let fallback = job
        .reports_out_dir
        .join(format!("{}.csynth.xml", job.top_name));
    let report_xml = if report_xml.is_file() {
        report_xml
    } else {
        fallback
    };
    let bytes = std::fs::read(&report_xml).map_err(|_| {
        XilinxError::HlsReportParse(format!(
            "missing csynth.xml at {}",
            report_xml.display()
        ))
    })?;
    let csynth = parse_csynth_xml(&bytes)?;

    let verilog_files = collect_files(&job.hdl_out_dir)?;
    if verilog_files.is_empty() {
        return Err(XilinxError::ToolFailure {
            program: "vitis_hls".into(),
            code: 0,
            stderr: format!(
                "no HDL output produced in {}",
                job.hdl_out_dir.display()
            ),
        });
    }
    let report_paths = collect_files(&job.reports_out_dir)?;
    Ok(HlsOutput {
        csynth,
        verilog_files,
        report_paths,
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// One Vitis HLS invocation. Returns the parsed report + HDL output
/// on success and a typed `XilinxError::ToolFailure` on non-zero
/// exit. The HLS project tree lives under a dedicated stage dir that
/// is cleaned up on return; only the requested reports/HDL paths
/// survive.
pub fn run_hls(runner: &dyn ToolRunner, job: &HlsJob) -> Result<HlsOutput> {
    let stage = tempfile::tempdir()?;
    let out = run_hls_attempt(runner, job, stage.path())?;
    if out.exit_code != 0 {
        return Err(XilinxError::ToolFailure {
            program: "vitis_hls".into(),
            code: out.exit_code,
            stderr: out.stderr,
        });
    }
    harvest_and_stage(runner, job, stage.path(), out)
}

fn collect_files(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for ent in std::fs::read_dir(dir)? {
        let ent = ent?;
        if ent.file_type()?.is_file() {
            out.push(ent.path());
        }
    }
    out.sort();
    Ok(out)
}

/// Bounded retry wrapper keyed on the transient-failure predicate. The
/// default budget is 3 attempts; callers can override per job via
/// `transient_patterns`.
pub fn run_hls_with_retry(
    runner: &dyn ToolRunner,
    job: &HlsJob,
    max_attempts: u32,
) -> Result<HlsOutput> {
    let max_attempts = max_attempts.max(1);
    for _ in 0..max_attempts {
        let stage = tempfile::tempdir()?;
        let out = run_hls_attempt(runner, job, stage.path())?;
        if out.exit_code == 0 {
            return harvest_and_stage(runner, job, stage.path(), out);
        }
        // Python keys the retry decision off stdout alone; stderr is
        // preserved but intentionally ignored by the default predicate.
        let transient = is_transient(job, &out.stdout, &out.stderr);
        if !transient {
            return Err(XilinxError::ToolFailure {
                program: "vitis_hls".into(),
                code: out.exit_code,
                stderr: out.stderr,
            });
        }
    }
    Err(XilinxError::HlsRetryExhausted {
        attempts: max_attempts,
    })
}

/// Same as [`run_hls_with_retry`] but uses a caller-owned stage
/// directory instead of a per-attempt tempdir. Callers that honor
/// `--keep-hls-work-dir` pass a persistent path here so the Vitis
/// project / logs survive past `run_hls`. The directory is **not**
/// cleared between retries — caller is responsible for that (the
/// `run_hls_for_leaves` wrapper in `tapa-cli` clears it before
/// creation).
pub fn run_hls_with_retry_in_stage(
    runner: &dyn ToolRunner,
    job: &HlsJob,
    max_attempts: u32,
    stage_dir: &std::path::Path,
) -> Result<HlsOutput> {
    let max_attempts = max_attempts.max(1);
    for _ in 0..max_attempts {
        let out = run_hls_attempt(runner, job, stage_dir)?;
        if out.exit_code == 0 {
            return harvest_and_stage(runner, job, stage_dir, out);
        }
        let transient = is_transient(job, &out.stdout, &out.stderr);
        if !transient {
            return Err(XilinxError::ToolFailure {
                program: "vitis_hls".into(),
                code: out.exit_code,
                stderr: out.stderr,
            });
        }
    }
    Err(XilinxError::HlsRetryExhausted {
        attempts: max_attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::{MockToolRunner, ToolOutput};

    fn fixture_job(tmp: &std::path::Path) -> HlsJob {
        HlsJob {
            task_name: "k".into(),
            cpp_source: tmp.join("k.cpp"),
            cflags: vec!["-I/tmp/inc".into()],
            target_part: "xcu250-figd2104-2L-e".into(),
            top_name: "k".into(),
            clock_period: "3.33".into(),
            reports_out_dir: tmp.join("report"),
            hdl_out_dir: tmp.join("hdl"),
            ..HlsJob::default()
        }
    }

    #[test]
    fn tcl_contains_ported_steps() {
        let job = fixture_job(std::path::Path::new("/tmp"));
        let tcl = build_hls_tcl(&job);
        for step in [
            "open_project \"project\"",
            "set_top k",
            "open_solution \"k\"",
            "create_clock -period 3.33",
            "config_compile -name_max_length 253",
            "config_interface -m_axi_addr64",
            "config_rtl -reset_level low -module_auto_prefix",
            "csynth_design",
        ] {
            assert!(tcl.contains(step), "missing TCL step: {step}\nfull:\n{tcl}");
        }
    }

    #[test]
    fn tcl_body_does_not_bake_absolute_kernel_paths() {
        // Python-parity: the TCL template must iterate the
        // `TAPA_KERNEL_*` env entries instead of splicing absolute
        // `cpp_source` / cflags into the body. Baking absolute paths
        // makes the TCL non-portable to a remote rootfs.
        let mut job = fixture_job(std::path::Path::new("/tmp"));
        job.cpp_source = PathBuf::from("/abs/local/kernel/k.cpp");
        job.cflags = vec![
            "-I/abs/local/kernel/include".into(),
            "-DSOMETHING=1".into(),
        ];
        let tcl = build_hls_tcl(&job);
        assert!(
            !tcl.contains("/abs/local/kernel/k.cpp"),
            "TCL must not bake in the local kernel path: {tcl}"
        );
        assert!(
            !tcl.contains("-I/abs/local/kernel/include"),
            "TCL must not bake in the local include dir: {tcl}"
        );
        assert!(
            tcl.contains("TAPA_KERNEL_COUNT"),
            "TCL must iterate TAPA_KERNEL_* env entries: {tcl}"
        );
        assert!(
            tcl.contains("TAPA_KERNEL_PATH_"),
            "TCL must read per-index kernel path env: {tcl}"
        );
        assert!(
            tcl.contains("TAPA_KERNEL_CFLAGS_"),
            "TCL must read per-index cflags env: {tcl}"
        );
    }

    #[test]
    fn kernel_env_entries_mirror_python_contract() {
        let mut job = fixture_job(std::path::Path::new("/tmp"));
        job.cpp_source = PathBuf::from("/abs/src/k.cpp");
        job.cflags = vec!["-I/abs/inc".into(), "-DFOO".into()];
        let env = kernel_env_entries(&job);
        let lookup = |key: &str| {
            env.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(lookup("TAPA_KERNEL_COUNT"), "1");
        assert_eq!(lookup("TAPA_KERNEL_PATH_0"), "/abs/src/k.cpp");
        assert_eq!(lookup("TAPA_KERNEL_CFLAGS_0"), "-I/abs/inc -DFOO");
    }

    #[test]
    fn kernel_include_dirs_picks_abs_directories_only() {
        let td = tempfile::tempdir().unwrap();
        let existing = td.path().join("inc");
        std::fs::create_dir_all(&existing).unwrap();
        let cflags = vec![
            format!("-I{}", existing.display()),
            format!("-isystem{}", existing.display()),
            "-Irelative/should/be/ignored".into(),
            "-I/nonexistent/should/be/ignored".into(),
            "-DJUST_A_DEFINE".into(),
        ];
        let dirs = kernel_include_dirs(&cflags);
        assert_eq!(dirs.len(), 2);
        for d in &dirs {
            assert_eq!(d, &existing);
        }
    }

    #[test]
    fn run_hls_attempt_env_and_uploads_wire_kernel_metadata() {
        // Drive a MockToolRunner that records the ToolInvocation so
        // we can assert the upload list includes the source directory
        // and every include dir referenced by `-I/-isystem`, and that
        // the env carries the TAPA_KERNEL_* entries.
        let td = tempfile::tempdir().unwrap();
        let src_dir = td.path().join("src");
        let inc_dir = td.path().join("inc");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&inc_dir).unwrap();
        let src = src_dir.join("k.cpp");
        std::fs::write(&src, b"void k(){}").unwrap();

        let mut job = fixture_job(td.path());
        job.cpp_source = src.clone();
        job.cflags = vec![format!("-I{}", inc_dir.display())];

        let runner = MockToolRunner::new();
        runner.push_ok(
            "vitis_hls",
            ToolOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let stage = tempfile::tempdir().unwrap();
        let _ = run_hls_attempt(&runner, &job, stage.path());
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let inv = &calls[0];
        assert_eq!(inv.program, "vitis_hls");
        assert_eq!(inv.cwd.as_deref(), Some(stage.path()));
        assert_eq!(inv.env.get("TAPA_KERNEL_COUNT").map(String::as_str), Some("1"));
        assert_eq!(
            inv.env.get("TAPA_KERNEL_PATH_0").map(PathBuf::from),
            Some(src)
        );
        assert!(
            inv.env
                .get("TAPA_KERNEL_CFLAGS_0")
                .is_some_and(|c| c.contains(&format!("-I{}", inc_dir.display()))),
            "TAPA_KERNEL_CFLAGS_0 must carry the `-I<inc>` flag"
        );
        assert!(inv.uploads.contains(&src_dir), "src dir not uploaded");
        assert!(inv.uploads.contains(&inc_dir), "include dir not uploaded");
        assert!(
            inv.downloads.contains(&stage.path().to_path_buf()),
            "stage dir must be in downloads so remote HLS output lands locally"
        );
    }

    #[test]
    fn stderr_only_error_still_retries_when_stdout_transient() {
        // Reproduces Python: stderr-only "\nERROR:" does not cancel
        // the retry when stdout contains `Pre-synthesis failed.`.
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(tmp.path());
        let runner = MockToolRunner::new();
        for _ in 0..3 {
            runner.push_ok(
                "vitis_hls",
                ToolOutput {
                    exit_code: 1,
                    stdout: "Pre-synthesis failed.".into(),
                    stderr: "\nERROR: spurious stderr line".into(),
                },
            );
        }
        let err = run_hls_with_retry(&runner, &job, 3).unwrap_err();
        assert!(
            matches!(err, XilinxError::HlsRetryExhausted { attempts: 3 }),
            "expected retry budget to be exhausted, got {err:?}"
        );
    }

    #[test]
    fn production_transient_predicate_matches_python() {
        assert!(is_transient_hls_output("Pre-synthesis failed.\n", ""));
        // Plain failure with ERROR: is not transient — matches Python's
        // `b"\nERROR:" not in stdout` guard.
        assert!(!is_transient_hls_output(
            "Pre-synthesis failed.\nERROR: bad\n",
            ""
        ));
        assert!(!is_transient_hls_output("just a regular failure", ""));
    }

    #[test]
    fn retry_exhaustion_yields_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(tmp.path());
        let runner = MockToolRunner::new();
        for _ in 0..3 {
            runner.push_ok(
                "vitis_hls",
                ToolOutput {
                    exit_code: 1,
                    stdout: "Pre-synthesis failed.".into(),
                    stderr: String::new(),
                },
            );
        }
        let err = run_hls_with_retry(&runner, &job, 3).unwrap_err();
        assert!(matches!(err, XilinxError::HlsRetryExhausted { attempts: 3 }));
    }

    #[test]
    fn non_transient_failure_short_circuits() {
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(tmp.path());
        let runner = MockToolRunner::new();
        runner.push_ok(
            "vitis_hls",
            ToolOutput {
                exit_code: 2,
                stdout: String::new(),
                stderr: "Syntax error at line 42".into(),
            },
        );
        let err = run_hls_with_retry(&runner, &job, 3).unwrap_err();
        assert!(matches!(err, XilinxError::ToolFailure { code: 2, .. }));
    }

    /// `run_hls_with_retry_in_stage` uses the caller-provided dir
    /// without creating or deleting a tempdir — lets the `--keep-hls-work-dir`
    /// flow preserve the Vitis project after a failure.
    #[test]
    fn run_hls_with_retry_in_stage_reuses_caller_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let persistent = tmp.path().join("persistent-stage");
        std::fs::create_dir_all(&persistent).unwrap();
        // Put a marker file in the stage dir. After the retry loop
        // exhausts, the dir (and the marker) must still be present —
        // the Python `--keep-hls-work-dir` contract.
        let marker = persistent.join("MARKER");
        std::fs::write(&marker, b"before").unwrap();

        let job = fixture_job(tmp.path());
        let runner = MockToolRunner::new();
        for _ in 0..2 {
            runner.push_ok(
                "vitis_hls",
                ToolOutput {
                    exit_code: 1,
                    stdout: "Pre-synthesis failed.".into(),
                    stderr: String::new(),
                },
            );
        }
        let err = run_hls_with_retry_in_stage(&runner, &job, 2, &persistent)
            .unwrap_err();
        assert!(matches!(err, XilinxError::HlsRetryExhausted { attempts: 2 }));
        assert!(
            marker.is_file(),
            "in-stage retry must leave the caller-provided dir intact",
        );
    }
}
