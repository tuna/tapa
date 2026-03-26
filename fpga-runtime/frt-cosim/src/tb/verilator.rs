use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode, StreamDir};

pub struct MmapArg {
    pub name: String,
    pub data_width_bytes: usize,
    pub base_addr: u64,
    pub reg_offset_lo: u32,
    pub reg_offset_hi: u32,
}

pub struct ScalarArg {
    pub name: String,
    pub value_u64: String,
    pub reg_offset: u32,
}

pub struct StreamArg {
    pub name: String,
    pub width_bytes: usize,
}

#[derive(Template)]
#[template(path = "tb_verilator.cpp.j2", escape = "none")]
struct TbTemplate<'a> {
    top_name: &'a str,
    mode: &'a str,
    mmap_args: Vec<MmapArg>,
    scalar_args: Vec<ScalarArg>,
    stream_args: Vec<StreamArg>,
    stream_out_args: Vec<StreamArg>,
}

pub struct VerilatorTbGenerator<'a> {
    spec: &'a KernelSpec,
    _dpi_lib: &'a Path,
    base_addresses: &'a HashMap<String, u64>,
    scalar_values: &'a HashMap<u32, u64>,
}

impl<'a> VerilatorTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        dpi_lib: &'a Path,
        base_addresses: &'a HashMap<String, u64>,
        scalar_values: &'a HashMap<u32, u64>,
    ) -> Self {
        Self {
            spec,
            _dpi_lib: dpi_lib,
            base_addresses,
            scalar_values,
        }
    }

    pub fn render_tb(&self) -> Result<String> {
        let mode_str = match self.spec.mode {
            Mode::Hls => "hls",
            Mode::Vitis => "vitis",
        };
        let mut mmap_args = vec![];
        let mut scalar_args = vec![];
        let mut stream_args = vec![];
        let mut stream_out_args = vec![];

        for arg in &self.spec.args {
            match &arg.kind {
                ArgKind::Mmap { data_width, .. } => {
                    let base = self.base_addresses.get(&arg.name).copied().unwrap_or(0);
                    let offset = self
                        .spec
                        .scalar_register_map
                        .get(&arg.name)
                        .copied()
                        .unwrap_or(0);
                    mmap_args.push(MmapArg {
                        name: arg.name.clone(),
                        data_width_bytes: (*data_width as usize).div_ceil(8),
                        base_addr: base,
                        reg_offset_lo: offset,
                        reg_offset_hi: offset + 4,
                    });
                }
                ArgKind::Scalar { .. } => {
                    let offset = self
                        .spec
                        .scalar_register_map
                        .get(&arg.name)
                        .copied()
                        .unwrap_or(0);
                    scalar_args.push(ScalarArg {
                        name: arg.name.clone(),
                        value_u64: format!(
                            "0x{:016x}ULL",
                            self.scalar_values.get(&arg.id).copied().unwrap_or(0)
                        ),
                        reg_offset: offset,
                    });
                }
                ArgKind::Stream {
                    width, dir, ..
                } => {
                    let w = (*width as usize).div_ceil(8);
                    match dir {
                        StreamDir::In => stream_args.push(StreamArg {
                            name: arg.name.clone(),
                            width_bytes: w,
                        }),
                        StreamDir::Out => stream_out_args.push(StreamArg {
                            name: arg.name.clone(),
                            width_bytes: w,
                        }),
                    }
                }
            }
        }

        let tmpl = TbTemplate {
            top_name: &self.spec.top_name,
            mode: mode_str,
            mmap_args,
            scalar_args,
            stream_args,
            stream_out_args,
        };
        tmpl.render()
            .map_err(|e| CosimError::Metadata(format!("template render failed: {e}")))
    }
}
