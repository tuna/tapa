pub mod environ;
pub mod verilator;
pub mod xsim;

use crate::{
    context::CosimContext,
    error::{CosimError, Result},
    metadata::KernelSpec,
};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Child;
use std::process::Command;

pub struct SimResult {
    pub wall_ns: u64,
}

pub trait SimRunner {
    fn prepare(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, Vec<u8>>,
        tb_dir: &Path,
    ) -> Result<()>;
    fn spawn(&self, spec: &KernelSpec, ctx: &CosimContext, tb_dir: &Path) -> Result<Child>;

    fn run(&self, spec: &KernelSpec, ctx: &CosimContext, tb_dir: &Path) -> Result<SimResult> {
        let t0 = std::time::Instant::now();
        self.prepare(spec, ctx, &HashMap::new(), tb_dir)?;
        let mut child = self.spawn(spec, ctx, tb_dir)?;
        let status = child.wait()?;
        let wall_ns = t0.elapsed().as_nanos() as u64;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }
        Ok(SimResult { wall_ns })
    }
}

/// Acquire an exclusive `flock`-based lock on the given path.
///
/// Creates the file (and parent directories) if they don't exist.
/// Returns the open `File` whose lifetime holds the lock.
#[cfg(unix)]
pub fn acquire_exclusive_lock(lock_path: &std::path::Path) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

pub fn configure_sim_command(cmd: &mut Command) {
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}
