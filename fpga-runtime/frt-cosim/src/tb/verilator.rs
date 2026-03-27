use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode, StreamDir};
use crate::tb::names::{cpp_identifier, cpp_signal};

pub struct MmapArg {
    pub name: String,
    pub ident: String,
    pub araddr: String,
    pub arburst: String,
    pub arcache: String,
    pub arid: String,
    pub arlen: String,
    pub arlock: String,
    pub arprot: String,
    pub arqos: String,
    pub arready: String,
    pub arsize: String,
    pub arvalid: String,
    pub awaddr: String,
    pub awburst: String,
    pub awcache: String,
    pub awid: String,
    pub awlen: String,
    pub awlock: String,
    pub awprot: String,
    pub awqos: String,
    pub awready: String,
    pub awsize: String,
    pub awvalid: String,
    pub bid: String,
    pub bready: String,
    pub bresp: String,
    pub bvalid: String,
    pub rdata: String,
    pub rid: String,
    pub rlast: String,
    pub rready: String,
    pub rresp: String,
    pub rvalid: String,
    pub wdata: String,
    pub wlast: String,
    pub wready: String,
    pub wstrb: String,
    pub wvalid: String,
    pub data_width_bytes: usize,
    pub base_addr: u64,
    pub data_size: usize,
    pub reg_offset_lo: u32,
    pub reg_offset_hi: u32,
}

pub struct ScalarArg {
    pub name: String,
    pub member: String,
    pub bytes_initializer: String,
    pub words: Vec<ScalarWord>,
}

pub struct ScalarWord {
    pub reg_offset: u32,
    pub value_u32: u32,
}

pub struct StreamArg {
    pub name: String,
    pub ident: String,
    pub empty_n: String,
    pub dout: String,
    pub din: String,
    pub read: String,
    pub full_n: String,
    pub write: String,
    pub tdata: String,
    pub tvalid: String,
    pub tready: String,
    pub tlast: String,
    pub width_bytes: usize,
    pub has_peek: bool,
    pub peek_name: String,
    pub peek_empty_n: String,
    pub peek_dout: String,
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
    buffer_sizes: &'a HashMap<String, usize>,
    scalar_values: &'a HashMap<u32, Vec<u8>>,
}

