pub mod environ;
pub mod verilator;
pub mod xsim;

use crate::{context::CosimContext, error::Result, metadata::KernelSpec};
use std::collections::HashMap;
use std::path::Path;

pub struct SimResult {
    pub wall_ns: u64,
}

pub trait SimRunner {
    fn build(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, u64>,
        tb_dir: &Path,
    ) -> Result<()>;
    fn run(&self, ctx: &CosimContext, tb_dir: &Path) -> Result<SimResult>;
}
