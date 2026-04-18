//! Vivado TCL runner.
//!
//! Ports `tapa/backend/xilinx_tools.py::Vivado` — `vivado -mode batch
//! -source <tcl>`. The orchestrator skeleton is wired up; the live TCL
//! emission for `package_xo` lands with the `.xo` packaging module.

use std::path::PathBuf;

use crate::error::Result;
use crate::runtime::process::{ToolInvocation, ToolRunner};

#[derive(Debug, Clone)]
pub struct VivadoJob {
    pub tcl: String,
    pub uploads: Vec<PathBuf>,
    pub downloads: Vec<PathBuf>,
    pub work_dir: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// `-tclargs` forwarded to the TCL script. Ports Python's
    /// `tapa/backend/xilinx_tools.py::Vivado` trailing tclargs list.
    pub tclargs: Vec<String>,
}

impl VivadoJob {
    pub fn new(tcl: impl Into<String>) -> Self {
        Self {
            tcl: tcl.into(),
            uploads: Vec::new(),
            downloads: Vec::new(),
            work_dir: None,
            env: Vec::new(),
            tclargs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VivadoOutput {
    pub stdout: String,
    pub stderr: String,
    pub produced: Vec<PathBuf>,
}

pub fn build_invocation(job: &VivadoJob, tcl_path: &std::path::Path) -> ToolInvocation {
    let mut inv = ToolInvocation::new("vivado")
        .arg("-mode")
        .arg("batch")
        .arg("-source")
        .arg(tcl_path.display().to_string())
        .arg("-nojournal")
        .arg("-nolog");
    if !job.tclargs.is_empty() {
        inv = inv.arg("-tclargs");
        for a in &job.tclargs {
            inv = inv.arg(a.clone());
        }
    }
    for (k, v) in &job.env {
        inv = inv.env(k.clone(), v.clone());
    }
    if let Some(cwd) = job.work_dir.clone() {
        inv.cwd = Some(cwd);
    }
    inv.uploads = job.uploads.clone();
    inv.downloads = job.downloads.clone();
    inv
}

/// Invoke Vivado via the provided runner. Writes the TCL script into a
/// tempfile on the local side and points `vivado -source` at it.
///
/// When `job.work_dir` is unset the runner allocates a per-call
/// `tempfile::TempDir` and uses it as both `cwd` and `HOME`. Mirrors
/// the Python `Vivado` wrapper that always created a temp cwd and
/// pinned `HOME` there — Vivado otherwise writes `~/.Xilinx` state
/// into the caller's home dir, which breaks under sandboxed or
/// unwritable homes (e.g. Bazel exec) and races between parallel
/// runs.
pub fn run_vivado(runner: &dyn ToolRunner, job: &VivadoJob) -> Result<VivadoOutput> {
    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp.path(), job.tcl.as_bytes())?;
    let scratch = if job.work_dir.is_none() {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    let mut inv = build_invocation(job, tmp.path());
    inv.uploads.push(tmp.path().to_path_buf());
    let home_dir = match (&job.work_dir, &scratch) {
        (Some(p), _) => p.clone(),
        (None, Some(t)) => {
            let p = t.path().to_path_buf();
            inv.cwd = Some(p.clone());
            p
        }
        (None, None) => unreachable!("scratch tempdir is allocated when work_dir is None"),
    };
    inv.env
        .insert("HOME".into(), home_dir.display().to_string());
    let out = runner.run(&inv)?;
    if out.exit_code != 0 {
        return Err(crate::error::XilinxError::ToolFailure {
            program: "vivado".into(),
            code: out.exit_code,
            stderr: out.stderr,
        });
    }
    Ok(VivadoOutput {
        stdout: out.stdout,
        stderr: out.stderr,
        produced: job.downloads.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::process::{MockToolRunner, ToolOutput};

    #[test]
    fn run_vivado_builds_expected_invocation() {
        let runner = MockToolRunner::new();
        runner.push_ok("vivado", ToolOutput::default());
        let job = VivadoJob::new("puts hi\nexit");
        run_vivado(&runner, &job).unwrap();
        let call = &runner.calls()[0];
        assert_eq!(call.program, "vivado");
        assert!(call.args.contains(&"-mode".to_string()));
        assert!(call.args.contains(&"batch".to_string()));
        assert!(call.args.contains(&"-source".to_string()));
    }

    #[test]
    fn run_vivado_surfaces_tool_failure() {
        let runner = MockToolRunner::new();
        runner.push_ok(
            "vivado",
            ToolOutput {
                exit_code: 1,
                stdout: String::new(),
                stderr: "bad TCL".into(),
            },
        );
        let err = run_vivado(&runner, &VivadoJob::new("exit 1")).unwrap_err();
        assert!(matches!(
            err,
            crate::error::XilinxError::ToolFailure { code: 1, .. }
        ));
    }
}
