//! `tapa run-autobridge` orchestration (local + remote).
//!
//! Ports `tapa.steps.floorplan.run_autobridge`: sanitize the floorplan
//! config (drop pre-assignments), spawn `rapidstream-tapafp`, and log
//! every `solution_*/floorplan.json` the tool emits. Dispatch goes
//! through `tapa_xilinx::ToolRunner` so the same code path covers:
//!
//!   * `LocalToolRunner` — spawn locally (Python-parity
//!     `subprocess.run`).
//!   * `RemoteToolRunner` — tar-pipe the floorplan project dir + the
//!     device config up, run `rapidstream-tapafp` over SSH-mux, and
//!     tar-pipe `autobridge/` back (Python-parity
//!     `tapa.remote.popen.create_tool_process` with
//!     `extra_download_paths=(autobridge_work_dir,)`).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tapa_xilinx::{
    LocalToolRunner, RemoteToolRunner, SshMuxOptions, SshSession, ToolInvocation,
    ToolRunner, XilinxError,
};

use crate::context::CliContext;
use crate::error::{CliError, Result};

use super::{RunAutobridgeArgs, AUTOBRIDGE_WORK_DIR};

const FLOORPLAN_CONFIG_NO_PRE_ASSIGNMENTS: &str =
    "floorplan_config_no_pre_assignments.json";
const RAPIDSTREAM_TAPAFP_BIN: &str = "rapidstream-tapafp";

/// `tapa run-autobridge` dispatcher.
///
/// Sanitizes the floorplan config, builds a `rapidstream-tapafp`
/// invocation, then hands it to a local or remote `ToolRunner` based
/// on `ctx.remote_config`. On success, `solution_*/floorplan.json`
/// files land under `<work_dir>/autobridge/` (directly for local; via
/// tar-pipe download for remote).
pub fn run_run_autobridge(
    args: &RunAutobridgeArgs,
    ctx: &mut CliContext,
) -> Result<()> {
    let prepared = prepare_inputs(args, ctx.work_dir.as_path())?;
    if let Some(cfg) = ctx.remote_config.as_ref() {
        let session = std::sync::Arc::new(SshSession::new(
            cfg.clone(),
            SshMuxOptions::default(),
        ));
        let runner = RemoteToolRunner::new(session);
        run_with(&runner, &prepared)
    } else {
        let runner = LocalToolRunner::new();
        run_with(&runner, &prepared)
    }
}

/// Inputs + derived paths for a single `rapidstream-tapafp` invocation.
struct Prepared {
    work_dir: PathBuf,
    ab_graph_path: PathBuf,
    autobridge_dir: PathBuf,
    device_config: PathBuf,
    sanitized_config: PathBuf,
}

/// Mirror the Python preprocessing: ensure the autobridge work dir
/// exists, strip pre-assignment keys from the floorplan config, and
/// persist the sanitized variant.
fn prepare_inputs(args: &RunAutobridgeArgs, work_dir: &Path) -> Result<Prepared> {
    let ab_graph_path = work_dir.join("ab_graph.json");
    let autobridge_dir = work_dir.join(AUTOBRIDGE_WORK_DIR);
    fs::create_dir_all(&autobridge_dir)?;

    let raw = fs::read_to_string(&args.floorplan_config)?;
    let mut config: Value = serde_json::from_str(&raw)?;
    if let Some(obj) = config.as_object_mut() {
        obj.remove("sys_port_pre_assignments");
        obj.remove("cpp_arg_pre_assignments");
        obj.insert("port_pre_assignments".to_string(), json!({}));
    }
    let sanitized_config = autobridge_dir.join(FLOORPLAN_CONFIG_NO_PRE_ASSIGNMENTS);
    fs::write(&sanitized_config, serde_json::to_vec(&config)?)?;

    Ok(Prepared {
        work_dir: work_dir.to_path_buf(),
        ab_graph_path,
        autobridge_dir,
        device_config: absolutize(&args.device_config)?,
        sanitized_config,
    })
}

