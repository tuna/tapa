//! `tapa floorplan` and `tapa run-autobridge` — clap parity with
//! `tapa/steps/floorplan.py`.
//!
//! The native paths cover the local-only happy paths:
//!   * `floorplan` without `--floorplan-path` is a stateful no-op that
//!     toggles `settings["floorplan"] = true` and marks the step as
//!     pipelined; the heavy `--floorplan-path` orchestration (which
//!     requires the Python `tapa.common.graph.Graph::get_floorplan_graph`
//!     transform) still routes through the bridge.
//!   * `run-autobridge` shells out to `rapidstream-tapafp`. When no
//!     `--remote-host` is configured we drive the subprocess directly;
//!     remote execution still needs the tar-pipe orchestration that
//!     lives in `tapa-xilinx`, so it falls back to the bridge.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use serde_json::{json, Value};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{settings as settings_io};
use crate::steps::python_bridge;

const AUTOBRIDGE_WORK_DIR: &str = "autobridge";
const FLOORPLAN_CONFIG_NO_PRE_ASSIGNMENTS: &str = "floorplan_config_no_pre_assignments.json";
const RAPIDSTREAM_TAPAFP_BIN: &str = "rapidstream-tapafp";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "floorplan",
    about = "Floorplan TAPA program and store the program description."
)]
pub struct FloorplanArgs {
    #[arg(long = "floorplan-path", value_name = "FILE")]
    pub floorplan_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "run-autobridge",
    about = "Run the autobridge tool to generate a floorplan."
)]
pub struct RunAutobridgeArgs {
    /// Path to the device configuration file.
    #[arg(long = "device-config", value_name = "FILE", required = true)]
    pub device_config: PathBuf,

    /// Path to the floorplan configuration file.
    #[arg(long = "floorplan-config", value_name = "FILE", required = true)]
    pub floorplan_config: PathBuf,
}

pub fn to_python_argv_floorplan(args: &FloorplanArgs) -> Vec<String> {
    let mut out = Vec::<String>::new();
    if let Some(p) = &args.floorplan_path {
        out.push("--floorplan-path".to_string());
        out.push(p.display().to_string());
    }
    out
}

/// Render the autobridge args back to Python click flags.
///
/// Used by composites (`generate-floorplan`,
/// `compile-with-floorplan-dse`) when forwarding through the bridge —
/// `run-autobridge` itself has no top-level Python CLI entry, so this
/// helper exists for the parent composite's argv builder.
pub fn to_python_argv_run_autobridge(args: &RunAutobridgeArgs) -> Vec<String> {
    vec![
        "--device-config".to_string(),
        args.device_config.display().to_string(),
        "--floorplan-config".to_string(),
        args.floorplan_config.display().to_string(),
    ]
}


/// `tapa floorplan` dispatcher.
///
/// Routes to the Python bridge when explicitly opted in or when
/// `--floorplan-path` is provided (the floorplan-graph transform still
/// lives in Python). Otherwise executes the native no-op that just
/// toggles `settings["floorplan"] = true`.
pub fn run_floorplan(args: &FloorplanArgs, ctx: &mut CliContext) -> Result<()> {
    if python_bridge::is_enabled("floorplan") {
        return python_bridge::run("floorplan", &to_python_argv_floorplan(args), ctx);
    }
    if args.floorplan_path.is_some() {
        return Err(CliError::InvalidArg(
            "`--floorplan-path` is not yet supported by the native floorplan step; \
             rerun with `TAPA_STEP_FLOORPLAN_PYTHON=1` to use the Python fallback."
                .to_string(),
        ));
    }
    run_floorplan_native_noop(ctx)
}

