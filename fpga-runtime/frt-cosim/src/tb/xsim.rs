use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, StreamDir};

#[derive(Clone)]
struct MmapArg {
    name: String,
    data_width_bytes: usize,
    base_addr: u64,
}

#[derive(Clone)]
struct ScalarArg {
    name: String,
    value: String,
}

#[derive(Clone)]
struct StreamArg {
    name: String,
    width_bytes: usize,
}

#[derive(Template)]
#[template(path = "tb_xsim.sv.j2", escape = "none")]
struct SvTemplate<'a> {
    top_name: &'a str,
    mmap_args: Vec<MmapArg>,
    scalar_args: Vec<ScalarArg>,
    stream_args: Vec<StreamArg>,
    stream_out_args: Vec<StreamArg>,
}

#[derive(Template)]
#[template(path = "run_cosim.tcl.j2", escape = "none")]
struct TclTemplate {
    tb_dir: String,
    part_num: String,
    verilog_files: Vec<String>,
    dpi_lib_path: String,
    save_waveform: bool,
}

pub struct XsimTbGenerator<'a> {
    spec: &'a KernelSpec,
    dpi_lib: &'a Path,
    base_addresses: &'a HashMap<String, u64>,
    part_num: &'a str,
    save_waveform: bool,
}

impl<'a> XsimTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        dpi_lib: &'a Path,
        base_addresses: &'a HashMap<String, u64>,
        part_num: &'a str,
        save_waveform: bool,
    ) -> Self {
        Self {
            spec,
            dpi_lib,
            base_addresses,
            part_num,
            save_waveform,
        }
    }

    fn collect_args(&self) -> (Vec<MmapArg>, Vec<ScalarArg>, Vec<StreamArg>, Vec<StreamArg>) {
        let mut mmap_args = vec![];
        let mut scalar_args = vec![];
        let mut stream_args = vec![];
        let mut stream_out_args = vec![];

        for arg in &self.spec.args {
            match &arg.kind {
                ArgKind::Mmap { data_width, .. } => {
                    mmap_args.push(MmapArg {
                        name: arg.name.clone(),
                        data_width_bytes: (*data_width as usize).div_ceil(8),
                        base_addr: self.base_addresses.get(&arg.name).copied().unwrap_or(0),
                    });
                }
                ArgKind::Scalar { .. } => {
                    scalar_args.push(ScalarArg {
                        name: arg.name.clone(),
                        value: "0".to_string(),
                    });
                }
                ArgKind::Stream { width, dir, .. } => {
                    let s = StreamArg {
                        name: arg.name.clone(),
                        width_bytes: (*width as usize).div_ceil(8),
                    };
                    if *dir == StreamDir::In {
                        stream_args.push(s);
                    } else {
                        stream_out_args.push(s);
                    }
                }
            }
        }
        (mmap_args, scalar_args, stream_args, stream_out_args)
    }

    pub fn render_tb(&self) -> Result<String> {
        let (mmap_args, scalar_args, stream_args, stream_out_args) = self.collect_args();
        let template = SvTemplate {
            top_name: &self.spec.top_name,
            mmap_args,
            scalar_args,
            stream_args,
            stream_out_args,
        };
        template
            .render()
            .map_err(|e| CosimError::Metadata(format!("template render failed: {e}")))
    }

    pub fn render_tcl(&self, tb_dir: &Path) -> Result<String> {
        let template = TclTemplate {
            tb_dir: tb_dir.to_string_lossy().to_string(),
            part_num: self.part_num.to_string(),
            verilog_files: self
                .spec
                .verilog_files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            dpi_lib_path: self.dpi_lib.to_string_lossy().to_string(),
            save_waveform: self.save_waveform,
        };
        template
            .render()
            .map_err(|e| CosimError::Metadata(format!("template render failed: {e}")))
    }
}