/// Resolve a potentially-relative path to an absolute one — remote
/// path rewriting only triggers on absolute paths.
fn absolutize(p: &Path) -> Result<PathBuf> {
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(p))
    }
}

fn build_invocation(prep: &Prepared) -> ToolInvocation {
    // For remote dispatch, uploads must be absolute paths that exist:
    //   * `work_dir` is uploaded via `cwd` (contains `ab_graph.json`
    //     and the sanitized `autobridge/` dir).
    //   * `device_config` sits outside `work_dir` in typical usage, so
    //     it has to be added as an extra upload so the remote tool can
    //     read it at the rewritten path.
    // Downloads list the full `autobridge/` dir so the remote tool's
    // `solution_*/floorplan.json` outputs tar-pipe back into place.
    let mut uploads: Vec<PathBuf> = Vec::new();
    if prep.device_config.is_absolute() && prep.device_config.exists() {
        uploads.push(prep.device_config.clone());
    }
    ToolInvocation {
        program: RAPIDSTREAM_TAPAFP_BIN.to_string(),
        args: vec![
            "--ab-graph-path".to_string(),
            prep.ab_graph_path.display().to_string(),
            "--work-dir".to_string(),
            prep.autobridge_dir.display().to_string(),
            "--device-config".to_string(),
            prep.device_config.display().to_string(),
            "--floorplan-config".to_string(),
            prep.sanitized_config.display().to_string(),
            "--run-floorplan".to_string(),
        ],
        cwd: Some(prep.work_dir.clone()),
        uploads,
        downloads: vec![prep.autobridge_dir.clone()],
        ..ToolInvocation::default()
    }
}

fn run_with(runner: &dyn ToolRunner, prep: &Prepared) -> Result<()> {
    let inv = build_invocation(prep);
    let out = runner.run(&inv).map_err(map_runner_err)?;
    if out.exit_code != 0 {
        let stderr = if out.stderr.is_empty() {
            format!("{RAPIDSTREAM_TAPAFP_BIN} exited non-zero with empty stderr")
        } else {
            out.stderr
        };
        return Err(CliError::TapaccFailed {
            code: out.exit_code,
            stderr,
        });
    }
    log_solution_floorplans(&prep.autobridge_dir);
    Ok(())
}

