//! Per-invocation execution context shared by the chained step pipeline.
//! Mirrors click's `ctx.obj` dict from `tapa/__main__.py`.

use std::cell::RefCell;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::Value;
use tapa_task_graph::Design;
use tapa_xilinx::RemoteConfig;

use crate::globals::GlobalArgs;
use crate::options::Options;

/// In-memory flow state — the Rust analogue of click's `ctx.obj` dict.
#[derive(Debug, Default)]
pub struct FlowState {
    pub design: Option<Design>,
    pub graph: Option<Value>,
    pub settings: Option<IndexMap<String, Value>>,
    /// Per-step `is_pipelined` markers (mirrors `is_pipelined()` in Python).
    pub pipelined: IndexMap<String, bool>,
}

#[derive(Debug)]
pub struct CliContext {
    pub work_dir: PathBuf,
    pub temp_dir: Option<PathBuf>,
    pub options: Options,
    pub remote: RemoteConfigArgs,
    /// Resolved remote config (`~/.taparc` + CLI overrides). `None`
    /// means the run is purely local — mirrors Python's
    /// `get_remote_config() is None`.
    pub remote_config: Option<RemoteConfig>,
    pub flow: RefCell<FlowState>,
    /// Verbosity counts forwarded to bridged Python invocations.
    pub verbose: u8,
    pub quiet: u8,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteConfigArgs {
    pub host: Option<String>,
    pub key_file: Option<String>,
    pub xilinx_settings: Option<String>,
    pub ssh_control_dir: Option<String>,
    pub ssh_control_persist: Option<String>,
    pub disable_ssh_mux: bool,
}

impl CliContext {
    pub fn from_globals(globals: &GlobalArgs) -> Self {
        let options = Options {
            clang_format_quota_in_bytes: globals.clang_format_quota_in_bytes,
        };
        let remote = RemoteConfigArgs {
            host: globals.remote_host.clone(),
            key_file: globals.remote_key_file.clone(),
            xilinx_settings: globals.remote_xilinx_settings.clone(),
            ssh_control_dir: globals.remote_ssh_control_dir.clone(),
            ssh_control_persist: globals.remote_ssh_control_persist.clone(),
            disable_ssh_mux: globals.remote_disable_ssh_mux,
        };
        Self {
            work_dir: absolutize_for_storage(&globals.work_dir),
            temp_dir: globals.temp_dir.clone(),
            options,
            remote,
            remote_config: None,
            flow: RefCell::new(FlowState::default()),
            verbose: globals.verbose,
            quiet: globals.quiet,
        }
    }

    pub fn switch_work_dir(&mut self, path: PathBuf) -> std::io::Result<()> {
        std::fs::create_dir_all(&path)?;
        let abs = absolutize_for_storage(&path);
        drop(path);
        self.work_dir = abs;
        Ok(())
    }
}

/// Normalize a user-supplied `--work-dir` into an absolute path at
/// storage time. The remote transport rewrites absolute local paths
/// into the remote rootfs; a relative `./work.out` would sneak
/// through unrewritten, leaving the remote command looking for
/// `work.out/cpp/Foo.cpp` in its temporary cwd and failing. Python
/// `tapa` stored `os.path.abspath(work_dir)`; we match that.
///
/// Non-existent paths still get a plain `current_dir().join(...)` —
/// `canonicalize` would fail before the first step creates the
/// directory.
fn absolutize_for_storage(path: &std::path::Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let joined = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path);
    std::fs::canonicalize(&joined).unwrap_or(joined)
}
