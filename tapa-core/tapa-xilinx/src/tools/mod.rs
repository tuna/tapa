pub mod aie;
pub mod hls;
pub mod package_xo;
pub mod slash;
pub mod vitis;
pub mod vivado;

use crate::error::{Result, XilinxError};
use crate::runtime::process::ToolOutput;
use camino::{Utf8Path, Utf8PathBuf};
use vitis::timing::KernelTiming;

/// Artifacts and kernel-clock measurements produced by one link.
///
/// Hardware links carry the routed kernel timing and the report it was read
/// from; simulation links produce no routed timing report, so both fields are
/// `None` for them.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkOutput {
    pub bitstream: Utf8PathBuf,
    pub requested_mhz: u32,
    pub recorded_mhz: f64,
    pub timing: Option<KernelTiming>,
    pub timing_report: Option<Utf8PathBuf>,
    pub stdout: String,
    pub stderr: String,
}

impl LinkOutput {
    /// The routed kernel timing, which only a hardware link produces.
    ///
    /// Consumers that rank or publish implementations (floorplan DSE, the
    /// implementation metrics) require concrete hardware timing; a simulation
    /// link gets an error here instead of a sentinel value.
    pub fn hardware_timing(&self) -> Result<&KernelTiming> {
        self.timing
            .as_ref()
            .ok_or(XilinxError::MissingHardwareTiming)
    }

    /// The routed timing report the kernel timing was read from, which only a
    /// hardware link produces.
    pub fn hardware_timing_report(&self) -> Result<&Utf8Path> {
        self.timing_report
            .as_deref()
            .ok_or(XilinxError::MissingHardwareTiming)
    }
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
