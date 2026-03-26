use super::{configure_sim_command, environ::xilinx_environ, SimRunner};
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
    pub start_gui: bool,
    pub part_num_override: Option<String>,
}

impl XsimRunner {
    pub fn find(
        dpi_lib: PathBuf,
        legacy: bool,
        save_waveform: bool,
        start_gui: bool,
        part_num_override: Option<String>,
    ) -> Result<Self> {
        let bin = which("vivado").map_err(|_| CosimError::ToolNotFound("vivado".into()))?;
        Ok(Self {
            vivado_bin: bin,
            dpi_lib,
            legacy,
            save_waveform,
            start_gui,
            part_num_override,
        })
    }
}

impl SimRunner for XsimRunner {
    fn build(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, Vec<u8>>,
        tb_dir: &Path,
    ) -> Result<()> {
        let part = self
            .part_num_override
            .as_deref()
            .or(spec.part_num.as_deref())
            .unwrap_or("xc7a100tcsg324-1");
        apply_default_nettype_wire(spec)?;
        let generator = XsimTbGenerator::new(
            spec,
            &self.dpi_lib,
            &ctx.base_addresses,
            scalar_values,
            part,
            self.save_waveform,
            self.legacy,
        );
        let tb_file = format!("tb_{}.sv", spec.top_name);

        std::fs::write(tb_dir.join(&tb_file), generator.render_tb()?)?;
        std::fs::write(tb_dir.join("run_cosim.tcl"), generator.render_tcl(tb_dir)?)?;
        Ok(())
    }

    fn spawn(&self, ctx: &CosimContext, tb_dir: &Path) -> Result<std::process::Child> {
        let mode = if self.start_gui { "gui" } else { "batch" };
        let home = tb_dir.join("run");
        std::fs::create_dir_all(&home)?;
        let mut cmd = Command::new(&self.vivado_bin);
        cmd.args(["-mode", mode, "-source", "run_cosim.tcl"])
            .current_dir(tb_dir)
            .env("HOME", home.as_os_str())
            .env("TAPA_DPI_CONFIG", ctx.dpi_config_json())
            .envs(xilinx_environ());
        configure_sim_command(&mut cmd);
        let child = cmd.spawn()?;
        Ok(child)
    }
}

fn apply_default_nettype_wire(spec: &KernelSpec) -> Result<()> {
    for file in &spec.verilog_files {
        let Some(ext) = file.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !matches!(ext, "v" | "sv") {
            continue;
        }
        let content = std::fs::read_to_string(file)?;
        if content.starts_with("`default_nettype") {
            continue;
        }
        std::fs::write(file, format!("`default_nettype wire\n{content}"))?;
    }
    Ok(())
}