impl<'a> VerilatorTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        dpi_lib: &'a Path,
        base_addresses: &'a HashMap<String, u64>,
        buffer_sizes: &'a HashMap<String, usize>,
        scalar_values: &'a HashMap<u32, Vec<u8>>,
    ) -> Self {
        Self {
            spec,
            _dpi_lib: dpi_lib,
            base_addresses,
            buffer_sizes,
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
                    let data_size = self.buffer_sizes.get(&arg.name).copied().unwrap_or(4 * 1024 * 1024);
                    mmap_args.push(MmapArg::new(
                        &arg.name,
                        (*data_width as usize).div_ceil(8),
                        base,
                        data_size,
                        offset,
                    ));
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
                    scalar_args.push(ScalarArg::new(&arg.name, &bytes, offset));
                }
                ArgKind::Stream { width, dir, .. } => {
                    let w = (*width as usize).div_ceil(8);
                    let peek = if self.spec.mode == Mode::Hls && *dir == StreamDir::In {
                        infer_peek_name(&arg.name)
                            .filter(|cand| stream_peek_ports_exist(&self.spec.verilog_files, cand))
                    } else {
                        None
                    };
                    let stream = StreamArg::new(&arg.name, w, peek);
                    match dir {
                        StreamDir::In => stream_args.push(stream),
                        StreamDir::Out => stream_out_args.push(stream),
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

impl MmapArg {
    fn new(name: &str, data_width_bytes: usize, base_addr: u64, data_size: usize, reg_offset_lo: u32) -> Self {
        let ident = cpp_identifier(name);
        Self {
            name: name.to_owned(),
            ident: ident.clone(),
            araddr: cpp_signal("m_axi_", name, "_ARADDR"),
            arburst: cpp_signal("m_axi_", name, "_ARBURST"),
            arcache: cpp_signal("m_axi_", name, "_ARCACHE"),
            arid: cpp_signal("m_axi_", name, "_ARID"),
            arlen: cpp_signal("m_axi_", name, "_ARLEN"),
            arlock: cpp_signal("m_axi_", name, "_ARLOCK"),
            arprot: cpp_signal("m_axi_", name, "_ARPROT"),
            arqos: cpp_signal("m_axi_", name, "_ARQOS"),
            arready: cpp_signal("m_axi_", name, "_ARREADY"),
            arsize: cpp_signal("m_axi_", name, "_ARSIZE"),
            arvalid: cpp_signal("m_axi_", name, "_ARVALID"),
            awaddr: cpp_signal("m_axi_", name, "_AWADDR"),
            awburst: cpp_signal("m_axi_", name, "_AWBURST"),
            awcache: cpp_signal("m_axi_", name, "_AWCACHE"),
            awid: cpp_signal("m_axi_", name, "_AWID"),
            awlen: cpp_signal("m_axi_", name, "_AWLEN"),
            awlock: cpp_signal("m_axi_", name, "_AWLOCK"),
            awprot: cpp_signal("m_axi_", name, "_AWPROT"),
            awqos: cpp_signal("m_axi_", name, "_AWQOS"),
            awready: cpp_signal("m_axi_", name, "_AWREADY"),
            awsize: cpp_signal("m_axi_", name, "_AWSIZE"),
            awvalid: cpp_signal("m_axi_", name, "_AWVALID"),
            bid: cpp_signal("m_axi_", name, "_BID"),
            bready: cpp_signal("m_axi_", name, "_BREADY"),
            bresp: cpp_signal("m_axi_", name, "_BRESP"),
            bvalid: cpp_signal("m_axi_", name, "_BVALID"),
            rdata: cpp_signal("m_axi_", name, "_RDATA"),
            rid: cpp_signal("m_axi_", name, "_RID"),
            rlast: cpp_signal("m_axi_", name, "_RLAST"),
            rready: cpp_signal("m_axi_", name, "_RREADY"),
            rresp: cpp_signal("m_axi_", name, "_RRESP"),
            rvalid: cpp_signal("m_axi_", name, "_RVALID"),
            wdata: cpp_signal("m_axi_", name, "_WDATA"),
            wlast: cpp_signal("m_axi_", name, "_WLAST"),
            wready: cpp_signal("m_axi_", name, "_WREADY"),
            wstrb: cpp_signal("m_axi_", name, "_WSTRB"),
            wvalid: cpp_signal("m_axi_", name, "_WVALID"),
            data_width_bytes,
            base_addr,
            data_size,
            reg_offset_lo,
            reg_offset_hi: reg_offset_lo + 4,
        }
    }
}

impl ScalarArg {
    fn new(name: &str, bytes: &[u8], reg_offset: u32) -> Self {
        Self {
            name: name.to_owned(),
            member: cpp_identifier(name),
            bytes_initializer: bytes_to_cpp_initializer(bytes),
            words: scalar_words(reg_offset, bytes),
        }
    }
}

impl StreamArg {
    fn new(name: &str, width_bytes: usize, peek: Option<String>) -> Self {
        let has_peek = peek.is_some();
        let peek_name = peek.unwrap_or_default();
        let ident = cpp_identifier(name);
        Self {
            name: name.to_owned(),
            ident: ident.clone(),
            empty_n: cpp_signal("", name, "_empty_n"),
            dout: cpp_signal("", name, "_dout"),
            din: cpp_signal("", name, "_din"),
            read: cpp_signal("", name, "_read"),
            full_n: cpp_signal("", name, "_full_n"),
            write: cpp_signal("", name, "_write"),
            tdata: cpp_signal("", name, "_TDATA"),
            tvalid: cpp_signal("", name, "_TVALID"),
            tready: cpp_signal("", name, "_TREADY"),
            tlast: cpp_signal("", name, "_TLAST"),
            width_bytes,
            has_peek,
            peek_name: peek_name.clone(),
            peek_empty_n: cpp_signal("", &peek_name, "_empty_n"),
            peek_dout: cpp_signal("", &peek_name, "_dout"),
        }
    }
}
