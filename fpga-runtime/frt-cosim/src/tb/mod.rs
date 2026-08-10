pub mod names;
pub mod verilator;
pub mod xsim;

use std::collections::HashMap;

use crate::error::{CosimError, Result};
use crate::metadata::{normalized_scalar_bytes, KernelSpec, Mode};

#[derive(Clone)]
pub struct ScalarWord {
    pub reg_offset: u32,
    pub value_u32: u32,
}

/// Look up the register offset for an arg name in the scalar register map.
/// Falls back to `"{name}_offset"` if the exact name is not found.
pub fn lookup_register_offset(spec: &KernelSpec, name: &str) -> Option<u32> {
    spec.scalar_register_map
        .get(name)
        .or_else(|| {
            let key = format!("{name}_offset");
            spec.scalar_register_map.get(&key)
        })
        .copied()
}

/// Resolve the AXI-lite register offset an arg's value is written to.
///
/// Vitis-mode testbenches program args through the `s_axi_control`
/// register file, so a missing map entry there means every such write
/// would land on offset 0 and silently corrupt the kernel state — hard
/// error instead. HLS-mode testbenches drive the ports directly and
/// never touch the register file, so no offset is fine.
fn required_register_offset(spec: &KernelSpec, name: &str) -> Result<u32> {
    match lookup_register_offset(spec, name) {
        Some(offset) => Ok(offset),
        None if spec.mode == Mode::Hls => Ok(0),
        None => Err(CosimError::Metadata(format!(
            "no control register offset for arg {name:?}: the s_axi control register map is missing or incomplete"
        ))),
    }
}

/// Read all Verilog file contents for peek-port detection and similar scans.
pub fn read_verilog_contents(spec: &KernelSpec) -> Vec<String> {
    spec.verilog_files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .collect()
}

/// Direct-offset scalar port on the DUT module.
///
/// Vitis HLS through 2024.2 emits `<name>_offset` for `offset=direct`
/// `m_axi` arguments; 2025.1+ renamed the generated scalar to
/// `<name>_r`. Prefer the conventional spelling and fall back to the
/// 2025 one only when the RTL declares it exclusively, so the
/// testbench pins whatever port the packaged module actually has.
pub fn direct_offset_port_name(verilog_contents: &[String], name: &str) -> String {
    let conventional = format!("{name}_offset");
    if port_declared(verilog_contents, &conventional) {
        return conventional;
    }
    let vitis_2025 = format!("{name}_r");
    if port_declared(verilog_contents, &vitis_2025) {
        return vitis_2025;
    }
    conventional
}

/// Whether any Verilog file declares a port with this exact name.
fn port_declared(verilog_contents: &[String], port: &str) -> bool {
    let pattern = format!(
        r"(?m)\b(?:input|output|inout)\b[^;\n]*\b{}\b",
        regex_lite::escape(port)
    );
    let re = regex_lite::Regex::new(&pattern).unwrap();
    verilog_contents.iter().any(|text| re.is_match(text))
}

/// The four groups [`classify_args`] sorts a spec's args into:
/// mmaps, scalars, input streams, output streams.
pub type ClassifiedArgs = (Vec<MmapArg>, Vec<ScalarArg>, Vec<StreamArg>, Vec<StreamArg>);

