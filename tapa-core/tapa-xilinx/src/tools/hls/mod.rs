//! Vitis HLS orchestration.
//!
//! Implements TCL emission, invocation via a `ToolRunner`, report parsing,
//! and a bounded retry wrapper keyed on transient-failure substrings.

use std::sync::Arc;
use std::time::Duration;

use backon::{BlockingRetryable, ExponentialBuilder};
use camino::Utf8PathBuf;
use typed_builder::TypedBuilder;

use crate::error::{Result, XilinxError};
use crate::runtime::process::{ToolInvocation, ToolOutput, ToolRunner};
use crate::tools::hls::report::{parse_csynth_xml, CsynthReport};

pub mod report;

/// Substrings the default transient predicate keys off.
///
/// Kept for fixture-driven tests and custom predicates. The real
/// production predicate (`is_transient_hls_output`) treats a run as
/// transient when stdout contains `Pre-synthesis failed.` without a
/// subsequent `\nERROR:` line.
pub const DEFAULT_TRANSIENT_HLS_PATTERNS: &[&str] = &[
    "Pre-synthesis failed.",
    "TCP connection closed",
    "License checkout failed",
    "Connection reset by peer",
    "No license available",
    "FLEXnet Licensing error",
];

/// A Vitis HLS invocation is considered transient iff its stdout
/// contains `Pre-synthesis
/// failed.` and does **not** contain `\nERROR:`.
#[must_use]
pub fn is_transient_hls_output(stdout: &str, _stderr: &str) -> bool {
    stdout.contains("Pre-synthesis failed.") && !stdout.contains("\nERROR:")
}

#[derive(Clone, TypedBuilder)]
pub struct HlsJob {
    pub task_name: String,
    pub cpp_source: Utf8PathBuf,
    pub target_part: String,
    pub top_name: String,
    pub clock_period: String,
    pub reports_out_dir: Utf8PathBuf,
    pub hdl_out_dir: Utf8PathBuf,
    #[builder(default)]
    pub cflags: Vec<String>,
    /// Additional files the runner needs to stage up (remote tar-pipe
    /// uploads).
    #[builder(default)]
    pub uploads: Vec<Utf8PathBuf>,
    /// Files the runner must stage down after the tool exits.
    #[builder(default)]
    pub downloads: Vec<Utf8PathBuf>,
    /// Optional HLS `other_configs` TCL fragment. Appended verbatim.
    #[builder(default)]
    pub other_configs: String,
    /// Solution name; defaults to the task name when empty.
    #[builder(default)]
    pub solution_name: String,
    /// Reset level for `config_rtl`; defaults to `low`.
    #[builder(default = true)]
    pub reset_low: bool,
    /// Enable `-module_auto_prefix` on the `config_rtl` line. Defaults
    /// to `true`.
    #[builder(default = true)]
    pub auto_prefix: bool,
    /// Optional override. When `None`, the production
    /// `is_transient_hls_output` predicate is used.
    #[builder(default)]
    pub transient_patterns: Option<Arc<Vec<String>>>,
    /// Injectable delay function for retry backoff. Defaults to
    /// `std::thread::sleep` when `None`.
    #[builder(default)]
    pub delay_fn: Option<Arc<dyn Fn(Duration) + Send + Sync>>,
}

impl std::fmt::Debug for HlsJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HlsJob")
            .field("task_name", &self.task_name)
            .field("cpp_source", &self.cpp_source)
            .field("cflags", &self.cflags)
            .field("target_part", &self.target_part)
            .field("top_name", &self.top_name)
            .field("clock_period", &self.clock_period)
            .field("reports_out_dir", &self.reports_out_dir)
            .field("hdl_out_dir", &self.hdl_out_dir)
            .field("uploads", &self.uploads)
            .field("downloads", &self.downloads)
            .field("other_configs", &self.other_configs)
            .field("solution_name", &self.solution_name)
            .field("reset_low", &self.reset_low)
            .field("auto_prefix", &self.auto_prefix)
            .field("transient_patterns", &self.transient_patterns)
            .field("delay_fn", &self.delay_fn.is_some())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct HlsOutput {
    pub csynth: CsynthReport,
    pub verilog_files: Vec<Utf8PathBuf>,
    pub report_paths: Vec<Utf8PathBuf>,
    pub stdout: String,
    pub stderr: String,
}

