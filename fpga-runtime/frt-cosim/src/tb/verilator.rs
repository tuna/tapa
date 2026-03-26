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
    pub bytes_initializer: String,
    pub words: Vec<ScalarWord>,
}

pub struct ScalarWord {
    pub reg_offset: u32,
    pub value_u32: u32,
}

pub struct StreamArg {
    pub name: String,
    pub width_bytes: usize,
    pub has_peek: bool,
    pub peek_name: String,
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
    scalar_values: &'a HashMap<u32, Vec<u8>>,
}

impl<'a> VerilatorTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        dpi_lib: &'a Path,
        base_addresses: &'a HashMap<String, u64>,
        scalar_values: &'a HashMap<u32, Vec<u8>>,
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
                ArgKind::Scalar { width } => {
                    let offset = self
                        .spec
                        .scalar_register_map
                        .get(&arg.name)
                        .copied()
                        .unwrap_or(0);
                    let bytes = normalized_scalar_bytes(
                        *width,
                        self.scalar_values.get(&arg.id).map(|x| x.as_slice()),
                    );
                    scalar_args.push(ScalarArg {
                        name: arg.name.clone(),
                        bytes_initializer: bytes_to_cpp_initializer(&bytes),
                        words: scalar_words(offset, &bytes),
                    });
                }
                ArgKind::Stream { width, dir, .. } => {
                    let w = (*width as usize).div_ceil(8);
                    let peek = if self.spec.mode == Mode::Hls && *dir == StreamDir::In {
                        infer_peek_name(&arg.name)
                            .filter(|cand| stream_peek_ports_exist(&self.spec.verilog_files, cand))
                    } else {
                        None
                    };
                    match dir {
                        StreamDir::In => stream_args.push(StreamArg {
                            name: arg.name.clone(),
                            width_bytes: w,
                            has_peek: peek.is_some(),
                            peek_name: peek.unwrap_or_default(),
                        }),
                        StreamDir::Out => stream_out_args.push(StreamArg {
                            name: arg.name.clone(),
                            width_bytes: w,
                            has_peek: false,
                            peek_name: String::new(),
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

fn normalized_scalar_bytes(width_bits: u32, raw: Option<&[u8]>) -> Vec<u8> {
    let expected = (width_bits as usize).div_ceil(8).max(1);
    let mut out = raw.map(|x| x.to_vec()).unwrap_or_default();
    if out.len() < expected {
        out.resize(expected, 0);
    } else if out.len() > expected {
        out.truncate(expected);
    }
    out
}

fn bytes_to_cpp_initializer(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("0x{b:02x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn scalar_words(base_offset: u32, bytes: &[u8]) -> Vec<ScalarWord> {
    let mut words = Vec::new();
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let mut raw = [0u8; 4];
        raw[..chunk.len()].copy_from_slice(chunk);
        words.push(ScalarWord {
            reg_offset: base_offset + (i as u32) * 4,
            value_u32: u32::from_le_bytes(raw),
        });
    }
    if words.is_empty() {
        words.push(ScalarWord {
            reg_offset: base_offset,
            value_u32: 0,
        });
    }
    words
}

fn infer_peek_name(stream_name: &str) -> Option<String> {
    if let Some(base) = stream_name.strip_suffix("_s") {
        return Some(format!("{base}_peek"));
    }
    let mut iter = stream_name.rsplitn(2, '_');
    let suffix = iter.next()?;
    let base = iter.next()?;
    if suffix.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("{base}_peek_{suffix}"));
    }
    None
}

fn stream_peek_ports_exist(verilog_files: &[std::path::PathBuf], peek_name: &str) -> bool {
    let dout = format!("{peek_name}_dout");
    let empty_n = format!("{peek_name}_empty_n");
    verilog_files.iter().any(|file| {
        std::fs::read_to_string(file)
            .map(|text| text.contains(&dout) && text.contains(&empty_n))
            .unwrap_or(false)
    })
}
