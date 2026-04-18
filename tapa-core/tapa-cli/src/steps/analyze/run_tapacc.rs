//! `tapacc` semantic-analyzer invocation for `tapa analyze`.
//!
//! Drives the `tapacc` binary against the flattened sources and parses
//! its JSON stdout into a `serde_json::Value` (the on-disk graph
//! schema).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::{CliError, Result};

/// Run `tapacc` and parse its JSON stdout.
pub(super) fn run_tapacc(
    tapacc: &Path,
    files: &[PathBuf],
    top: &str,
    cflags: &[String],
    target: &str,
) -> Result<Value> {
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
    let value: Value = serde_json::from_slice(&output.stdout)?;
    Ok(value)
}
