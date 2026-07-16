//! `tapacc` semantic-analyzer invocation for `tapa analyze`.
//!
//! Drives the `tapacc` binary against the flattened sources and hands back
//! its raw JSON stdout. Parsing is the caller's job: `analyze` persists these
//! bytes verbatim as a debug artifact *before* interpreting them, so a
//! `tapacc` output that fails to parse is still on disk to look at.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{CliError, Result};

/// Run `tapacc` and return its JSON stdout verbatim.
pub(super) fn run_tapacc(
    tapacc: &Path,
    files: &[PathBuf],
    top: &str,
    cflags: &[String],
    target: &str,
) -> Result<String> {
    let mut cmd = Command::new(tapacc);
    for f in files {
        cmd.arg(f);
    }
    cmd.args(["-top", top, "--target", target, "--"]);
    for f in cflags {
        cmd.arg(f);
    }
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
    String::from_utf8(output.stdout)
        .map_err(|e| CliError::InvalidArg(format!("`tapacc` emitted non-UTF-8 output: {e}")))
}
