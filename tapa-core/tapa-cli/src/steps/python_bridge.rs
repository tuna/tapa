//! Transitional Python fallback bridge.
//!
//! Delegates un-ported steps to the stock click entry point via
//! `python -m tapa.__main__ <global flags> <step> <step flags>`. State
//! handoff happens via the on-disk JSON files under `work_dir`. The
//! bridge is per-step opt-in behind `TAPA_STEP_<NAME>_PYTHON=1` and is
//! deleted once all steps run natively.

#![allow(
    clippy::wildcard_enum_match_arm,
    reason = "test cases match the typed error variant under inspection; the \
              wildcard surfaces real regressions, not stylistic noise"
)]

use std::path::Path;
use std::process::Command;

use crate::context::CliContext;
use crate::error::{CliError, Result};

/// Convert a step name into the `TAPA_STEP_<UPPER>_PYTHON` env-var key.
/// Hyphens map to underscores so `compile-with-floorplan-dse` →
/// `COMPILE_WITH_FLOORPLAN_DSE`.
pub fn flag_name(step: &str) -> String {
    step.chars()
        .map(|c| match c {
            '-' | '+' => '_',
            other => other.to_ascii_uppercase(),
        })
        .collect()
}

/// True when the user has opted into the Python fallback for `step`
/// via `TAPA_STEP_<step>_PYTHON=1`.
pub fn is_enabled(step: &str) -> bool {
    let var = format!("TAPA_STEP_{}_PYTHON", flag_name(step));
    std::env::var(var).map(|v| v == "1").unwrap_or(false)
}

/// Refuse to run a step that lacks a native impl AND has no bridge flag set.
/// Matches the negative test in AC-6.
pub fn require_enabled(step: &str) -> Result<()> {
    if is_enabled(step) {
        Ok(())
    } else {
        Err(CliError::StepUnported {
            step: step.to_string(),
            flag_name: flag_name(step),
        })
    }
}

/// Invoke `python -m tapa.__main__ <globals> <step> <argv>` with the work
/// directory wired up. Surfaces non-zero exit as
/// [`CliError::PythonBridge`].
pub fn run(step: &str, argv: &[String], ctx: &CliContext) -> Result<()> {
    let python = std::env::var("TAPA_PYTHON").unwrap_or_else(|_| "python3".to_string());
    let mut cmd = Command::new(&python);
    cmd.arg("-m").arg("tapa.__main__");
    for arg in globals_to_argv(ctx) {
        cmd.arg(arg);
    }
    cmd.arg(step);
    for a in argv {
        cmd.arg(a);
    }
    let status = cmd
        .status()
        .map_err(|e| CliError::PythonBridgeLaunch(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::PythonBridge {
            step: step.to_string(),
            code: status.code().unwrap_or(-1),
            stderr: format!(
                "subprocess `{python} -m tapa.__main__ {step}` exited {status}"
            ),
        })
    }
}

/// Render the relevant subset of `CliContext` back into the click flag
/// shape Python expects. Order matches `entry_point`'s click options.
fn globals_to_argv(ctx: &CliContext) -> Vec<String> {
    let mut out = Vec::<String>::new();
    out.push("--work-dir".to_string());
    out.push(ctx.work_dir.display().to_string());
    if let Some(temp) = &ctx.temp_dir {
        out.push("--temp-dir".to_string());
        out.push(temp.display().to_string());
    }
    out.push("--clang-format-quota-in-bytes".to_string());
    out.push(ctx.options.clang_format_quota_in_bytes.to_string());
    push_opt(&mut out, "--remote-host", ctx.remote.host.as_deref());
    push_opt(&mut out, "--remote-key-file", ctx.remote.key_file.as_deref());
    push_opt(
        &mut out,
        "--remote-xilinx-settings",
        ctx.remote.xilinx_settings.as_deref(),
    );
    push_opt(
        &mut out,
        "--remote-ssh-control-dir",
        ctx.remote.ssh_control_dir.as_deref(),
    );
    push_opt(
        &mut out,
        "--remote-ssh-control-persist",
        ctx.remote.ssh_control_persist.as_deref(),
    );
    if ctx.remote.disable_ssh_mux {
        out.push("--remote-disable-ssh-mux".to_string());
    }
    out
}

fn push_opt(out: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push(flag.to_string());
        out.push(v.to_string());
    }
}

/// Test helper: probe whether the given work directory has the artifacts
/// the bridged Python step would write so callers can skip a redundant
/// run. Currently unused at runtime; kept for future rounds.
pub fn _has_artifact(work_dir: &Path, name: &str) -> bool {
    work_dir.join(name).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_name_uppercases_and_normalizes_hyphen() {
        assert_eq!(flag_name("synth"), "SYNTH");
        assert_eq!(flag_name("compile-with-floorplan-dse"), "COMPILE_WITH_FLOORPLAN_DSE");
        assert_eq!(flag_name("g++"), "G__");
    }

    #[test]
    fn unported_without_flag_errors() {
        // We don't set the env var, so require_enabled must refuse.
        std::env::remove_var("TAPA_STEP_TESTONLY_PYTHON");
        let err = require_enabled("testonly").unwrap_err();
        match err {
            CliError::StepUnported { step, flag_name } => {
                assert_eq!(step, "testonly");
                assert_eq!(flag_name, "TESTONLY");
            }
            other => panic!("expected StepUnported, got {other:?}"),
        }
    }

    #[test]
    fn enabled_when_env_set() {
        // Use a unique var name to avoid contamination from other tests.
        std::env::set_var("TAPA_STEP_BRIDGECHECK_PYTHON", "1");
        assert!(is_enabled("bridgecheck"));
        require_enabled("bridgecheck").expect("bridge enabled by env var");
        std::env::remove_var("TAPA_STEP_BRIDGECHECK_PYTHON");
    }
}
