//! `tapacc` semantic-analyzer invocation for `tapa analyze`.
//!
//! Drives the `tapacc` binary against the flattened sources and hands back
//! its raw JSON stdout. Parsing is the caller's job: `analyze` persists these
//! bytes verbatim as a debug artifact *before* interpreting them, so a
//! `tapacc` output that fails to parse is still on disk to look at.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{CliError, Result};
use crate::tapacc::{TAPACC_HLS_SHIM, TAPACC_HLS_SHIM_FILE};

/// Run `tapacc` and return its JSON stdout verbatim.
///
/// The analysis shim ([`TAPACC_HLS_SHIM`]) is written into `work_dir` and
/// force-included here so tapacc's clang can type-check Vitis HLS headers it
/// otherwise rejects. It is deliberately *not* part of the shared cflags: the
/// `tapa-cpp` flatten stage must never see the stub macros, or they would be
/// baked into the flattened source that goes to real synthesis.
pub(super) fn run_tapacc(
    tapacc: &Path,
    files: &[PathBuf],
    top: &str,
    cflags: &[String],
    target: &str,
    work_dir: &Path,
) -> Result<String> {
    let shim = work_dir.join(TAPACC_HLS_SHIM_FILE);
    fs::write(&shim, TAPACC_HLS_SHIM)?;

    let mut cmd = Command::new(tapacc);
    for f in files {
        cmd.arg(f);
    }
    cmd.args(["-top", top, "--target", target, "--"]);
    for f in cflags {
        cmd.arg(f);
    }
    cmd.arg("-include");
    cmd.arg(&shim);
    cmd.args(["-DTAPA_TARGET_DEVICE_", "-DTAPA_TARGET_STUB_"]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = cmd.output().map_err(|e| CliError::TapaccNotExecutable {
        path: tapacc.to_path_buf(),
        reason: e.to_string(),
    })?;
    if !output.status.success() {
        return Err(CliError::TapaccFailed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    // Non-fatal tapacc diagnostics (warnings, and the vendor-usage remarks
    // that suggest portable alternatives) reach the user; stdout stays the
    // machine-read JSON only.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    String::from_utf8(output.stdout)
        .map_err(|e| CliError::Codegen(format!("`tapacc` emitted non-UTF-8 output: {e}")))
}
