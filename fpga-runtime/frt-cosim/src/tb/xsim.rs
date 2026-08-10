use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode};
use crate::tb::names::{escape_verilog_identifier, verilator_identifier};
use crate::tb::{classify_args, read_verilog_contents, MmapArg, Naming, ScalarArg, StreamArg};

/// xsim drives the DUT through escaped Verilog port references, while the
/// testbench state it declares around them must stay bare identifiers.
const NAMING: Naming = Naming {
    ident: verilator_identifier,
    port: escape_verilog_identifier,
};

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
    start_gui: bool,
}

/// How the simulation is run, as opposed to what is simulated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XsimOptions {
    /// Log all signals to a waveform database.
    pub save_waveform: bool,
    /// Target a Vivado old enough to predate the `xsim.*` fileset properties.
    pub legacy: bool,
    /// Hand the simulation to the Vivado GUI instead of running it headless.
    pub start_gui: bool,
}

pub struct XsimTbGenerator<'a> {
    spec: &'a KernelSpec,
    dpi_lib: &'a Path,
    base_addresses: &'a HashMap<String, u64>,
    scalar_values: &'a HashMap<u32, Vec<u8>>,
    part_num: &'a str,
    options: XsimOptions,
}

impl<'a> XsimTbGenerator<'a> {
    pub fn new(
        spec: &'a KernelSpec,
        dpi_lib: &'a Path,
        base_addresses: &'a HashMap<String, u64>,
        scalar_values: &'a HashMap<u32, Vec<u8>>,
        part_num: &'a str,
        options: XsimOptions,
    ) -> Self {
        Self {
            spec,
            dpi_lib,
            base_addresses,
            scalar_values,
            part_num,
            options,
        }
    }

    fn collect_args(&self) -> Result<super::ClassifiedArgs> {
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
                let offset_port = super::direct_offset_port_name(&verilog_contents, &arg.name);
                MmapArg {
                    id_width: detect_axi_id_width(&verilog_contents, &arg.name),
                    ..MmapArg::new(
                        NAMING,
                        &arg.name,
                        &offset_port,
                        (data_width as usize).div_ceil(8),
                        base_addresses.get(&arg.name).copied().unwrap_or(0),
                        offset,
                    )
                }
            },
            |arg, width, offset, bytes| {
                ScalarArg::new(NAMING, &arg.name, sv_literal(width, bytes), offset, bytes)
            },
            |arg, width_bytes, peek, axis| {
                StreamArg::new(NAMING, &arg.name, width_bytes, peek, axis)
            },
        )
    }

    pub fn render_tb(&self) -> Result<String> {
        let (mmap_args, scalar_args, stream_args, stream_out_args) = self.collect_args()?;
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
            save_waveform: self.options.save_waveform,
            legacy: self.options.legacy,
            start_gui: self.options.start_gui,
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
