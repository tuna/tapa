pub mod aie;
pub mod hls;
pub mod package_xo;
pub mod slash;
pub mod vitis;
pub mod vivado;

use crate::error::XilinxError;
use crate::runtime::process::ToolOutput;
use camino::Utf8PathBuf;
use vitis::timing::KernelTiming;

/// Artifacts and kernel-clock measurements produced by one hardware link.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkOutput {
    pub bitstream: Utf8PathBuf,
    pub requested_mhz: u32,
    pub recorded_mhz: f64,
    pub timing: KernelTiming,
    pub timing_report: Utf8PathBuf,
    pub stdout: String,
    pub stderr: String,
}

/// Map a failed (non-zero-exit) tool run into a
/// [`XilinxError::ToolFailure`], folding the output streams so the
/// diagnostics survive whichever stream the tool logged to.
fn tool_failure(program: &str, out: ToolOutput) -> XilinxError {
    XilinxError::ToolFailure {
        program: program.into(),
        code: out.exit_code,
        stderr: merged_failure_output(out.stdout, out.stderr),
    }
}

fn merged_failure_output(stdout: String, stderr: String) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, _) => stderr,
        (_, true) => stdout,
        (false, false) => format!("stdout:\n{stdout}\nstderr:\n{stderr}"),
    }
}
