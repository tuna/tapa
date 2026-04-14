use super::{configure_sim_command, environ::xilinx_environ, SimRunner};
use crate::context::CosimContext;
use crate::error::{CosimError, Result};
use crate::metadata::KernelSpec;
use crate::tb::xsim::XsimTbGenerator;
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use which::which;

pub struct XsimRunner {
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
        Ok(Self {
            dpi_lib,
            legacy,
            save_waveform,
            start_gui,
            part_num_override,
        })
    }
}

impl SimRunner for XsimRunner {
    fn prepare(
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

    fn spawn(
        &self,
        _spec: &KernelSpec,
        ctx: &CosimContext,
        tb_dir: &Path,
    ) -> Result<std::process::Child> {
        let mode = if self.start_gui { "gui" } else { "batch" };
        let home = tb_dir.join("run");
        std::fs::create_dir_all(&home)?;
        let vivado_bin = which("vivado").map_err(|_e| CosimError::ToolNotFound("vivado".into()))?;
        let mut cmd = Command::new(vivado_bin);
        // Base environment (incl. Xilinx tool paths) FIRST, then per-instance
        // overrides LAST — .envs() would overwrite .env() values for keys
        // already in the parent env (HOME, TMPDIR).
        cmd.args(["-mode", mode, "-source", "run_cosim.tcl"])
            .current_dir(tb_dir)
            .envs(xilinx_environ())
            .env("HOME", home.as_os_str())
            .env("TMPDIR", home.as_os_str())
            .env(frt_shm::env::TAPA_DPI_CONFIG, ctx.dpi_config_json());
        configure_sim_command(&mut cmd);
        let child = cmd.spawn()?;
        Ok(child)
    }
}

fn apply_default_nettype_wire(spec: &KernelSpec) -> Result<()> {
    const PREFIX: &[u8] = b"`default_nettype";
    for file in &spec.verilog_files {
        let Some(ext) = file.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !matches!(ext, "v" | "sv") {
            continue;
        }
        // Read only enough bytes to check for the prefix, avoiding a full
        // read_to_string when the directive is already present.
        {
            let mut f = std::fs::File::open(file)?;
            let mut buf = [0u8; PREFIX.len()];
            if f.read(&mut buf)? == PREFIX.len() && buf == *PREFIX {
                continue;
            }
        }
        let content = std::fs::read_to_string(file)?;
        std::fs::write(file, format!("`default_nettype wire\n{content}"))?;
    }
    Ok(())
}
