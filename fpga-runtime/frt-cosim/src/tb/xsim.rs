use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode};
use crate::tb::names::{escape_verilog_identifier, escaped_verilog_signal, verilator_identifier};
use crate::tb::{classify_args, read_verilog_contents, scalar_words, ScalarWord};

#[derive(Clone)]
struct MmapArg {
    name: String,
    ident: String,
    offset_name: String,
    araddr: String,
    arburst: String,
    arcache: String,
    arid: String,
    arlen: String,
    arlock: String,
    arprot: String,
    arqos: String,
    arready: String,
    arsize: String,
    arvalid: String,
    awaddr: String,
    awburst: String,
    awcache: String,
    awid: String,
    awlen: String,
    awlock: String,
    awprot: String,
    awqos: String,
    awready: String,
    awsize: String,
    awvalid: String,
    bid: String,
    bready: String,
    bresp: String,
    bvalid: String,
    rdata: String,
    rid: String,
    rlast: String,
    rready: String,
    rresp: String,
    rvalid: String,
    wdata: String,
    wlast: String,
    wready: String,
    wstrb: String,
    wvalid: String,
    data_width_bytes: usize,
    id_width: usize,
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
struct StreamArg {
    name: String,
    ident: String,
    empty_n: String,
    dout: String,
    din: String,
    read: String,
    full_n: String,
    write: String,
    tdata: String,
    tvalid: String,
    tready: String,
    tlast: String,
    width_bytes: usize,
    /// Total bytes passed to/from the DPI function.  Always `width_bytes + 1`:
    /// the extra byte carries the EOS/TLAST flag.  For AXIS streams this maps
    /// to TLAST; for `ApFifo` streams it maps to the MSB of the `dout`/`din` port.
    dpi_width_bytes: usize,
    /// True when the stream uses AXI-Stream (Vitis mode).  The EOS bit is
    /// carried as a separate byte in the DPI transfer and maps to TLAST.
    axis: bool,
    has_peek: bool,
    peek_empty_n: String,
    peek_dout: String,
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
    dpi_sv_root: String,
    dpi_sv_lib: String,
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
        let verilog_contents = read_verilog_contents(self.spec);
        let base_addresses = self.base_addresses;

        classify_args(
            self.spec,
            self.scalar_values,
            &verilog_contents,
            |arg, offset| {
                let data_width = match &arg.kind {
                    ArgKind::Mmap { data_width, .. } => *data_width,
                    ArgKind::Scalar { .. } | ArgKind::Stream { .. } => unreachable!(),
                };
                let id_width = detect_axi_id_width(&verilog_contents, &arg.name);
                MmapArg::new(
                    &arg.name,
                    (data_width as usize).div_ceil(8),
                    id_width,
                    base_addresses.get(&arg.name).copied().unwrap_or(0),
                    offset,
                )
            },
            |arg, width, offset, bytes| ScalarArg::new(&arg.name, width, bytes, offset),
            |arg, width_bytes, peek, axis| StreamArg::new(&arg.name, width_bytes, peek, axis),
        )
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
            part_num: self.part_num.to_owned(),
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
            dpi_sv_root: self
                .dpi_lib
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .to_string(),
            dpi_sv_lib: self
                .dpi_lib
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            save_waveform: self.save_waveform,
            legacy: self.legacy,
        };
        template
            .render()
            .map_err(|e| CosimError::Metadata(format!("template render failed: {e}")))
    }
}

fn sv_literal(width_bits: u32, bytes_le: &[u8]) -> String {
    use std::fmt::Write;
    let width = width_bits.max(1);
    let hex = bytes_le.iter().rev().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    });
    format!("{width}'h{hex}")
}

/// Detect the AXI ID width for a given mmap port by scanning the Verilog
/// source files for the ARID port declaration.  Returns the bit-width
/// (e.g. 3 for `[2:0]`).  Defaults to 1 if the port is not found.
fn detect_axi_id_width(verilog_contents: &[String], mmap_name: &str) -> usize {
    let escaped = escape_verilog_identifier(&format!("m_axi_{mmap_name}_ARID"));
    // Match patterns like:  output [2:0] m_axi_foo_ARID  or  wire [2:0] ...
    // The left bound N in [N:0] gives width = N+1.
    let pattern = format!(
        r"\[\s*(\d+)\s*:\s*0\s*\]\s*{}",
        regex_lite::escape(&escaped)
    );
    let re = regex_lite::Regex::new(&pattern).unwrap();
    for text in verilog_contents {
        if let Some(caps) = re.captures(text) {
            if let Ok(n) = caps[1].parse::<usize>() {
                return n + 1;
            }
        }
    }
    1
}

