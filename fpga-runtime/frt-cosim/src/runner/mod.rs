pub mod environ;
pub mod verilator;
pub mod xsim;

use crate::{context::CosimContext, error::Result, metadata::KernelSpec};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Child;
use std::process::Command;

pub trait SimRunner {
    fn prepare(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, Vec<u8>>,
        tb_dir: &Path,
    ) -> Result<()>;
    fn spawn(&self, spec: &KernelSpec, ctx: &CosimContext, tb_dir: &Path) -> Result<Child>;
}

/// Acquire an exclusive `flock`-based lock on the given path.
///
/// Creates the file (and parent directories) if they don't exist.
/// Returns the open `File` whose lifetime holds the lock.
#[cfg(unix)]
pub fn acquire_exclusive_lock(lock_path: &std::path::Path) -> Result<std::fs::File> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    file.lock()?;
    Ok(file)
}

pub fn configure_sim_command(cmd: &mut Command) {
    #[cfg(unix)]
    cmd.process_group(0);
}
