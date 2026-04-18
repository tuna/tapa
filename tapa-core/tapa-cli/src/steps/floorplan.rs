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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use indexmap::IndexMap;
use serde_json::{json, Value};
use tapa_task_graph::{
    apply_floorplan, convert_region_format, region_to_slot_name, Graph, TransformError,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};
use crate::state::{graph as graph_io, settings as settings_io};
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
    if let Some(path) = args.floorplan_path.as_ref() {
        return run_floorplan_native_apply(path, ctx);
    }
    run_floorplan_native_noop(ctx)
}

/// Apply a floorplan JSON file to the cached graph.
///
/// Mirrors `tapa.steps.floorplan.floorplan` and
/// `tapa.steps.floorplan.get_slot_to_inst`:
///   1. Read `floorplan_path` as `vertex → "x:y"` JSON.
///   2. Filter to vertices that match a known top-level instance.
///   3. Group by `region` with `:` → `_` to derive the slot name.
///   4. Run [`apply_floorplan`] to wrap leaf instances under slot tasks.
///   5. Persist the rewritten graph to `<work_dir>/graph.json` and stash
///      `slot_task_name_to_fp_region` (regions in `_TO_` form) plus the
///      `floorplan` flag in `settings.json`.
fn run_floorplan_native_apply(path: &Path, ctx: &CliContext) -> Result<()> {
    let work_dir = ctx.work_dir.as_path();
    let graph_value = load_or_cached_graph(ctx)?;

    let graph_json = serde_json::to_string(&graph_value)?;
    let typed = Graph::from_json(&graph_json)?;

    let raw = fs::read_to_string(path)?;
    let vertex_to_region: IndexMap<String, String> = serde_json::from_str(&raw)?;

    let top_def = typed.tasks.get(&typed.top).ok_or_else(|| {
        CliError::InvalidArg(format!(
            "graph is missing the top task `{}`",
            typed.top,
        ))
    })?;
    let mut known_inst_names = BTreeSet::<String>::new();
    for (def_name, insts) in &top_def.tasks {
        for idx in 0..insts.len() {
            known_inst_names.insert(format!("{def_name}_{idx}"));
        }
    }

    let mut slot_to_insts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut slot_to_region: IndexMap<String, String> = IndexMap::new();
    for (vertex, region) in &vertex_to_region {
        if !known_inst_names.contains(vertex) {
            continue;
        }
        let slot = region_to_slot_name(region);
        slot_to_insts
            .entry(slot.clone())
            .or_default()
            .push(vertex.clone());
        slot_to_region.insert(slot, convert_region_format(region));
    }

    if slot_to_insts.is_empty() {
        return Err(CliError::InvalidArg(format!(
            "floorplan file `{}` did not match any top-level instances; \
             verify the floorplan is for the flattened graph",
            path.display(),
        )));
    }

    let (new_graph, _slot_echo) =
        apply_floorplan(&typed, &slot_to_insts).map_err(map_transform_err)?;

    let new_json = new_graph
        .to_json()
        .map_err(|e| CliError::InvalidArg(format!("re-serialize graph: {e}")))?;
    let new_value: Value = serde_json::from_str(&new_json)?;
    graph_io::store_graph(work_dir, &new_value)?;

    // Rebuild design.json so chained downstream steps see slot tasks +
    // the floorplan region map. Codex Round 3 finding: standalone
    // `tapa floorplan --floorplan-path` previously left design.json
    // stale (only graph.json + settings.json were rewritten).
    let target = match settings_io::load_settings(work_dir) {
        Ok(s) => s
            .get("target")
            .and_then(Value::as_str)
            .map_or_else(|| typed.tasks.get(&typed.top).map_or("xilinx-vitis", |_| "xilinx-vitis").to_string(), ToString::to_string),
        Err(_) => "xilinx-vitis".to_string(),
    };
    let region_map_for_design: indexmap::IndexMap<String, String> =
        slot_to_region.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let design = build_design_with_floorplan(
        &new_value,
        &typed.top,
        &target,
        Some(&region_map_for_design),
    )?;
    crate::state::design::store_design(work_dir, &design)?;

    let mut flow = ctx.flow.borrow_mut();
    let mut settings = if let Some(s) = flow.settings.take() {
        s
    } else {
        match settings_io::load_settings(work_dir) {
            Ok(s) => s,
            Err(CliError::MissingState { .. }) => settings_io::Settings::new(),
            Err(other) => return Err(other),
        }
    };
    settings.insert("floorplan".to_string(), Value::Bool(true));
    let region_obj: serde_json::Map<String, Value> = slot_to_region
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    settings.insert(
        "slot_task_name_to_fp_region".to_string(),
        Value::Object(region_obj),
    );
    settings_io::store_settings(work_dir, &settings)?;
    flow.settings = Some(settings);
    flow.graph = Some(new_value);
    flow.design = Some(design);
    flow.pipelined.insert("floorplan".to_string(), true);
    Ok(())
}

