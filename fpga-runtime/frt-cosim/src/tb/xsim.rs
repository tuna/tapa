use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode, StreamDir};

#[derive(Clone)]
struct MmapArg {
    name: String,
    data_width_bytes: usize,
    base_addr: u64,
    reg_offset_lo: u32,
    reg_offset_hi: u32,
}

#[derive(Clone)]
struct ScalarArg {
    name: String,
    value_expr: String,
    words: Vec<ScalarWord>,
}

#[derive(Clone)]
struct ScalarWord {
    reg_offset: u32,
    value_u32: u32,
}

#[derive(Clone)]
struct StreamArg {
    name: String,
    width_bytes: usize,
    has_peek: bool,
    peek_name: String,
}

#[derive(Template)]
#[template(path = "tb_xsim.sv.j2", escape = "none")]
struct SvTemplate<'a> {
    top_name: &'a str,
    mode: &'a str,
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
    tcl_files: Vec<String>,
    xci_files: Vec<String>,
    tb_sv_file: String,
    tb_top: String,
    dpi_lib_path: String,
    save_waveform: bool,
    legacy: bool,
}

pub struct XsimTbGenerator<'a> {
    spec: &'a KernelSpec,
    dpi_lib: &'a Path,
    base_addresses: &'a HashMap<String, u64>,
    scalar_values: &'a HashMap<u32, Vec<u8>>,
    part_num: &'a str,
    save_waveform: bool,
    legacy: bool,
}

impl<'a> XsimTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        dpi_lib: &'a Path,
        base_addresses: &'a HashMap<String, u64>,
        scalar_values: &'a HashMap<u32, Vec<u8>>,
        part_num: &'a str,
        save_waveform: bool,
        legacy: bool,
    ) -> Self {
        Self {
            spec,
            dpi_lib,
            base_addresses,
            scalar_values,
            part_num,
            save_waveform,
            legacy,
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
                    let offset = self
                        .spec
                        .scalar_register_map
                        .get(&arg.name)
                        .copied()
                        .unwrap_or(0);
                    mmap_args.push(MmapArg {
                        name: arg.name.clone(),
                        data_width_bytes: (*data_width as usize).div_ceil(8),
                        base_addr: self.base_addresses.get(&arg.name).copied().unwrap_or(0),
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
                        value_expr: sv_literal(*width, &bytes),
                        words: scalar_words(offset, &bytes),
                    });
                }
                ArgKind::Stream { width, dir, .. } => {
                    let peek = if self.spec.mode == Mode::Hls && *dir == StreamDir::In {
                        infer_peek_name(&arg.name)
                            .filter(|cand| stream_peek_ports_exist(&self.spec.verilog_files, cand))
                    } else {
                        None
                    };
                    let s = StreamArg {
                        name: arg.name.clone(),
                        width_bytes: (*width as usize).div_ceil(8),
                        has_peek: peek.is_some(),
                        peek_name: peek.unwrap_or_default(),
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
        let mode = match self.spec.mode {
            Mode::Hls => "hls",
            Mode::Vitis => "vitis",
        };
        let template = SvTemplate {
            top_name: &self.spec.top_name,
            mode,
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
            tcl_files: self
                .spec
                .tcl_files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            xci_files: self
                .spec
                .xci_files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            tb_sv_file: tb_dir
                .join(format!("tb_{}.sv", self.spec.top_name))
                .to_string_lossy()
                .to_string(),
            tb_top: format!("tb_{}", self.spec.top_name),
            dpi_lib_path: self.dpi_lib.to_string_lossy().to_string(),
            save_waveform: self.save_waveform,
            legacy: self.legacy,
        };
        template
            .render()
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

fn sv_literal(width_bits: u32, bytes_le: &[u8]) -> String {
    let width = width_bits.max(1);
    let hex = bytes_le
        .iter()
        .rev()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{width}'h{hex}")
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
