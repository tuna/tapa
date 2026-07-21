//! Per-invocation execution context shared by the chained step pipeline.

use std::path::PathBuf;

use tapa_xilinx::RemoteConfig;

use crate::globals::GlobalArgs;

#[derive(Debug)]
pub struct CliContext {
    pub work_dir: PathBuf,
    pub temp_dir: Option<PathBuf>,
    pub clang_format_quota_in_bytes: u64,
    /// Resolved remote config (`~/.taparc` + CLI overrides). `None`
    /// means the run is purely local.
    pub remote_config: Option<RemoteConfig>,
    /// Verbosity counts for downstream command invocations.
    pub verbose: u8,
    pub quiet: u8,
}

impl CliContext {
    pub fn from_globals(globals: &GlobalArgs) -> Self {
        Self {
            work_dir: absolutize_for_storage(&globals.work_dir),
            temp_dir: globals.temp_dir.clone(),
            clang_format_quota_in_bytes: globals.clang_format_quota_in_bytes,
            remote_config: None,
            verbose: globals.verbose,
            quiet: globals.quiet,
        }
    }

    /// Dispatch `f` with the remote tool runner when a remote config
    /// is active (`~/.taparc` / `--remote-host`), otherwise the local
    /// runner.
    pub fn with_tool_runner<R>(&self, f: impl FnOnce(&dyn tapa_xilinx::ToolRunner) -> R) -> R {
        if let Some(cfg) = self.remote_config.as_ref() {
            let session = std::sync::Arc::new(tapa_xilinx::SshSession::new(
                cfg.clone(),
                tapa_xilinx::SshMuxOptions::default(),
            ));
            let runner = tapa_xilinx::RemoteToolRunner::new(session);
            f(&runner)
        } else {
            let runner = tapa_xilinx::LocalToolRunner::new();
            f(&runner)
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
/// `work.out/cpp/Foo.cpp` in its temporary cwd and failing.
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