/// Build a [`tapa_task_graph::Design`] from a (possibly floorplan-rewritten)
/// graph dict, threading the slot→region echo map through. Mirrors the
/// projection in `analyze::build_design` but accepts an explicit
/// `slot_task_name_to_fp_region` so the post-floorplan write captures
/// the new slot-task identity.
fn build_design_with_floorplan(
    graph: &Value,
    top: &str,
    target: &str,
    slot_to_region: Option<&indexmap::IndexMap<String, String>>,
) -> Result<tapa_task_graph::Design> {
    use tapa_task_graph::TaskTopology;
    let tasks_obj = graph
        .get("tasks")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CliError::InvalidArg("rewritten graph missing `tasks` object".to_string())
        })?;
    let slot_set: std::collections::BTreeSet<&str> = slot_to_region
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let mut topology: indexmap::IndexMap<String, TaskTopology> =
        indexmap::IndexMap::new();
    for (name, task) in tasks_obj {
        topology.insert(
            name.clone(),
            TaskTopology {
                name: name.clone(),
                level: task
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("lower")
                    .to_string(),
                code: task
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                ports: task
                    .get("ports")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| serde_json::from_value(p.clone()).ok())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                tasks: value_to_indexmap(task.get("tasks")),
                fifos: value_to_indexmap(task.get("fifos")),
                target: task
                    .get("target")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                is_slot: slot_set.contains(name.as_str()),
                self_area: indexmap::IndexMap::new(),
                total_area: indexmap::IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
    }
    Ok(tapa_task_graph::Design {
        top: top.to_string(),
        target: target.to_string(),
        tasks: topology,
        slot_task_name_to_fp_region: slot_to_region.cloned(),
    })
}

fn value_to_indexmap(value: Option<&Value>) -> indexmap::IndexMap<String, Value> {
    let Some(Value::Object(obj)) = value else {
        return indexmap::IndexMap::new();
    };
    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn load_or_cached_graph(ctx: &CliContext) -> Result<Value> {
    {
        let flow = ctx.flow.borrow();
        if let Some(g) = flow.graph.as_ref() {
            return Ok(g.clone());
        }
    }
    graph_io::load_graph(ctx.work_dir.as_path())
}

fn map_transform_err(e: TransformError) -> CliError {
    match e {
        TransformError::DeepHierarchyNotSupported(child) => CliError::InvalidArg(format!(
            "floorplan: child `{child}` is upper-level; flatten first",
        )),
        other @ (TransformError::MissingTop(_)
        | TransformError::TopIsLeaf(_)
        | TransformError::UnknownFloorplanInstance(_)
        | TransformError::SlotNameCollision(_)
        | TransformError::Json(_)) => CliError::InvalidArg(other.to_string()),
    }
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
        // With native --floorplan-path enabled, the failure is now a
        // typed graph-load error (no graph.json on disk, no cached
        // graph in flow state) rather than an `InvalidArg` opt-in stub.
        assert!(
            matches!(err, CliError::MissingState { .. } | CliError::Io(_)),
            "expected typed graph-load failure, got {err:?}",
        );
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
