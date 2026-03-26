pub mod environ;
pub mod verilator;
pub mod xsim;

use crate::{context::CosimContext, error::Result, metadata::KernelSpec};
use std::path::Path;

pub struct SimResult {
    pub wall_ns: u64,
}

pub trait SimRunner {
    fn build(&self, spec: &KernelSpec, tb_dir: &Path) -> Result<()>;
    fn run(&self, ctx: &CosimContext, tb_dir: &Path) -> Result<SimResult>;
}
