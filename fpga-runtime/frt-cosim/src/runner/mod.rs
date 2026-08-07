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
    // SAFETY: `file` is an open file we just created/opened, so `as_raw_fd()` is a valid fd.
    // `flock(fd, LOCK_EX)` is safe to call on any valid file descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

/// Detach the freshly-forked child into its own process group.
#[cfg(unix)]
fn detach_process_group() -> std::io::Result<()> {
    // SAFETY: `setpgid(0, 0)` is async-signal-safe and only affects the
    // freshly-forked child process.
    if unsafe { libc::setpgid(0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

pub fn configure_sim_command(cmd: &mut Command) {
    #[cfg(unix)]
    // SAFETY: The `pre_exec` callback runs between `fork()` and `exec()` in the
    // child process. It only calls `setpgid(0, 0)` which is async-signal-safe
    // per POSIX and does not access any shared mutable state.
    unsafe {
        cmd.pre_exec(detach_process_group);
    }
}
