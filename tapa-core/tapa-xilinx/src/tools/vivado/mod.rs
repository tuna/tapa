//! Vivado TCL runner for `vivado -mode batch -source <tcl>`.

use camino::Utf8PathBuf;

use crate::error::Result;
use crate::runtime::process::{ToolInvocation, ToolRunner};

#[derive(Debug, Clone)]
pub struct VivadoJob {
    pub tcl: String,
    pub uploads: Vec<Utf8PathBuf>,
    pub downloads: Vec<Utf8PathBuf>,
    pub work_dir: Option<Utf8PathBuf>,
    pub env: Vec<(String, String)>,
    /// Arguments forwarded to the TCL script after `-tclargs`.
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
    pub produced: Vec<Utf8PathBuf>,
}

pub(crate) fn build_invocation(job: &VivadoJob, tcl_path: &camino::Utf8Path) -> ToolInvocation {
    let mut inv = ToolInvocation::new("vivado")
        .arg("-mode")
        .arg("batch")
        .arg("-source")
        .arg(tcl_path.as_str())
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
/// When `job.work_dir` is unset, the runner allocates a per-call
/// temporary directory and uses it as both `cwd` and `HOME`. Vivado
/// otherwise writes `~/.Xilinx` state
/// into the caller's home dir, which breaks under sandboxed or
/// unwritable homes (e.g. Bazel exec) and races between parallel
/// runs.
pub fn run_vivado(runner: &dyn ToolRunner, job: &VivadoJob) -> Result<VivadoOutput> {
    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp.path(), job.tcl.as_bytes())?;
    let tmp_path = crate::util::utf8(tmp.path());
    let scratch = if job.work_dir.is_none() {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    let mut inv = build_invocation(job, &tmp_path);
    inv.uploads.push(tmp_path);
    let home_dir = match (&job.work_dir, &scratch) {
        (Some(p), _) => p.clone(),
        (None, Some(t)) => {
            let p = crate::util::utf8(t.path());
            inv.cwd = Some(p.clone());
            p
        }
        (None, None) => unreachable!("scratch tempdir is allocated when work_dir is None"),
    };
    inv.env.insert("HOME".into(), home_dir.as_str().to_string());
    let out = runner.run(&inv)?;
    if out.exit_code != 0 {
        return Err(super::tool_failure("vivado", out));
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
                stdout: "TCL command context".into(),
                stderr: "bad TCL".into(),
            },
        );
        let err = run_vivado(&runner, &VivadoJob::new("exit 1")).unwrap_err();
        let crate::error::XilinxError::ToolFailure {
            code,
            stderr: output,
            ..
        } = err
        else {
            panic!("expected tool failure");
        };
        assert_eq!(code, 1);
        assert!(output.contains("stdout:\nTCL command context"));
        assert!(output.contains("stderr:\nbad TCL"));
    }
}