/// Classify spec args into four groups by applying backend-specific constructors.
#[allow(
    clippy::implicit_hasher,
    reason = "generic hasher adds no value for internal helper"
)]
pub fn classify_args(
    spec: &KernelSpec,
    scalar_values: &HashMap<u32, Vec<u8>>,
    verilog_contents: &[String],
    make_mmap: impl Fn(&crate::metadata::ArgSpec, u32) -> MmapArg,
    make_scalar: impl Fn(&crate::metadata::ArgSpec, u32, u32, &[u8]) -> ScalarArg,
    make_stream: impl Fn(&crate::metadata::ArgSpec, usize, Option<String>, bool) -> StreamArg,
) -> Result<ClassifiedArgs> {
    use crate::metadata::{ArgKind, StreamDir, StreamProtocol};
    use names::{infer_peek_name, stream_peek_ports_exist};

    let mut mmaps = vec![];
    let mut scalars = vec![];
    let mut streams_in = vec![];
    let mut streams_out = vec![];

    for arg in &spec.args {
        match &arg.kind {
            ArgKind::Mmap { .. } => {
                let offset = required_register_offset(spec, &arg.name)?;
                mmaps.push(make_mmap(arg, offset));
            }
            ArgKind::Scalar { width } => {
                let offset = required_register_offset(spec, &arg.name)?;
                let bytes =
                    normalized_scalar_bytes(*width, scalar_values.get(&arg.id).map(Vec::as_slice));
                scalars.push(make_scalar(arg, *width, offset, &bytes));
            }
            ArgKind::Stream {
                width,
                dir,
                protocol,
                ..
            } => {
                let w = (*width as usize).div_ceil(8);
                let axis = *protocol == StreamProtocol::Axis;
                let peek = if spec.mode == Mode::Hls && *dir == StreamDir::In {
                    infer_peek_name(&arg.name).filter(|cand| {
                        stream_peek_ports_exist(verilog_contents, &spec.top_name, cand)
                    })
                } else {
                    None
                };
                let s = make_stream(arg, w, peek, axis);
                match dir {
                    StreamDir::In => streams_in.push(s),
                    StreamDir::Out => streams_out.push(s),
                }
            }
        }
    }
    Ok((mmaps, scalars, streams_in, streams_out))
}

/// How one backend spells the two kinds of name a testbench needs.
///
/// A backend is fully described by this pair, so adding one is a matter of
/// supplying it rather than duplicating the argument model below.
#[derive(Clone, Copy)]
pub struct Naming {
    /// A bare identifier safe to embed in generated variable names.
    pub ident: fn(&str) -> String,
    /// A reference to a port the packaged RTL already declares, in the
    /// backend's own syntax (escaped Verilog, a C++ member name, …).
    pub port: fn(&str) -> String,
}

impl Naming {
    /// The backend spelling of the port named `{prefix}{name}{suffix}`.
    fn signal(self, prefix: &str, name: &str, suffix: &str) -> String {
        (self.port)(&format!("{prefix}{name}{suffix}"))
    }
}

/// AXI4 signal names for a single mmap port, in one backend's spelling.
#[derive(Clone)]
pub struct AxiSignals {
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
}

impl AxiSignals {
    fn new(name: &str, naming: Naming) -> Self {
        let sig = |prefix: &str, name: &str, suffix: &str| naming.signal(prefix, name, suffix);
        Self {
            araddr: sig("m_axi_", name, "_ARADDR"),
            arburst: sig("m_axi_", name, "_ARBURST"),
            arcache: sig("m_axi_", name, "_ARCACHE"),
            arid: sig("m_axi_", name, "_ARID"),
            arlen: sig("m_axi_", name, "_ARLEN"),
            arlock: sig("m_axi_", name, "_ARLOCK"),
            arprot: sig("m_axi_", name, "_ARPROT"),
            arqos: sig("m_axi_", name, "_ARQOS"),
            arready: sig("m_axi_", name, "_ARREADY"),
            arsize: sig("m_axi_", name, "_ARSIZE"),
            arvalid: sig("m_axi_", name, "_ARVALID"),
            awaddr: sig("m_axi_", name, "_AWADDR"),
            awburst: sig("m_axi_", name, "_AWBURST"),
            awcache: sig("m_axi_", name, "_AWCACHE"),
            awid: sig("m_axi_", name, "_AWID"),
            awlen: sig("m_axi_", name, "_AWLEN"),
            awlock: sig("m_axi_", name, "_AWLOCK"),
            awprot: sig("m_axi_", name, "_AWPROT"),
            awqos: sig("m_axi_", name, "_AWQOS"),
            awready: sig("m_axi_", name, "_AWREADY"),
            awsize: sig("m_axi_", name, "_AWSIZE"),
            awvalid: sig("m_axi_", name, "_AWVALID"),
            bid: sig("m_axi_", name, "_BID"),
            bready: sig("m_axi_", name, "_BREADY"),
            bresp: sig("m_axi_", name, "_BRESP"),
            bvalid: sig("m_axi_", name, "_BVALID"),
            rdata: sig("m_axi_", name, "_RDATA"),
            rid: sig("m_axi_", name, "_RID"),
            rlast: sig("m_axi_", name, "_RLAST"),
            rready: sig("m_axi_", name, "_RREADY"),
            rresp: sig("m_axi_", name, "_RRESP"),
            rvalid: sig("m_axi_", name, "_RVALID"),
            wdata: sig("m_axi_", name, "_WDATA"),
            wlast: sig("m_axi_", name, "_WLAST"),
            wready: sig("m_axi_", name, "_WREADY"),
            wstrb: sig("m_axi_", name, "_WSTRB"),
            wvalid: sig("m_axi_", name, "_WVALID"),
        }
    }
}

