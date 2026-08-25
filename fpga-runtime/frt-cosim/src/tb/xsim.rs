use askama::Template;
use std::collections::HashMap;
use std::path::Path;

use crate::error::{CosimError, Result};
use crate::metadata::{ArgKind, KernelSpec, Mode};
use crate::tb::names::{escape_verilog_identifier, verilator_identifier};
use crate::tb::{
    classify_args, control_addr_width, read_verilog_contents, MmapArg, Naming, ScalarArg, StreamArg,
};

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
    control_addr_width: u32,
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

    fn collect_args(&self, verilog_contents: &[String]) -> Result<super::ClassifiedArgs> {
        let base_addresses = self.base_addresses;

        classify_args(
            self.spec,
            self.scalar_values,
            verilog_contents,
            |arg, offset| {
                let data_width = match &arg.kind {
                    ArgKind::Mmap { data_width, .. } => *data_width,
                    ArgKind::Scalar { .. } | ArgKind::Stream { .. } => unreachable!(),
                };
                let offset_port = super::direct_offset_port_name(verilog_contents, &arg.name);
                MmapArg {
                    id_width: detect_axi_port_width(
                        verilog_contents,
                        &format!("m_axi_{}_ARID", arg.name),
                    ),
                    lock_width: detect_axi_port_width(
                        verilog_contents,
                        &format!("m_axi_{}_ARLOCK", arg.name),
                    ),
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
        // Read the packaged RTL once: a large design ships thousands of
        // Verilog files and every probe below scans all of them.
        let verilog_contents = read_verilog_contents(self.spec);
        let (mmap_args, scalar_args, stream_args, stream_out_args) =
            self.collect_args(&verilog_contents)?;
        let mode = match self.spec.mode {
            Mode::Hls => "hls",
            Mode::Vitis => "vitis",
        };
        let required_control_width = control_addr_width(&mmap_args, &scalar_args);
        let control_addr_width = match detect_parameter_value(
            &verilog_contents,
            "C_S_AXI_CONTROL_ADDR_WIDTH",
        ) {
            Some(declared) if declared < required_control_width => {
                return Err(CosimError::Metadata(format!(
                    "packaged control address port is {declared} bits, but register offsets require {required_control_width} bits"
                )));
            }
            Some(declared) => declared,
            None => required_control_width.max(8),
        };
        let template = SvTemplate {
            top_name: &self.spec.top_name,
            mode,
            control_addr_width,
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

fn detect_parameter_value(verilog_contents: &[String], name: &str) -> Option<u32> {
    let pattern = format!(r"{}\s*=\s*(\d+)", regex_lite::escape(name));
    let re = regex_lite::Regex::new(&pattern).unwrap();
    for text in verilog_contents {
        if let Some(caps) = re.captures(text) {
            if let Ok(value) = caps[1].parse::<u32>() {
                return Some(value);
            }
        }
    }
    None
}

/// Detect a top-level AXI port width, or one for scalar/absent ports.
/// Scans for the escaped spelling, which is how the packaged RTL declares
/// an identifier that needs escaping.
fn detect_axi_port_width(verilog_contents: &[String], port_name: &str) -> usize {
    crate::tb::port_declaration(verilog_contents, &escape_verilog_identifier(port_name))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{ArgKind, ArgSpec};

    #[test]
    fn detects_axi_port_widths() {
        let verilog = vec![concat!(
            "module Top #(parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6) ",
            "(output [2:0] m_axi_mem_ARID, output m_axi_mem_ARLOCK); endmodule"
        )
        .to_owned()];
        assert_eq!(detect_axi_port_width(&verilog, "m_axi_mem_ARID"), 3);
        assert_eq!(detect_axi_port_width(&verilog, "m_axi_mem_ARLOCK"), 1);
        assert_eq!(
            detect_parameter_value(&verilog, "C_S_AXI_CONTROL_ADDR_WIDTH"),
            Some(6)
        );
    }

    /// The testbench programs a 64-bit mmap base address as two 32-bit
    /// control writes, so the template has to split it with a mask and a
    /// shift. Askama spells bitwise AND `bitand`; getting that expression
    /// wrong still renders, and the damage only shows up as a DUT reading
    /// the wrong address under xsim.
    #[test]
    fn splits_mmap_base_address_into_low_and_high_words() {
        let spec = KernelSpec {
            top_name: "Top".to_owned(),
            mode: Mode::Vitis,
            args: vec![ArgSpec {
                name: "buf".to_owned(),
                id: 0,
                kind: ArgKind::Mmap {
                    data_width: 32,
                    addr_width: 64,
                },
            }],
            part_num: None,
            verilog_files: vec![],
            tcl_files: vec![],
            xci_files: vec![],
            scalar_register_map: HashMap::from([("buf".to_owned(), 0x10)]),
        };
        let base_addresses = HashMap::from([("buf".to_owned(), 0xdead_beef_1234_5678_u64)]);
        let tb = XsimTbGenerator::new(
            &spec,
            Path::new("/tmp/libfrt_dpi_xsim.so"),
            &base_addresses,
            &HashMap::new(),
            "xcu55c-fsvh2892-2L-e",
            XsimOptions::default(),
        )
        .render_tb()
        .expect("render testbench");

        assert!(
            tb.contains("32'h12345678"),
            "low word of the base address missing: {tb}"
        );
        assert!(
            tb.contains("32'hdeadbeef"),
            "high word of the base address missing: {tb}"
        );
    }

    #[test]
    fn widens_control_address_bus_for_large_register_map() {
        let spec = KernelSpec {
            top_name: "Top".to_owned(),
            mode: Mode::Vitis,
            args: vec![ArgSpec {
                name: "value".to_owned(),
                id: 0,
                kind: ArgKind::Scalar { width: 64 },
            }],
            part_num: None,
            verilog_files: vec![],
            tcl_files: vec![],
            xci_files: vec![],
            scalar_register_map: HashMap::from([("value".to_owned(), 0x100)]),
        };
        let base_addresses = HashMap::new();
        let scalar_values = HashMap::from([(0, vec![0; 8])]);
        let tb = XsimTbGenerator::new(
            &spec,
            Path::new("/tmp/libfrt_dpi_xsim.so"),
            &base_addresses,
            &scalar_values,
            "xcu55c-fsvh2892-2L-e",
            XsimOptions::default(),
        )
        .render_tb()
        .expect("render testbench");

        assert!(tb.contains("reg [8:0] s_axi_control_AWADDR"));
        assert!(tb.contains("ctrl_write(9'h100"));
        assert!(tb.contains("ctrl_write(9'h104"));
    }
}