impl MmapArg {
    fn new(
        name: &str,
        data_width_bytes: usize,
        id_width: usize,
        base_addr: u64,
        reg_offset_lo: u32,
    ) -> Self {
        Self {
            name: name.to_owned(),
            ident: verilator_identifier(name),
            offset_name: escape_verilog_identifier(&format!("{name}_offset")),
            araddr: escaped_verilog_signal("m_axi_", name, "_ARADDR"),
            arburst: escaped_verilog_signal("m_axi_", name, "_ARBURST"),
            arcache: escaped_verilog_signal("m_axi_", name, "_ARCACHE"),
            arid: escaped_verilog_signal("m_axi_", name, "_ARID"),
            arlen: escaped_verilog_signal("m_axi_", name, "_ARLEN"),
            arlock: escaped_verilog_signal("m_axi_", name, "_ARLOCK"),
            arprot: escaped_verilog_signal("m_axi_", name, "_ARPROT"),
            arqos: escaped_verilog_signal("m_axi_", name, "_ARQOS"),
            arready: escaped_verilog_signal("m_axi_", name, "_ARREADY"),
            arsize: escaped_verilog_signal("m_axi_", name, "_ARSIZE"),
            arvalid: escaped_verilog_signal("m_axi_", name, "_ARVALID"),
            awaddr: escaped_verilog_signal("m_axi_", name, "_AWADDR"),
            awburst: escaped_verilog_signal("m_axi_", name, "_AWBURST"),
            awcache: escaped_verilog_signal("m_axi_", name, "_AWCACHE"),
            awid: escaped_verilog_signal("m_axi_", name, "_AWID"),
            awlen: escaped_verilog_signal("m_axi_", name, "_AWLEN"),
            awlock: escaped_verilog_signal("m_axi_", name, "_AWLOCK"),
            awprot: escaped_verilog_signal("m_axi_", name, "_AWPROT"),
            awqos: escaped_verilog_signal("m_axi_", name, "_AWQOS"),
            awready: escaped_verilog_signal("m_axi_", name, "_AWREADY"),
            awsize: escaped_verilog_signal("m_axi_", name, "_AWSIZE"),
            awvalid: escaped_verilog_signal("m_axi_", name, "_AWVALID"),
            bid: escaped_verilog_signal("m_axi_", name, "_BID"),
            bready: escaped_verilog_signal("m_axi_", name, "_BREADY"),
            bresp: escaped_verilog_signal("m_axi_", name, "_BRESP"),
            bvalid: escaped_verilog_signal("m_axi_", name, "_BVALID"),
            rdata: escaped_verilog_signal("m_axi_", name, "_RDATA"),
            rid: escaped_verilog_signal("m_axi_", name, "_RID"),
            rlast: escaped_verilog_signal("m_axi_", name, "_RLAST"),
            rready: escaped_verilog_signal("m_axi_", name, "_RREADY"),
            rresp: escaped_verilog_signal("m_axi_", name, "_RRESP"),
            rvalid: escaped_verilog_signal("m_axi_", name, "_RVALID"),
            wdata: escaped_verilog_signal("m_axi_", name, "_WDATA"),
            wlast: escaped_verilog_signal("m_axi_", name, "_WLAST"),
            wready: escaped_verilog_signal("m_axi_", name, "_WREADY"),
            wstrb: escaped_verilog_signal("m_axi_", name, "_WSTRB"),
            wvalid: escaped_verilog_signal("m_axi_", name, "_WVALID"),
            data_width_bytes,
            id_width,
            base_addr,
            reg_offset_lo,
            reg_offset_hi: reg_offset_lo + 4,
        }
    }
}

impl ScalarArg {
    fn new(name: &str, width_bits: u32, bytes: &[u8], reg_offset: u32) -> Self {
        Self {
            name: escape_verilog_identifier(name),
            value_expr: sv_literal(width_bits, bytes),
            words: scalar_words(reg_offset, bytes),
        }
    }
}

impl StreamArg {
    fn new(name: &str, width_bytes: usize, peek: Option<String>, axis: bool) -> Self {
        let has_peek = peek.is_some();
        let peek_name = peek.unwrap_or_default();
        let dpi_width_bytes = width_bytes + 1;
        Self {
            name: name.to_owned(),
            ident: verilator_identifier(name),
            empty_n: escaped_verilog_signal("", name, "_empty_n"),
            dout: escaped_verilog_signal("", name, "_dout"),
            din: escaped_verilog_signal("", name, "_din"),
            read: escaped_verilog_signal("", name, "_read"),
            full_n: escaped_verilog_signal("", name, "_full_n"),
            write: escaped_verilog_signal("", name, "_write"),
            tdata: escaped_verilog_signal("", name, "_TDATA"),
            tvalid: escaped_verilog_signal("", name, "_TVALID"),
            tready: escaped_verilog_signal("", name, "_TREADY"),
            tlast: escaped_verilog_signal("", name, "_TLAST"),
            width_bytes,
            dpi_width_bytes,
            axis,
            has_peek,
            peek_empty_n: escaped_verilog_signal("", &peek_name, "_empty_n"),
            peek_dout: escaped_verilog_signal("", &peek_name, "_dout"),
        }
    }
}