/// Map low-level `XilinxError` variants from the tool runner to typed
/// CLI errors that explicitly name the failure mode (spawn / transfer
/// / signal). Previously the remote branch hard-errored with "use the
/// Python CLI"; now failures surface the concrete runtime gap.
fn map_runner_err(err: XilinxError) -> CliError {
    match err {
        // Spawn failure from `LocalToolRunner` when `rapidstream-tapafp`
        // is missing from `PATH` (or not executable).
        XilinxError::ToolFailure {
            program,
            code,
            stderr,
        } if program == RAPIDSTREAM_TAPAFP_BIN && stderr.starts_with("spawn failed") => {
            CliError::TapaccNotExecutable {
                path: PathBuf::from(RAPIDSTREAM_TAPAFP_BIN),
                reason: format!("exit code {code}: {stderr}"),
            }
        }
        XilinxError::ToolFailure { code, stderr, .. } => {
            CliError::TapaccFailed { code, stderr }
        }
        XilinxError::ToolSignaled { program } => CliError::TapaccFailed {
            code: -1,
            stderr: format!("{program} killed by signal"),
        },
        XilinxError::ToolTimeout {
            program,
            timeout_secs,
        } => CliError::TapaccFailed {
            code: -1,
            stderr: format!("{program} timed out after {timeout_secs}s"),
        },
        // SSH / transfer / setup failures: keep the full context in the
        // typed error rather than collapsing onto `TapaccFailed`.
        // Enumerated explicitly so adding a new `XilinxError` variant
        // forces a decision here (workspace denies wildcard matches).
        inner @ (XilinxError::SshConnect { .. }
        | XilinxError::SshMuxLost { .. }
        | XilinxError::RemoteTransfer(_)
        | XilinxError::ToolNotFound(_)
        | XilinxError::MissingXilinxHls
        | XilinxError::Config { .. }
        | XilinxError::DeviceConfig { .. }
        | XilinxError::PlatformNotFound(_)
        | XilinxError::HlsReportParse(_)
        | XilinxError::HlsRetryExhausted { .. }
        | XilinxError::KernelXml(_)
        | XilinxError::XoRedaction(_)
        | XilinxError::Io(_)
        | XilinxError::Zip(_)
        | XilinxError::Xml(_)
        | XilinxError::Json(_)) => CliError::Xilinx(inner),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tapa_xilinx::{MockToolRunner, ToolOutput};

    fn write_cfg(dir: &Path, name: &str, body: &Value) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, serde_json::to_vec(body).unwrap()).unwrap();
        p
    }

    fn make_prepared(work_dir: &Path) -> Prepared {
        let fp_cfg_body = json!({
            "sys_port_pre_assignments": {"x": "A"},
            "cpp_arg_pre_assignments": {"y": "B"},
            "port_pre_assignments": {"z": "C"},
            "other": 1,
        });
        let fp_cfg = write_cfg(work_dir, "floorplan_config.json", &fp_cfg_body);
        let dev_cfg = write_cfg(work_dir, "device.json", &json!({"part": "xcvu37p"}));
        fs::write(work_dir.join("ab_graph.json"), b"{}").unwrap();
        let args = RunAutobridgeArgs {
            device_config: dev_cfg,
            floorplan_config: fp_cfg,
        };
        prepare_inputs(&args, work_dir).expect("prepare inputs")
    }

    #[test]
    fn prepare_strips_pre_assignments_and_creates_work_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let prep = make_prepared(tmp.path());
        assert!(prep.autobridge_dir.is_dir(), "autobridge dir must be created");
        assert!(
            prep.sanitized_config.is_file(),
            "sanitized config must be written",
        );
        let sanitized: Value =
            serde_json::from_slice(&fs::read(&prep.sanitized_config).unwrap()).unwrap();
        let obj = sanitized.as_object().unwrap();
        assert!(obj.get("sys_port_pre_assignments").is_none());
        assert!(obj.get("cpp_arg_pre_assignments").is_none());
        assert_eq!(obj.get("port_pre_assignments"), Some(&json!({})));
        assert_eq!(obj.get("other"), Some(&json!(1)));
    }

    #[test]
    fn invocation_shape_matches_python_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let prep = make_prepared(tmp.path());
        let inv = build_invocation(&prep);

        assert_eq!(inv.program, RAPIDSTREAM_TAPAFP_BIN);
        // The argv must match the Python `cmd` list verbatim (in order).
        assert_eq!(
            inv.args,
            vec![
                "--ab-graph-path".to_string(),
                prep.ab_graph_path.display().to_string(),
                "--work-dir".to_string(),
                prep.autobridge_dir.display().to_string(),
                "--device-config".to_string(),
                prep.device_config.display().to_string(),
                "--floorplan-config".to_string(),
                prep.sanitized_config.display().to_string(),
                "--run-floorplan".to_string(),
            ],
        );
        assert_eq!(inv.cwd.as_deref(), Some(prep.work_dir.as_path()));

        // Uploads include the device config (outside the cwd); cwd
        // itself is uploaded by `RemoteToolRunner::run_once` so no
        // explicit entry is required here.
        assert!(
            inv.uploads.iter().any(|p| p == &prep.device_config),
            "uploads must include the device config: {:?}",
            inv.uploads,
        );

        // Downloads list the autobridge dir so `solution_*/floorplan.json`
        // tar-pipes back.
        assert_eq!(inv.downloads.as_slice(), std::slice::from_ref(&prep.autobridge_dir));
    }

    #[test]
    fn mock_runner_success_drives_through_invocation() {
        let tmp = tempfile::tempdir().unwrap();
        let prep = make_prepared(tmp.path());
        let runner = MockToolRunner::new();
        runner.push_ok(
            RAPIDSTREAM_TAPAFP_BIN,
            ToolOutput {
                exit_code: 0,
                stdout: "ok".into(),
                stderr: String::new(),
            },
        );
        run_with(&runner, &prep).expect("mock success must propagate as Ok");

        let calls = runner.calls();
        assert_eq!(calls.len(), 1, "exactly one rapidstream-tapafp call");
        let inv = &calls[0];
        assert_eq!(inv.program, RAPIDSTREAM_TAPAFP_BIN);
        assert!(
            inv.args.contains(&"--run-floorplan".to_string()),
            "`--run-floorplan` flag must be present: {:?}",
            inv.args,
        );
        assert_eq!(inv.cwd.as_deref(), Some(prep.work_dir.as_path()));
        assert_eq!(inv.downloads.as_slice(), std::slice::from_ref(&prep.autobridge_dir));
    }

    /// A remote-style failure (non-zero exit) must surface as a typed
    /// `CliError::TapaccFailed` carrying the remote stderr — **not** a
    /// "use the Python CLI" string.
    #[test]
    fn mock_runner_nonzero_exit_surfaces_tapacc_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let prep = make_prepared(tmp.path());
        let runner = MockToolRunner::new();
        runner.push_ok(
            RAPIDSTREAM_TAPAFP_BIN,
            ToolOutput {
                exit_code: 4,
                stdout: String::new(),
                stderr: "no floorplan solution found".into(),
            },
        );
        let err = run_with(&runner, &prep).expect_err("non-zero exit must error");
        let CliError::TapaccFailed { code, stderr } = err else {
            panic!("expected TapaccFailed, got {err:?}")
        };
        assert_eq!(code, 4);
        assert!(
            stderr.contains("no floorplan solution found"),
            "stderr must echo the remote message: {stderr}",
        );
    }

    /// A transport-layer failure (the SSH mux dies mid-run) must
    /// surface as a typed `CliError::Xilinx(SshMuxLost)` rather than
    /// being collapsed onto `TapaccFailed` or a "use the Python CLI"
    /// string.
    #[test]
    fn mock_runner_transport_failure_surfaces_typed_ssh_error() {
        let tmp = tempfile::tempdir().unwrap();
        let prep = make_prepared(tmp.path());
        let runner = MockToolRunner::new();
        runner.push_err(
            RAPIDSTREAM_TAPAFP_BIN,
            XilinxError::SshMuxLost {
                detail: "mux_client_read_packet: broken pipe".into(),
            },
        );
        let err = run_with(&runner, &prep).expect_err("mux loss must error");
        let CliError::Xilinx(XilinxError::SshMuxLost { detail }) = err else {
            panic!("expected Xilinx(SshMuxLost), got {err:?}")
        };
        assert!(
            detail.contains("broken pipe"),
            "detail must echo mux failure: {detail}",
        );
    }

    /// A missing binary on the remote surfaces as `ToolFailure` with a
    /// non-"spawn failed" stderr; must route to `TapaccFailed` (not
    /// `TapaccNotExecutable`, which is the `LocalToolRunner`
    /// spawn-failure shape).
    #[test]
    fn mock_runner_remote_missing_binary_surfaces_tapacc_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let prep = make_prepared(tmp.path());
        let runner = MockToolRunner::new();
        runner.push_err(
            RAPIDSTREAM_TAPAFP_BIN,
            XilinxError::ToolFailure {
                program: RAPIDSTREAM_TAPAFP_BIN.to_string(),
                code: 127,
                stderr: "bash: rapidstream-tapafp: command not found".into(),
            },
        );
        let err = run_with(&runner, &prep).expect_err("missing binary must error");
        let CliError::TapaccFailed { code, stderr } = err else {
            panic!("expected TapaccFailed, got {err:?}")
        };
        assert_eq!(code, 127);
        assert!(
            stderr.contains("command not found"),
            "stderr must describe the gap: {stderr}",
        );
    }
}