/// Stream signal names for a single stream port.
#[derive(Clone)]
pub struct StreamSignals {
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
    pub peek_empty_n: String,
    pub peek_dout: String,
}

impl StreamSignals {
    fn new(name: &str, peek_name: &str, naming: Naming) -> Self {
        let sig = |prefix: &str, name: &str, suffix: &str| naming.signal(prefix, name, suffix);
        Self {
            empty_n: sig("", name, "_empty_n"),
            dout: sig("", name, "_dout"),
            din: sig("", name, "_din"),
            read: sig("", name, "_read"),
            full_n: sig("", name, "_full_n"),
            write: sig("", name, "_write"),
            tdata: sig("", name, "_TDATA"),
            tvalid: sig("", name, "_TVALID"),
            tready: sig("", name, "_TREADY"),
            tlast: sig("", name, "_TLAST"),
            peek_empty_n: sig("", peek_name, "_empty_n"),
            peek_dout: sig("", peek_name, "_dout"),
        }
    }
}

/// One mmap argument as both testbench templates see it.
#[derive(Clone)]
pub struct MmapArg {
    /// Raw argument name, as the host runtime knows it.
    pub name: String,
    /// Backend identifier used to name this argument's generated state.
    pub ident: String,
    /// Backend spelling of the direct-offset scalar port (`<name>_offset`,
    /// or `<name>_r` on Vitis HLS 2025.1+ modules).
    pub offset_port: String,
    pub axi: AxiSignals,
    pub data_width_bytes: usize,
    pub base_addr: u64,
    pub reg_offset_lo: u32,
    pub reg_offset_hi: u32,
    /// Bytes of device memory modeled for this argument. Only the Verilator
    /// backend models memory in the testbench; xsim leaves it zero.
    pub data_size: usize,
    /// ARID width the packaged RTL declares. Only xsim declares the AXI
    /// signals itself and so needs it; Verilator leaves it zero.
    pub id_width: usize,
}

impl MmapArg {
    fn new(
        naming: Naming,
        name: &str,
        offset_port: &str,
        data_width_bytes: usize,
        base_addr: u64,
        reg_offset_lo: u32,
    ) -> Self {
        Self {
            name: name.to_owned(),
            ident: (naming.ident)(name),
            offset_port: (naming.port)(offset_port),
            axi: AxiSignals::new(name, naming),
            data_width_bytes,
            base_addr,
            reg_offset_lo,
            reg_offset_hi: reg_offset_lo + 4,
            data_size: 0,
            id_width: 0,
        }
    }
}

