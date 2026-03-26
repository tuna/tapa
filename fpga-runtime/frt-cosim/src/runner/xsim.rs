use super::{environ::xilinx_environ, SimResult, SimRunner};
use crate::context::CosimContext;
use crate::error::{CosimError, Result};
use crate::metadata::KernelSpec;
use crate::tb::xsim::XsimTbGenerator;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use which::which;

pub struct XsimRunner {
    pub vivado_bin: PathBuf,
    pub dpi_lib: PathBuf,
    pub legacy: bool,
    pub save_waveform: bool,
}

impl XsimRunner {
    pub fn find(dpi_lib: PathBuf, legacy: bool, save_waveform: bool) -> Result<Self> {
        let bin = which("vivado").map_err(|_| CosimError::ToolNotFound("vivado".into()))?;
        Ok(Self {
            vivado_bin: bin,
            dpi_lib,
            legacy,
            save_waveform,
        })
    }
}

impl SimRunner for XsimRunner {
    fn build(&self, spec: &KernelSpec, tb_dir: &Path) -> Result<()> {
        let part = spec.part_num.as_deref().unwrap_or("xc7a100tcsg324-1");
        let base_addrs: HashMap<String, u64> = spec
            .args
            .iter()
            .filter_map(|arg| match &arg.kind {
                crate::metadata::ArgKind::Mmap { .. } => Some((arg.name.clone(), 0x1000_0000)),
                _ => None,
            })
            .collect();
        let generator = XsimTbGenerator::new(spec, &self.dpi_lib, &base_addrs, part, self.save_waveform);

        std::fs::write(
            tb_dir.join(format!("tb_{}.sv", spec.top_name)),
            generator.render_tb()?,
        )?;
        std::fs::write(tb_dir.join("run_cosim.tcl"), generator.render_tcl(tb_dir)?)?;
        Ok(())
    }

    fn run(&self, ctx: &CosimContext, tb_dir: &Path) -> Result<SimResult> {
        let _ = self.legacy;
        let t0 = std::time::Instant::now();
        let status = Command::new(&self.vivado_bin)
            .args(["-mode", "batch", "-source", "run_cosim.tcl"])
            .current_dir(tb_dir)
            .env("TAPA_DPI_CONFIG", ctx.dpi_config_json())
            .envs(xilinx_environ())
            .status()?;
        let wall_ns = t0.elapsed().as_nanos() as u64;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }
        Ok(SimResult { wall_ns })
    }
}