/// Build the Vitis HLS TCL script for the given job.
///
/// Build the HLS command sequence: `open_project` → `set_top` →
/// `add_files` → `open_solution` → `set_part` → `create_clock` →
/// `config_compile` → `config_interface` → `{config}` →
/// `{other_configs}` → `config_rtl` → `csynth_design` → `exit`.
fn build_rtl_config(reset_low: bool, auto_prefix: bool) -> String {
    let mut line = format!(
        "config_rtl -reset_level {}",
        if reset_low { "low" } else { "high" }
    );
    if auto_prefix {
        // Vitis HLS accepts `-module_auto_prefix` here.
        line.push_str(" -module_auto_prefix");
    }
    line
}

/// Collect every `-I<dir>` / `-isystem<dir>` destination from the
/// job's CFLAGS.  Existing directories are uploaded verbatim so the
/// remote `vitis_hls` resolves sibling headers the same way the local
/// run would. Relative paths are absolutized against the current
/// working directory.
/// Handles both fused (`-I/dir`) and split (`-I`, `/dir`) forms.
fn kernel_include_dirs(cflags: &[String]) -> Vec<Utf8PathBuf> {
    let mut out: Vec<Utf8PathBuf> = Vec::new();
    let cwd = std::env::current_dir().ok();
    let mut i = 0;
    while i < cflags.len() {
        let trimmed = cflags[i].trim();
        let (dir_str, consumed) = if let Some(rest) = trimmed.strip_prefix("-isystem") {
            let rest = rest.trim();
            if rest.is_empty() && i + 1 < cflags.len() {
                (cflags[i + 1].trim(), 2)
            } else {
                (rest, 1)
            }
        } else if let Some(rest) = trimmed.strip_prefix("-I") {
            let rest = rest.trim();
            if rest.is_empty() && i + 1 < cflags.len() {
                (cflags[i + 1].trim(), 2)
            } else {
                (rest, 1)
            }
        } else {
            i += 1;
            continue;
        };
        if !dir_str.is_empty() {
            let p = Utf8PathBuf::from(dir_str);
            let p = if p.is_absolute() {
                p
            } else if let Some(ref cwd) = cwd {
                Utf8PathBuf::from_path_buf(cwd.join(p.as_std_path())).unwrap_or(p)
            } else {
                p
            };
            if p.is_dir() {
                out.push(p);
            }
        }
        i += consumed;
    }
    out
}

/// Kernel metadata passed through the
/// `TAPA_KERNEL_COUNT / TAPA_KERNEL_PATH_$i / TAPA_KERNEL_CFLAGS_$i`
/// environment contract. Keeping the
/// per-task paths in env entries (instead of baking them into the
/// TCL body) lets the remote runner rewrite them through its
/// rootfs-mirroring path-rewriter just like every other absolute
/// local path.
fn kernel_env_entries(job: &HlsJob) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    env.push(("TAPA_KERNEL_COUNT".into(), "1".into()));
    env.push((
        "TAPA_KERNEL_PATH_0".into(),
        job.cpp_source.as_str().to_string(),
    ));
    // Vitis `add_files -cflags` receives the value as a Tcl string,
    // not a shell command — shell quoting is treated literally and
    // breaks flags like `-D__builtin_FILE()=__FILE__`.
    let cflags = job.cflags.join(" ");
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
    let other = if job.other_configs.is_empty() {
        String::new()
    } else {
        format!("{}\n", job.other_configs)
    };
    let rtl = build_rtl_config(job.reset_low, job.auto_prefix);
    let mut env = minijinja::Environment::new();
    env.add_template("run_hls", include_str!("templates/run_hls.tcl.j2"))
        .expect("template parses");
    env.get_template("run_hls")
        .expect("template exists")
        .render(minijinja::context! {
            top => job.top_name,
            solution,
            part => job.target_part,
            clock => job.clock_period,
            other,
            rtl,
        })
        .expect("render succeeds")
}

fn is_transient(job: &HlsJob, stdout: &str, stderr: &str) -> bool {
    match job.transient_patterns.as_deref() {
        Some(v) => v
            .iter()
            .any(|p| stdout.contains(p.as_str()) || stderr.contains(p.as_str())),
        None => is_transient_hls_output(stdout, stderr),
    }
}

