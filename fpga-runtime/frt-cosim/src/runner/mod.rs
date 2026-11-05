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

pub fn configure_sim_command(cmd: &mut Command) {
    #[cfg(unix)]
    cmd.process_group(0);
}