/// Native no-op path: load (or initialize) `settings.json`, set
/// `floorplan = true`, persist, and mark the step as pipelined.
fn run_floorplan_native_noop(ctx: &CliContext) -> Result<()> {
    let work_dir = ctx.work_dir.as_path();
    let mut flow = ctx.flow.borrow_mut();
    let mut settings = if let Some(s) = flow.settings.take() {
        s
    } else {
        // Fall back to disk if an upstream step in this process did not
        // populate the in-memory cache (e.g. user invoked `tapa floorplan`
        // standalone after a prior `tapa analyze`).
        match settings_io::load_settings(work_dir) {
            Ok(s) => s,
            Err(CliError::MissingState { .. }) => settings_io::Settings::new(),
            Err(other) => return Err(other),
        }
    };
    settings.insert("floorplan".to_string(), Value::Bool(true));
    settings_io::store_settings(work_dir, &settings)?;
    flow.settings = Some(settings);
    flow.pipelined.insert("floorplan".to_string(), true);
    Ok(())
}

/// `tapa run-autobridge` dispatcher.
///
/// Note: the Python click CLI does NOT register a top-level
/// `run-autobridge` subcommand — `tapa.steps.floorplan.run_autobridge`
/// is only reachable from inside `tapa.steps.meta.generate_floorplan_entry`
/// via `forward_applicable`. There is therefore no working bridge
/// fallback for this step (`python -m tapa.__main__ run-autobridge`
/// fails with "No such command"), so we always run the native path
/// (or surface a typed error for the remote case which still needs
/// `tapa-xilinx` SSH transport).
pub fn run_run_autobridge(
    args: &RunAutobridgeArgs,
    ctx: &mut CliContext,
) -> Result<()> {
    if ctx.remote.host.is_some() {
        // Remote execution still needs the tar-pipe orchestration in
        // `tapa-xilinx`, which `tapa-cli` does not depend on. Surface a
        // clear opt-in so the user knows what's missing.
        return Err(CliError::InvalidArg(
            "remote `run-autobridge` is not yet supported by `tapa-cli`; \
             native paths cover the local-only case. Run from a host with \
             `rapidstream-tapafp` installed locally, or use the Python CLI \
             (`tapa generate-floorplan`) which embeds the autobridge step."
                .to_string(),
        ));
    }
    run_autobridge_native(args, ctx)
}

fn run_autobridge_native(args: &RunAutobridgeArgs, ctx: &CliContext) -> Result<()> {
    let work_dir = ctx.work_dir.as_path();
    let ab_graph_path = work_dir.join("ab_graph.json");
    let autobridge_dir = work_dir.join(AUTOBRIDGE_WORK_DIR);
    fs::create_dir_all(&autobridge_dir)?;

    // Strip pre-assignments from the floorplan config and persist the
    // sanitized variant. Mirrors the Python preprocessing step.
    let raw = fs::read_to_string(&args.floorplan_config)?;
    let mut config: Value = serde_json::from_str(&raw)?;
    if let Some(obj) = config.as_object_mut() {
        obj.remove("sys_port_pre_assignments");
        obj.remove("cpp_arg_pre_assignments");
        obj.insert("port_pre_assignments".to_string(), json!({}));
    }
    let sanitized_path = autobridge_dir.join(FLOORPLAN_CONFIG_NO_PRE_ASSIGNMENTS);
    fs::write(&sanitized_path, serde_json::to_vec(&config)?)?;

    let mut cmd = Command::new(RAPIDSTREAM_TAPAFP_BIN);
    cmd.args([
        "--ab-graph-path",
        ab_graph_path.display().to_string().as_str(),
        "--work-dir",
        autobridge_dir.display().to_string().as_str(),
        "--device-config",
        args.device_config.display().to_string().as_str(),
        "--floorplan-config",
        sanitized_path.display().to_string().as_str(),
        "--run-floorplan",
    ]);
    let status = cmd.status().map_err(|e| {
        CliError::TapaccNotExecutable {
            path: PathBuf::from(RAPIDSTREAM_TAPAFP_BIN),
            reason: e.to_string(),
        }
    })?;
    if !status.success() {
        return Err(CliError::TapaccFailed {
            code: status.code().unwrap_or(-1),
            stderr: format!("{RAPIDSTREAM_TAPAFP_BIN} failed"),
        });
    }

    log_solution_floorplans(&autobridge_dir);
    Ok(())
}