enum RetryError {
    Transient,
    Fatal(XilinxError),
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
    stage_dir: &camino::Utf8Path,
) -> Result<ToolOutput> {
    let tcl = build_hls_tcl(job);
    let tcl_path = stage_dir.join("run_hls.tcl");
    fs_err::write(&tcl_path, tcl.as_bytes())?;
    let mut inv = ToolInvocation::new("vitis_hls")
        .arg("-f")
        .arg(tcl_path.as_str());
    inv.cwd = Some(stage_dir.to_path_buf());
    // Pin `HOME` to the per-run stage dir. Vitis HLS otherwise writes shared
    // `~/.Xilinx` state that pollutes the workspace and races under
    // sandboxed/parallel Bazel builds. Using `inv.env` (vs
    // `Command::env`) lets the remote runner's path rewriter remap
    // the value to its rootfs counterpart.
    inv.env
        .insert("HOME".into(), stage_dir.as_str().to_string());
    // Uploads: TCL, the kernel source, every `-I` / `-isystem`
    // include directory referenced by the cflags, plus any caller extras.
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
fn copy_tree(src: &camino::Utf8Path, dest: &camino::Utf8Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let src_path = entry.path();
        let rel = src_path.strip_prefix(src).expect("prefix must match");
        let rel = Utf8PathBuf::from_path_buf(rel.to_path_buf())
            .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
        let dest_path = dest.join(&rel);
        if let Some(parent) = dest_path.parent() {
            fs_err::create_dir_all(parent)?;
        }
        fs_err::copy(src_path, dest_path)?;
    }
    Ok(())
}