/// One scalar argument as both testbench templates see it.
#[derive(Clone)]
pub struct ScalarArg {
    /// Backend spelling of the DUT port carrying this scalar.
    pub port: String,
    /// The value, spelled in the template's own language.
    pub value: String,
    /// The `s_axi_control` writes that program the value.
    pub words: Vec<ScalarWord>,
}

impl ScalarArg {
    fn new(naming: Naming, name: &str, value: String, reg_offset: u32, bytes: &[u8]) -> Self {
        Self {
            port: (naming.port)(name),
            value,
            words: scalar_words(reg_offset, bytes),
        }
    }
}

/// One stream argument as both testbench templates see it.
#[derive(Clone)]
pub struct StreamArg {
    /// Raw argument name, as the host runtime knows it.
    pub name: String,
    /// Backend identifier used to name this argument's generated state.
    pub ident: String,
    pub sig: StreamSignals,
    pub width_bytes: usize,
    /// Total bytes passed to/from the DPI function. Always `width_bytes + 1`:
    /// the extra byte carries the EOS/TLAST flag. For AXIS streams this maps
    /// to TLAST; for `ApFifo` streams it maps to the MSB of the `dout`/`din`
    /// port.
    pub dpi_width_bytes: usize,
    /// True when the stream uses AXI-Stream (Vitis mode).
    pub axis: bool,
    /// True when the RTL declares the companion peek ports.
    pub has_peek: bool,
}

impl StreamArg {
    fn new(
        naming: Naming,
        name: &str,
        width_bytes: usize,
        peek: Option<String>,
        axis: bool,
    ) -> Self {
        let peek_name = peek.unwrap_or_default();
        Self {
            name: name.to_owned(),
            ident: (naming.ident)(name),
            sig: StreamSignals::new(name, &peek_name, naming),
            width_bytes,
            dpi_width_bytes: width_bytes + 1,
            axis,
            has_peek: !peek_name.is_empty(),
        }
    }
}