fn log_solution_floorplans(autobridge_dir: &Path) {
    let Ok(entries) = fs::read_dir(autobridge_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("solution_") {
            continue;
        }
        let candidate = entry.path().join("floorplan.json");
        if candidate.exists() {
            log::info!("Generated floorplan file: {}", candidate.display());
        }
    }
}

/// Enumerate `<work_dir>/autobridge/solution_*/floorplan.json` files in a
/// stable order. Used by the DSE composite to drive Stage 2.
pub fn enumerate_solution_floorplans(work_dir: &Path) -> Result<Vec<PathBuf>> {
    let autobridge_dir = work_dir.join(AUTOBRIDGE_WORK_DIR);
    if !autobridge_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::<PathBuf>::new();
    for entry in fs::read_dir(&autobridge_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with("solution_") {
            continue;
        }
        let candidate = entry.path().join("floorplan.json");
        if candidate.exists() {
            out.push(candidate);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::GlobalArgs;

    fn ctx_with_work_dir(work_dir: &Path) -> CliContext {
        let globals = GlobalArgs {
            verbose: 0,
            quiet: 0,
            work_dir: work_dir.to_path_buf(),
            temp_dir: None,
            clang_format_quota_in_bytes: 0,
            remote_host: None,
            remote_key_file: None,
            remote_xilinx_settings: None,
            remote_ssh_control_dir: None,
            remote_ssh_control_persist: None,
            remote_disable_ssh_mux: false,
        };
        CliContext::from_globals(&globals)
    }

    #[test]
    fn floorplan_no_arg_writes_settings_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::remove_var("TAPA_STEP_FLOORPLAN_PYTHON");
        let mut ctx = ctx_with_work_dir(dir.path());
        let args = FloorplanArgs { floorplan_path: None };
        run_floorplan(&args, &mut ctx).expect("native floorplan no-op");
        let settings = settings_io::load_settings(dir.path()).expect("load settings");
        assert_eq!(settings.get("floorplan"), Some(&Value::Bool(true)));
        let flow = ctx.flow.borrow();
        assert_eq!(flow.pipelined.get("floorplan"), Some(&true));
    }

    #[test]
    fn floorplan_with_path_errors_without_bridge() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::env::remove_var("TAPA_STEP_FLOORPLAN_PYTHON");
        let mut ctx = ctx_with_work_dir(dir.path());
        let args = FloorplanArgs {
            floorplan_path: Some(dir.path().join("fp.json")),
        };
        let err = run_floorplan(&args, &mut ctx).expect_err("must reject without bridge");
        assert!(matches!(err, CliError::InvalidArg(_)));
    }

    #[test]
    fn enumerate_solutions_sorted_and_filtered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ab = dir.path().join(AUTOBRIDGE_WORK_DIR);
        for name in ["solution_2", "solution_1", "solution_10", "noise"] {
            let p = ab.join(name);
            fs::create_dir_all(&p).expect("mkdir");
            if name.starts_with("solution_") {
                fs::write(p.join("floorplan.json"), b"{}").expect("write");
            }
        }
        let solutions = enumerate_solution_floorplans(dir.path()).expect("enumerate");
        let names: Vec<String> = solutions
            .iter()
            .map(|p| {
                p.parent()
                    .and_then(Path::file_name)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(names, vec!["solution_1", "solution_10", "solution_2"]);
    }

    #[test]
    fn enumerate_solutions_returns_empty_when_dir_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let solutions = enumerate_solution_floorplans(dir.path()).expect("enumerate");
        assert!(solutions.is_empty());
    }
}