fn harvest_and_stage(
    _runner: &dyn ToolRunner,
    job: &HlsJob,
    stage_dir: &camino::Utf8Path,
    out: ToolOutput,
) -> Result<HlsOutput> {
    // The runner already ensured the HLS project tree is on the
    // local filesystem (Local: it wrote directly under `cwd`; Remote:
    // `run_once` downloaded `cwd` back into place via the rootfs
    // mirror). We just need to copy the caller-facing slices out.
    let solution = solution_name(job);
    let syn_rel: Utf8PathBuf = ["project", &solution, "syn"].iter().collect();

    // Copy the real HLS artifacts into the caller-visible output dirs.
    let syn_abs = stage_dir.join(&syn_rel);
    fs_err::create_dir_all(&job.reports_out_dir)?;
    fs_err::create_dir_all(&job.hdl_out_dir)?;
    copy_tree(&syn_abs.join("report"), &job.reports_out_dir).map_err(|e| {
        XilinxError::HlsReportParse(format!(
            "stage reports {} → {}: {e}",
            syn_abs.join("report").as_str(),
            job.reports_out_dir.as_str()
        ))
    })?;
    copy_tree(&syn_abs.join("verilog"), &job.hdl_out_dir).map_err(|e| {
        XilinxError::HlsReportParse(format!(
            "stage verilog {} → {}: {e}",
            syn_abs.join("verilog").as_str(),
            job.hdl_out_dir.as_str()
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
    let bytes = fs_err::read(&report_xml).map_err(|_| {
        XilinxError::HlsReportParse(format!("missing csynth.xml at {}", report_xml.as_str()))
    })?;
    let csynth = parse_csynth_xml(&bytes)?;

    let verilog_files = collect_files(&job.hdl_out_dir)?;
    if verilog_files.is_empty() {
        return Err(XilinxError::ToolFailure {
            program: "vitis_hls".into(),
            code: 0,
            stderr: format!("no HDL output produced in {}", job.hdl_out_dir.as_str()),
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
    let stage_path = Utf8PathBuf::from_path_buf(stage.path().to_path_buf())
        .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned()));
    let out = run_hls_attempt(runner, job, &stage_path)?;
    if out.exit_code != 0 {
        let stderr = if out.stderr.is_empty() {
            out.stdout
        } else {
            out.stderr
        };
        return Err(XilinxError::ToolFailure {
            program: "vitis_hls".into(),
            code: out.exit_code,
            stderr,
        });
    }
    harvest_and_stage(runner, job, &stage_path, out)
}

fn collect_files(dir: &camino::Utf8Path) -> Result<Vec<Utf8PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for ent in fs_err::read_dir(dir)? {
        let ent = ent?;
        if ent.file_type()?.is_file() {
            out.push(
                Utf8PathBuf::from_path_buf(ent.path())
                    .unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())),
            );
        }
    }
    out.sort();
    Ok(out)
}

/// How the per-attempt staging directory is sourced.
#[derive(Debug, Clone, Copy)]
enum StageDir<'a> {
    /// Fresh tempdir created (and cleaned up) per retry attempt.
    Ephemeral,
    /// Caller-owned directory reused across retries; the caller owns cleanup.
    Borrowed(&'a camino::Utf8Path),
}

/// Bounded retry wrapper keyed on the transient-failure predicate. The
/// default budget is 3 attempts; callers can override per job via
/// `transient_patterns`.
fn run_hls_with_retry_impl(
    runner: &dyn ToolRunner,
    job: &HlsJob,
    max_attempts: u32,
    stage: StageDir<'_>,
) -> Result<HlsOutput> {
    let max_attempts = max_attempts.max(1);
    let backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(500))
        .with_max_delay(Duration::from_secs(30))
        .with_max_times(max_attempts.saturating_sub(1) as usize);

    let delay_fn = job.delay_fn.clone();

    let result = (|| -> std::result::Result<HlsOutput, RetryError> {
        // Resolve the stage path for this attempt. For `Ephemeral`, the
        // tempdir guard is kept alive in `_guard` for the whole closure
        // body so the directory exists during run + harvest.
        let (stage_path, _guard): (camino::Utf8PathBuf, Option<tempfile::TempDir>) = match &stage {
            StageDir::Ephemeral => {
                let dir = tempfile::tempdir().map_err(|e| RetryError::Fatal(XilinxError::Io(e)))?;
                let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
                    .unwrap_or_else(|p| {
                        camino::Utf8PathBuf::from(p.to_string_lossy().into_owned())
                    });
                (path, Some(dir))
            }
            StageDir::Borrowed(p) => (p.to_path_buf(), None),
        };
        let out = run_hls_attempt(runner, job, &stage_path).map_err(RetryError::Fatal)?;
        if out.exit_code == 0 {
            return harvest_and_stage(runner, job, &stage_path, out).map_err(RetryError::Fatal);
        }
        let transient = is_transient(job, &out.stdout, &out.stderr);
        if !transient {
            let stderr = if out.stderr.is_empty() {
                out.stdout
            } else {
                out.stderr
            };
            return Err(RetryError::Fatal(XilinxError::ToolFailure {
                program: "vitis_hls".into(),
                code: out.exit_code,
                stderr,
            }));
        }
        Err(RetryError::Transient)
    })
    .retry(backoff)
    .when(|err| matches!(err, RetryError::Transient))
    .sleep(move |dur| {
        if let Some(f) = &delay_fn {
            f(dur);
        } else {
            std::thread::sleep(dur);
        }
    })
    .call();

    match result {
        Ok(output) => Ok(output),
        Err(RetryError::Fatal(e)) => Err(e),
        Err(RetryError::Transient) => Err(XilinxError::HlsRetryExhausted {
            attempts: max_attempts,
        }),
    }
}

/// Run Vitis HLS with bounded retry; each attempt gets a fresh tempdir.
pub fn run_hls_with_retry(
    runner: &dyn ToolRunner,
    job: &HlsJob,
    max_attempts: u32,
) -> Result<HlsOutput> {
    run_hls_with_retry_impl(runner, job, max_attempts, StageDir::Ephemeral)
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
    stage_dir: &camino::Utf8Path,
) -> Result<HlsOutput> {
    run_hls_with_retry_impl(runner, job, max_attempts, StageDir::Borrowed(stage_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::{MockToolRunner, ToolOutput};
    use std::time::Duration;

    fn fixture_job(tmp: &camino::Utf8Path) -> HlsJob {
        HlsJob::builder()
            .task_name("k".into())
            .cpp_source(tmp.join("k.cpp"))
            .cflags(vec!["-I/tmp/inc".into()])
            .target_part("xcu250-figd2104-2L-e".into())
            .top_name("k".into())
            .clock_period("3.33".into())
            .reports_out_dir(tmp.join("report"))
            .hdl_out_dir(tmp.join("hdl"))
            .delay_fn(Some(Arc::new(|_| {})))
            .build()
    }

    #[test]
    fn tcl_contains_required_steps() {
        let job = fixture_job(camino::Utf8Path::new("/tmp"));
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
        // The TCL template must iterate the
        // `TAPA_KERNEL_*` env entries instead of splicing absolute
        // `cpp_source` / cflags into the body. Baking absolute paths
        // makes the TCL non-portable to a remote rootfs.
        let mut job = fixture_job(camino::Utf8Path::new("/tmp"));
        job.cpp_source = Utf8PathBuf::from("/abs/local/kernel/k.cpp");
        job.cflags = vec!["-I/abs/local/kernel/include".into(), "-DSOMETHING=1".into()];
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
    fn kernel_env_entries_mirror_current_contract() {
        let mut job = fixture_job(camino::Utf8Path::new("/tmp"));
        job.cpp_source = Utf8PathBuf::from("/abs/src/k.cpp");
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
    fn kernel_include_dirs_absolutizes_relative_dirs() {
        let td = tempfile::tempdir().unwrap();
        let existing = Utf8PathBuf::from_path_buf(td.path().join("inc")).unwrap();
        fs_err::create_dir_all(&existing).unwrap();
        let cflags = vec![
            format!("-I{}", existing.as_str()),
            format!("-isystem{}", existing.as_str()),
            // Relative paths are now absolutized against cwd so they can be
            // uploaded for remote HLS runs.
            "-Irelative/should/be/ignored".into(),
            "-I/nonexistent/should/be/ignored".into(),
            "-DJUST_A_DEFINE".into(),
            "-I".into(),
            existing.as_str().into(),
            "-isystem".into(),
            existing.as_str().into(),
        ];
        let dirs = kernel_include_dirs(&cflags);
        // 4 absolute + 1 absolutized relative (if it exists under cwd)
        // The relative one won't exist, so still 4.
        assert_eq!(dirs.len(), 4);
        for d in &dirs {
            assert_eq!(d, existing.as_path());
        }
    }

    #[test]
    fn run_hls_attempt_env_and_uploads_wire_kernel_metadata() {
        // Drive a MockToolRunner that records the ToolInvocation so
        // we can assert the upload list includes the source directory
        // and every include dir referenced by `-I/-isystem`, and that
        // the env carries the TAPA_KERNEL_* entries.
        let td = tempfile::tempdir().unwrap();
        let src_dir = Utf8PathBuf::from_path_buf(td.path().join("src")).unwrap();
        let inc_dir = Utf8PathBuf::from_path_buf(td.path().join("inc")).unwrap();
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&inc_dir).unwrap();
        let src = src_dir.join("k.cpp");
        fs_err::write(&src, b"void k(){}").unwrap();

        let mut job = fixture_job(&Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap());
        job.cpp_source = src.clone();
        job.cflags = vec![format!("-I{}", inc_dir.as_str())];

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
        let stage_path = Utf8PathBuf::from_path_buf(stage.path().to_path_buf()).unwrap();
        let _ = run_hls_attempt(&runner, &job, &stage_path);
        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        let inv = &calls[0];
        assert_eq!(inv.program, "vitis_hls");
        assert_eq!(inv.cwd.as_deref(), Some(stage_path.as_path()));
        assert_eq!(
            inv.env.get("TAPA_KERNEL_COUNT").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            inv.env.get("TAPA_KERNEL_PATH_0").map(Utf8PathBuf::from),
            Some(src)
        );
        assert!(
            inv.env
                .get("TAPA_KERNEL_CFLAGS_0")
                .is_some_and(|c| c.contains(&format!("-I{}", inc_dir.as_str()))),
            "TAPA_KERNEL_CFLAGS_0 must carry the `-I<inc>` flag"
        );
        assert!(inv.uploads.contains(&src_dir), "src dir not uploaded");
        assert!(inv.uploads.contains(&inc_dir), "include dir not uploaded");
        assert!(
            inv.downloads.contains(&stage_path),
            "stage dir must be in downloads so remote HLS output lands locally"
        );
    }

    #[test]
    fn stderr_only_error_still_retries_when_stdout_transient() {
        // Stderr-only "\nERROR:" does not cancel the retry when
        // stdout contains `Pre-synthesis failed.`.
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
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
    fn default_transient_predicate_classifies_failures() {
        assert!(is_transient_hls_output("Pre-synthesis failed.\n", ""));
        // Plain failure with ERROR: is not transient.
        assert!(!is_transient_hls_output(
            "Pre-synthesis failed.\nERROR: bad\n",
            ""
        ));
        assert!(!is_transient_hls_output("just a regular failure", ""));
    }

    #[test]
    fn retry_exhaustion_yields_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
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
        assert!(matches!(
            err,
            XilinxError::HlsRetryExhausted { attempts: 3 }
        ));
    }

    #[test]
    fn non_transient_failure_short_circuits() {
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
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
        let persistent = Utf8PathBuf::from_path_buf(tmp.path().join("persistent-stage")).unwrap();
        fs_err::create_dir_all(&persistent).unwrap();
        // Put a marker file in the stage dir. After the retry loop
        // exhausts, the dir (and the marker) must still be present —
        // the `--keep-hls-work-dir` contract.
        let marker = persistent.join("MARKER");
        fs_err::write(&marker, b"before").unwrap();

        let job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
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
        let err = run_hls_with_retry_in_stage(&runner, &job, 2, &persistent).unwrap_err();
        assert!(matches!(
            err,
            XilinxError::HlsRetryExhausted { attempts: 2 }
        ));
        assert!(
            marker.is_file(),
            "in-stage retry must leave the caller-provided dir intact",
        );
    }

    #[test]
    fn retry_budget_exhausted_with_zero_delay() {
        let tmp = tempfile::tempdir().unwrap();
        let job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
        let runner = MockToolRunner::new();
        for _ in 0..5 {
            runner.push_ok(
                "vitis_hls",
                ToolOutput {
                    exit_code: 1,
                    stdout: "Pre-synthesis failed.".into(),
                    stderr: String::new(),
                },
            );
        }
        let start = std::time::Instant::now();
        let err = run_hls_with_retry(&runner, &job, 5).unwrap_err();
        let elapsed = start.elapsed();
        assert!(matches!(
            err,
            XilinxError::HlsRetryExhausted { attempts: 5 }
        ));
        assert!(
            elapsed < Duration::from_millis(100),
            "retry loop must not insert delays, took {elapsed:?}"
        );
    }

    #[test]
    fn custom_transient_patterns_match_both_stdout_and_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let mut job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
        job.transient_patterns = Some(Arc::new(vec!["custom-pattern".into()]));
        let runner = MockToolRunner::new();
        for _ in 0..3 {
            runner.push_ok(
                "vitis_hls",
                ToolOutput {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: "custom-pattern".into(),
                },
            );
        }
        let err = run_hls_with_retry(&runner, &job, 3).unwrap_err();
        assert!(matches!(
            err,
            XilinxError::HlsRetryExhausted { attempts: 3 }
        ));
    }

    #[test]
    fn cflags_with_spaces_preserved_in_kernel_env() {
        let tmp = tempfile::tempdir().unwrap();
        let mut job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
        job.cflags = vec!["-I/tmp/inc".into(), "-DMSG=\"hello world\"".into()];
        let env = kernel_env_entries(&job);
        let cflags = env
            .iter()
            .find(|(k, _)| k == "TAPA_KERNEL_CFLAGS_0")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(
            cflags.contains("-DMSG=\"hello world\""),
            "spaces preserved: {cflags}"
        );
    }

    #[test]
    fn cflags_path_with_spaces_not_shell_quoted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
        // Fused form: include path contains spaces — we do NOT shell-quote
        // because Vitis `add_files -cflags` is a Tcl string, not a shell
        // command.  Quoting would be treated literally and break compilation.
        job.cflags = vec!["-I/path with spaces/include".into()];
        let env = kernel_env_entries(&job);
        let cflags = env
            .iter()
            .find(|(k, _)| k == "TAPA_KERNEL_CFLAGS_0")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(
            cflags.contains("-I/path with spaces/include"),
            "raw join (no shell quoting): {cflags}"
        );
    }

    #[test]
    fn cflags_split_include_with_spaces_not_shell_quoted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
        // Split form: -I and path are separate arguments — joined raw.
        job.cflags = vec!["-I".into(), "/path with spaces/include".into()];
        let env = kernel_env_entries(&job);
        let cflags = env
            .iter()
            .find(|(k, _)| k == "TAPA_KERNEL_CFLAGS_0")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert!(
            cflags.contains("-I /path with spaces/include"),
            "raw join (no shell quoting): {cflags}"
        );
    }

    #[test]
    fn cflags_single_quotes_not_shell_escaped() {
        let tmp = tempfile::tempdir().unwrap();
        let mut job = fixture_job(&Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap());
        job.cflags = vec!["-DMSG='hello'".into()];
        let env = kernel_env_entries(&job);
        let cflags = env
            .iter()
            .find(|(k, _)| k == "TAPA_KERNEL_CFLAGS_0")
            .map(|(_, v)| v.clone())
            .unwrap();
        // Raw join — no shell escaping because Vitis Tcl does not shell-parse.
        assert!(
            cflags.contains("-DMSG='hello'"),
            "raw join (no shell quoting): {cflags}"
        );
    }
}
