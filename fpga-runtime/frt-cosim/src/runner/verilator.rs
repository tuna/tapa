use super::{environ::xilinx_environ, SimResult, SimRunner};
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
    fn build(&self, spec: &KernelSpec, tb_dir: &Path) -> Result<()> {
        let base_addrs: HashMap<String, u64> = spec
            .args
            .iter()
            .filter_map(|arg| match &arg.kind {
                crate::metadata::ArgKind::Mmap { .. } => Some((arg.name.clone(), 0x1000_0000)),
                _ => None,
            })
            .collect();
        let generator = VerilatorTbGenerator::new(spec, &self.dpi_lib, &base_addrs);
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
        let status = Command::new(&self.verilator_bin)
            .args([
                "--cc",
                "--top-module",
                top,
                "--no-timing",
                "--exe",
                "tb.cpp",
                "-Wno-WIDTH",
                "-Wno-UNDRIVEN",
                "-Wno-STMTDLY",
                "-y",
                rtl_dir.to_string_lossy().as_ref(),
            ])
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
                &format!("V{top}"),
            ])
            .current_dir(tb_dir)
            .status()?;
        if !status.success() {
            return Err(CosimError::SimFailed(status));
        }
        Ok(())
    }

    fn run(&self, ctx: &CosimContext, tb_dir: &Path) -> Result<SimResult> {
        let exe = tb_dir.join("obj_dir").join(format!("V{}", "top"));
        let top = if exe.exists() {
            exe
        } else {
            let mut found = None;
            for entry in std::fs::read_dir(tb_dir.join("obj_dir"))? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_str()
                    .map(|n| n.starts_with('V'))
                    .unwrap_or(false)
                {
                    found = Some(entry.path());
                    break;
                }
            }
            found.ok_or_else(|| CosimError::ToolNotFound("Verilator binary".into()))?
        };
        let t0 = std::time::Instant::now();
        let status = Command::new(top)
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

fn num_cpus_str() -> String {
    std::thread::available_parallelism()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "4".into())
}