pub fn scalar_words(base_offset: u32, bytes: &[u8]) -> Vec<ScalarWord> {
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

#[cfg(test)]
mod tests {
    use super::direct_offset_port_name;
    use super::{classify_args, KernelSpec, Mode};
    use crate::metadata::{ArgKind, ArgSpec};
    use std::collections::HashMap;

    fn scalar_spec(mode: Mode, register_map: HashMap<String, u32>) -> KernelSpec {
        KernelSpec {
            top_name: "Top".to_owned(),
            mode,
            args: vec![ArgSpec {
                name: "n".to_owned(),
                id: 0,
                kind: ArgKind::Scalar { width: 32 },
            }],
            part_num: None,
            verilog_files: vec![],
            tcl_files: vec![],
            xci_files: vec![],
            scalar_register_map: register_map,
        }
    }

    fn classify_offsets(spec: &KernelSpec) -> crate::error::Result<Vec<u32>> {
        let naming = super::Naming {
            ident: str::to_owned,
            port: str::to_owned,
        };
        let (_, scalars, _, _) = classify_args(
            spec,
            &HashMap::new(),
            &[],
            |arg, offset| super::MmapArg::new(naming, &arg.name, &arg.name, 4, 0, offset),
            |arg, _, offset, bytes| {
                super::ScalarArg::new(naming, &arg.name, String::new(), offset, bytes)
            },
            |arg, width, peek, axis| super::StreamArg::new(naming, &arg.name, width, peek, axis),
        )?;
        Ok(scalars
            .iter()
            .map(|scalar| scalar.words[0].reg_offset)
            .collect())
    }

    /// A Vitis-mode arg missing from the control register map must be
    /// an error: falling back to offset 0 writes every such scalar over
    /// the control registers and silently corrupts the cosimulation.
    #[test]
    fn missing_register_offset_is_an_error_in_vitis_mode() {
        let spec = scalar_spec(Mode::Vitis, HashMap::new());
        let error = classify_offsets(&spec).expect_err("offset 0 fallback must not happen");
        assert!(
            error.to_string().contains("control register offset"),
            "unexpected error: {error}"
        );
    }

    /// HLS-mode testbenches drive ports directly and never consult the
    /// register file, so an empty map stays fine there.
    #[test]
    fn missing_register_offset_is_fine_in_hls_mode() {
        let spec = scalar_spec(Mode::Hls, HashMap::new());
        assert_eq!(classify_offsets(&spec).expect("classify"), vec![0]);
    }

    #[test]
    fn register_offset_falls_back_to_offset_spelling() {
        let spec = scalar_spec(
            Mode::Vitis,
            HashMap::from([("n_offset".to_owned(), 0x1c_u32)]),
        );
        assert_eq!(classify_offsets(&spec).expect("classify"), vec![0x1c]);
    }

    #[test]
    fn direct_offset_prefers_conventional_spelling() {
        let rtl = vec!["module m(mmap_offset);\ninput [63:0] mmap_offset;\nendmodule".to_owned()];
        assert_eq!(direct_offset_port_name(&rtl, "mmap"), "mmap_offset");
    }

    #[test]
    fn direct_offset_falls_back_to_vitis_2025_spelling() {
        let rtl = vec!["module m(mmap_r);\ninput [63:0] mmap_r;\nendmodule".to_owned()];
        assert_eq!(direct_offset_port_name(&rtl, "mmap"), "mmap_r");
    }

    #[test]
    fn direct_offset_defaults_to_conventional_when_neither_declared() {
        let rtl = vec!["module m(ap_clk);\ninput ap_clk;\nendmodule".to_owned()];
        assert_eq!(direct_offset_port_name(&rtl, "mmap"), "mmap_offset");
    }

    #[test]
    fn direct_offset_ignores_non_declaration_mentions() {
        // `mmap_offset` appearing only in a comment must not mask the
        // actually-declared 2025-style port.
        let rtl = vec![
            "// carries mmap_offset semantics\nmodule m(mmap_r);\ninput [63:0] mmap_r;\nendmodule"
                .to_owned(),
        ];
        assert_eq!(direct_offset_port_name(&rtl, "mmap"), "mmap_r");
    }

    /// The probe order is shared with `tapa-codegen`'s child-instance
    /// pinning through the cross-tool naming fixture; both tests read
    /// the same file, so the two implementations cannot drift apart
    /// (the 2025.2 `_offset` -> `_r` rename had to be fixed in both
    /// independently).
    #[test]
    fn direct_offset_probe_order_follows_naming_fixture() {
        let fixture = include_str!("../../../../tapa-core/tapa-ir/testdata/naming_conventions.tsv");
        let mut checked = 0;
        for line in fixture
            .lines()
            .filter(|line| line.starts_with("direct_offset_port\t"))
        {
            let fields: Vec<&str> = line.split('\t').collect();
            let (base, candidates) = (fields[1], &fields[2..]);
            assert!(candidates.len() >= 2, "probe list needs candidates: {line}");
            // RTL declaring exactly one candidate binds that port.
            for expected in candidates {
                let rtl = vec![format!(
                    "module m({expected});\ninput [63:0] {expected};\nendmodule"
                )];
                assert_eq!(
                    direct_offset_port_name(&rtl, base),
                    *expected,
                    "line: {line}"
                );
            }
            // All candidates declared resolves in fixture probe order;
            // none declared falls back to the first spelling.
            let declarations = candidates
                .iter()
                .map(|candidate| format!("input [63:0] {candidate};"))
                .collect::<Vec<_>>()
                .join("\n");
            let rtl = vec![format!("module m();\n{declarations}\nendmodule")];
            assert_eq!(
                direct_offset_port_name(&rtl, base),
                candidates[0],
                "line: {line}"
            );
            assert_eq!(
                direct_offset_port_name(&[], base),
                candidates[0],
                "line: {line}"
            );
            checked += 1;
        }
        assert!(
            checked >= 1,
            "fixture lost its direct_offset_port production"
        );
    }
}
