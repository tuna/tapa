use super::{configure_sim_command, environ::xilinx_environ, SimRunner};
use crate::context::CosimContext;
use crate::error::{CosimError, Result};
use crate::metadata::KernelSpec;
use crate::tb::verilator::VerilatorTbGenerator;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use which::which;

pub struct VerilatorRunner {
    pub verilator_bin: PathBuf,
    pub dpi_lib: PathBuf,
}

impl VerilatorRunner {
    pub fn find(dpi_lib: PathBuf) -> Result<Self> {
        let bin = which("verilator").map_err(|_| CosimError::ToolNotFound("verilator".into()))?;
        Ok(Self {
            verilator_bin: bin,
            dpi_lib,
        })
    }
}

impl SimRunner for VerilatorRunner {
    fn build(
        &self,
        spec: &KernelSpec,
        ctx: &CosimContext,
        scalar_values: &HashMap<u32, Vec<u8>>,
        tb_dir: &Path,
    ) -> Result<()> {
        let generator =
            VerilatorTbGenerator::new(spec, &self.dpi_lib, &ctx.base_addresses, scalar_values);
        std::fs::write(tb_dir.join("tb.cpp"), generator.render_tb()?)?;
        std::fs::write(tb_dir.join("dpi_support.cpp"), "// optional support\n")?;

        let rtl_dir = tb_dir.join("rtl");
        std::fs::create_dir_all(&rtl_dir)?;
        for f in &spec.verilog_files {
            if let Some(fname) = f.file_name() {
                std::fs::copy(f, rtl_dir.join(fname))?;
            }
        }

        let top = &spec.top_name;
        let mut args = vec![
            "--cc".to_string(),
            "--top-module".to_string(),
            top.to_string(),
            "--no-timing".to_string(),
            "--exe".to_string(),
            "tb.cpp".to_string(),
            "-LDFLAGS".to_string(),
            self.dpi_lib.to_string_lossy().to_string(),
            "-Wno-WIDTH".to_string(),
            "-Wno-UNDRIVEN".to_string(),
            "-Wno-STMTDLY".to_string(),
            "-Wno-SYMRSVDWORD".to_string(),
            "-y".to_string(),
            rtl_dir.to_string_lossy().to_string(),
        ];
        for f in std::fs::read_dir(&rtl_dir)? {
            let path = f?.path();
            if path
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| matches!(x, "v" | "sv" | "vh"))
                .unwrap_or(false)
            {
                args.push(path.to_string_lossy().to_string());
            }
        }
        let status = Command::new(&self.verilator_bin)
            .args(&args)
            .current_dir(tb_dir)
            .status()?;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }
        let status = Command::new("make")
            .args([
                "-j",
                &num_cpus_str(),
                "-C",
                "obj_dir",
                "-f",
                &format!("V{top}.mk"),
                &format!("V{top}"),
            ])
            .current_dir(tb_dir)
            .status()?;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }
        Ok(())
    }

    fn spawn(&self, ctx: &CosimContext, tb_dir: &Path) -> Result<std::process::Child> {
        let exe = tb_dir.join("obj_dir").join(format!("V{}", "top"));
        let top = if exe.exists() {
            exe
        } else {
            let mut found = None;
            for entry in std::fs::read_dir(tb_dir.join("obj_dir"))? {
                let entry = entry?;
                let path = entry.path();
                let looks_like_verilator_bin = entry
                    .file_name()
                    .to_str()
                    .map(|n| n.starts_with('V'))
                    .unwrap_or(false);
                let is_executable = path.is_file()
                    && std::fs::metadata(&path)
                        .map(|m| {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                m.permissions().mode() & 0o111 != 0
                            }
                            #[cfg(not(unix))]
                            {
                                true
                            }
                        })
                        .unwrap_or(false);
                if looks_like_verilator_bin && is_executable {
                    found = Some(path);
                    break;
                }
            }
            found.ok_or_else(|| CosimError::ToolNotFound("Verilator binary".into()))?
        };
        let mut cmd = Command::new(top);
        cmd.env("TAPA_DPI_CONFIG", ctx.dpi_config_json())
            .envs(xilinx_environ());
        configure_sim_command(&mut cmd);
        let child = cmd.spawn()?;
        Ok(child)
    }
}

fn num_cpus_str() -> String {
    std::thread::available_parallelism()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "4".into())
}
