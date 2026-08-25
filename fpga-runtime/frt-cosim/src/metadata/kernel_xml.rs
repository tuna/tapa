//! The one reader for Vitis kernel metadata XML.
//!
//! The same document describes a kernel whether it arrives as `kernel.xml`
//! inside an `.xo`/`.zip` or as the `EMBEDDED_METADATA` section of an
//! `.xclbin`. Both runtimes read it through this module; the fields each
//! one ignores are simply the fields it does not need.

use super::{ArgKind, ArgSpec, StreamDir, StreamProtocol};
use crate::error::{CosimError, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;

/// What the `<core target="...">` attribute says the binary was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XclbinTarget {
    /// Real hardware.
    Flat,
    HwEmu,
    SwEmu,
}

/// Everything this XML states about a kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelXml {
    pub top_name: String,
    /// `<platform name="...">`, empty when the document omits it (as
    /// `kernel.xml` inside an `.xo` does).
    pub platform: String,
    pub target: XclbinTarget,
    pub args: Vec<ArgSpec>,
}

/// Port attributes an `<arg>` defers to: mmap and stream widths live on
/// `<port>`, and a stream's direction is only stated there.
struct PortInfo {
    mode: String,
    data_width: u32,
}

/// Raw `<arg>` attributes, before they are resolved into an [`ArgKind`].
#[derive(Default)]
struct RawArg {
    name: String,
    id: u32,
    qualifier: u32,
    port: String,
    /// `dataWidth` or `width` — a bit width when present.
    data_width: Option<u32>,
    addr_width: Option<u32>,
    depth: Option<u32>,
    /// `hostSize`, the generator's logical C width in bytes.
    host_size_bytes: Option<u32>,
    /// `size`, the `s_axi` register footprint in bytes.
    size_bytes: Option<u32>,
    /// `type`, which may be a bare typedef name.
    c_type: String,
}

pub fn parse(xml: &str) -> Result<KernelXml> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut top_name = String::new();
    let mut platform = String::new();
    let mut target = XclbinTarget::Flat;
    let mut ports: HashMap<String, PortInfo> = HashMap::new();
    let mut args = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => match e.name().as_ref() {
                b"kernel" => {
                    // A multi-kernel package has one <kernel> per kernel;
                    // merging their args would alias registers across
                    // kernels, so refuse rather than pick one silently.
                    if !top_name.is_empty() {
                        return Err(CosimError::Metadata(
                            "multiple <kernel> elements in kernel metadata XML;                              cosim packages must contain exactly one kernel"
                                .into(),
                        ));
                    }
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == b"name" {
                            top_name = String::from_utf8_lossy(&a.value).into_owned();
                        }
                    }
                }
                b"platform" => {
                    if platform.is_empty() {
                        platform = platform_name(&e);
                    }
                }
                b"core" => {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == b"target" {
                            target = parse_target(&String::from_utf8_lossy(&a.value));
                        }
                    }
                }
                b"port" => {
                    let mut name = String::new();
                    let mut info = PortInfo {
                        mode: String::new(),
                        data_width: DEFAULT_DATA_WIDTH,
                    };
                    for a in e.attributes().flatten() {
                        let v = String::from_utf8_lossy(&a.value).into_owned();
                        match a.key.as_ref() {
                            b"name" => name = v,
                            b"mode" => info.mode = v,
                            b"dataWidth" => {
                                info.data_width = v.parse().unwrap_or(DEFAULT_DATA_WIDTH);
                            }
                            _ => {}
                        }
                    }
                    if !name.is_empty() {
                        ports.insert(name, info);
                    }
                }
                // TAPA emits <ports> before <args>, so the port table is
                // complete by the time an <arg> needs it.
                b"arg" => args.push(resolve_arg(read_arg(&e)?, &ports)?),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CosimError::Metadata(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if top_name.is_empty() {
        return Err(CosimError::Metadata(
            "no kernel name found in kernel metadata XML".into(),
        ));
    }

    Ok(KernelXml {
        top_name,
        platform,
        target,
        args,
    })
}

/// Vitis writes 32-bit ports when it writes no width at all.
const DEFAULT_DATA_WIDTH: u32 = 32;
/// Vitis's own default AXI address width.
const DEFAULT_ADDR_WIDTH: u32 = 64;
/// Stream depth when the XML states none.
const DEFAULT_STREAM_DEPTH: u32 = 16;

fn platform_name(e: &quick_xml::events::BytesStart) -> String {
    for a in e.attributes().flatten() {
        let key = a.key.as_ref();
        if key == b"name" || key == b"vbnv" || key == b"platformVBNV" {
            let value = String::from_utf8_lossy(&a.value).trim().to_owned();
            if !value.is_empty() {
                return value;
            }
        }
    }
    String::new()
}

fn parse_target(raw: &str) -> XclbinTarget {
    let target = raw.to_ascii_lowercase();
    if target.contains("hw_em") {
        XclbinTarget::HwEmu
    } else if target.contains("csim") || target.contains("sw_em") {
        XclbinTarget::SwEmu
    } else {
        XclbinTarget::Flat
    }
}

fn read_arg(e: &quick_xml::events::BytesStart) -> Result<RawArg> {
    let mut arg = RawArg::default();
    for a in e.attributes().flatten() {
        let v = String::from_utf8_lossy(&a.value).into_owned();
        match a.key.as_ref() {
            b"name" => arg.name = v,
            // A malformed id would silently alias another argument's slot;
            // fail loudly instead.
            b"id" => arg.id = parse_attr(&v, "id")?,
            b"addressQualifier" => arg.qualifier = parse_attr(&v, "addressQualifier")?,
            b"port" => arg.port = v,
            b"dataWidth" | b"width" => arg.data_width = v.parse().ok(),
            b"addrWidth" => arg.addr_width = v.parse().ok(),
            // Stream queues take a modulo by the depth, so a zero or an
            // unparsable value turns into a runtime panic further down.
            b"depth" => {
                arg.depth = Some(v.parse().ok().filter(|d| *d > 0).ok_or_else(|| {
                    CosimError::Metadata(format!(
                        "invalid stream depth {v:?} in kernel metadata XML (want an integer >= 1)"
                    ))
                })?);
            }
            b"hostSize" => arg.host_size_bytes = parse_size_bytes(&v),
            b"size" => arg.size_bytes = parse_size_bytes(&v),
            b"type" => arg.c_type = v,
            _ => {}
        }
    }
    Ok(arg)
}

fn parse_attr(value: &str, attr: &str) -> Result<u32> {
    value.parse().map_err(|_parse_err| {
        CosimError::Metadata(format!("malformed {attr} {value:?} in kernel metadata XML"))
    })
}

fn resolve_arg(arg: RawArg, ports: &HashMap<String, PortInfo>) -> Result<ArgSpec> {
    let port = ports.get(&arg.port);
    let kind = match arg.qualifier {
        0 => ArgKind::Scalar {
            width: scalar_width(&arg)?,
        },
        1 => ArgKind::Mmap {
            // The width lives on the port (`m_axi_<name>`) when there is
            // one; `size` does not help here, being the 8-byte pointer.
            data_width: port
                .map(|p| p.data_width)
                .or(arg.data_width)
                .unwrap_or(DEFAULT_DATA_WIDTH),
            addr_width: arg.addr_width.unwrap_or(DEFAULT_ADDR_WIDTH),
        },
        4 => ArgKind::Stream {
            width: port
                .map(|p| p.data_width)
                .or(arg.data_width)
                .unwrap_or(DEFAULT_DATA_WIDTH),
            depth: arg.depth.unwrap_or(DEFAULT_STREAM_DEPTH),
            dir: stream_dir(port, &arg.port),
            protocol: StreamProtocol::Axis,
        },
        q => {
            return Err(CosimError::Metadata(format!(
                "unknown addressQualifier {q} for arg {}",
                arg.name
            )))
        }
    };
    Ok(ArgSpec {
        name: arg.name,
        id: arg.id,
        kind,
    })
}

/// Scalar register width must match the kernel's declaration or the `OpenCL`
/// driver rejects the argument, so there is no silent default: every
/// historical wrong-width bug surfaced as an opaque `clSetKernelArg`
/// failure at run time. `hostSize` is the generator's logical C width in
/// bytes; `size` is the `s_axi` register footprint, 4-byte-padded for
/// sub-32-bit scalars (a `uint16_t` arg ships `hostSize="0x2"` with
/// `size="0x4"`); `type` can be a bare typedef name. Rank accordingly.
fn scalar_width(arg: &RawArg) -> Result<u32> {
    arg.data_width
        .or_else(|| arg.host_size_bytes.map(|b| b.saturating_mul(8)))
        .or_else(|| c_scalar_type_bits(&arg.c_type))
        .or_else(|| arg.size_bytes.map(|b| b.saturating_mul(8)))
        .ok_or_else(|| {
            CosimError::Metadata(format!(
                "cannot determine scalar width for arg {:?}: no dataWidth/width, hostSize, \
                 recognizable type (got {:?}), or size attribute",
                arg.name, arg.c_type
            ))
        })
}

/// TAPA names stream ports after the bare argument (`a`), so the
/// `s_axis`/`istream` spelling only decides for foreign `.xo` files that
/// carry no `<port mode="...">`.
fn stream_dir(port: Option<&PortInfo>, port_name: &str) -> StreamDir {
    match port.map(|p| p.mode.as_str()) {
        Some("read_only") => StreamDir::In,
        Some("write_only") => StreamDir::Out,
        _ => {
            if port_name.starts_with("s_axis") || port_name.contains("istream") {
                StreamDir::In
            } else {
                StreamDir::Out
            }
        }
    }
}

/// Bit width of a primitive C scalar type name from Vitis arg metadata
/// (`type="uint16_t"`); returns `None` for pointers, composites, and other
/// non-primitive spellings so callers can fall back to `size`.
fn c_scalar_type_bits(ty: &str) -> Option<u32> {
    let t = ty.trim().trim_start_matches("const").trim();
    match t {
        "bool" | "char" | "signed char" | "unsigned char" | "int8_t" | "uint8_t" => Some(8),
        "short" | "short int" | "unsigned short" | "unsigned short int" | "int16_t"
        | "uint16_t" => Some(16),
        "int" | "unsigned" | "unsigned int" | "int32_t" | "uint32_t" | "float" => Some(32),
        "long" | "long int" | "unsigned long" | "unsigned long int" | "long long"
        | "unsigned long long" | "int64_t" | "uint64_t" | "double" => Some(64),
        _ => None,
    }
}

/// Parse a Vitis XML `size` attribute (hex `0x..` or decimal) into bytes.
fn parse_size_bytes(value: &str) -> Option<u32> {
    let s = value.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg<'a>(parsed: &'a KernelXml, name: &str) -> &'a ArgSpec {
        parsed
            .args
            .iter()
            .find(|a| a.name == name)
            .unwrap_or_else(|| panic!("no arg {name:?} in {:?}", parsed.args))
    }

    #[test]
    fn multiple_kernels_are_rejected_instead_of_merged() {
        let err = parse(
            r#"<?xml version="1.0"?>
<project>
  <kernel name="a"><args>
    <arg name="x" addressQualifier="0" id="0" dataWidth="32"/>
  </args></kernel>
  <kernel name="b"><args>
    <arg name="y" addressQualifier="0" id="0" dataWidth="32"/>
  </args></kernel>
</project>"#,
        )
        .expect_err("two <kernel> elements must not merge into one arg list");
        assert!(err.to_string().contains("multiple <kernel>"), "{err}");
    }

    #[test]
    fn xclbin_metadata_yields_kernel_platform_and_target() {
        let parsed = parse(
            r#"<?xml version="1.0"?>
<project>
  <platform name="xilinx_u250_gen3x16_xdma_3_1_202020_1">
    <device><core target="hw_em">
      <kernel name="vadd"><args>
        <arg name="a" addressQualifier="1" id="0" dataWidth="512" addrWidth="64"/>
        <arg name="n" addressQualifier="0" id="1" dataWidth="32"/>
      </args></kernel>
    </core></device>
  </platform>
</project>"#,
        )
        .expect("parse");
        assert_eq!(parsed.top_name, "vadd");
        assert_eq!(parsed.platform, "xilinx_u250_gen3x16_xdma_3_1_202020_1");
        assert_eq!(parsed.target, XclbinTarget::HwEmu);
        assert_eq!(
            arg(&parsed, "a").kind,
            ArgKind::Mmap {
                data_width: 512,
                addr_width: 64
            }
        );
        assert_eq!(arg(&parsed, "n").kind, ArgKind::Scalar { width: 32 });
    }

    #[test]
    fn a_kernel_xml_without_a_platform_still_parses() {
        let parsed = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="vadd"><args>
  <arg name="n" addressQualifier="0" id="0" dataWidth="32"/>
</args></kernel></root>"#,
        )
        .expect("parse");
        assert_eq!(parsed.top_name, "vadd");
        assert!(parsed.platform.is_empty());
        assert_eq!(parsed.target, XclbinTarget::Flat);
    }

    #[test]
    fn sw_emu_targets_are_recognized_under_both_spellings() {
        assert_eq!(parse_target("sw_emu"), XclbinTarget::SwEmu);
        assert_eq!(parse_target("csim"), XclbinTarget::SwEmu);
        assert_eq!(parse_target("hw_emu"), XclbinTarget::HwEmu);
        assert_eq!(parse_target("hw"), XclbinTarget::Flat);
    }

    #[test]
    fn mmap_and_stream_widths_come_from_the_port_table() {
        let parsed = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="top">
  <ports>
    <port name="m_axi_a" mode="master" dataWidth="512"/>
    <port name="s" mode="read_only" dataWidth="128"/>
    <port name="t" mode="write_only" dataWidth="64"/>
  </ports>
  <args>
    <arg name="a" addressQualifier="1" id="0" port="m_axi_a" dataWidth="32"/>
    <arg name="s" addressQualifier="4" id="1" port="s" depth="8"/>
    <arg name="t" addressQualifier="4" id="2" port="t"/>
  </args>
</kernel></root>"#,
        )
        .expect("parse");
        assert_eq!(
            arg(&parsed, "a").kind,
            ArgKind::Mmap {
                data_width: 512,
                addr_width: DEFAULT_ADDR_WIDTH
            }
        );
        assert_eq!(
            arg(&parsed, "s").kind,
            ArgKind::Stream {
                width: 128,
                depth: 8,
                dir: StreamDir::In,
                protocol: StreamProtocol::Axis
            }
        );
        assert_eq!(
            arg(&parsed, "t").kind,
            ArgKind::Stream {
                width: 64,
                depth: DEFAULT_STREAM_DEPTH,
                dir: StreamDir::Out,
                protocol: StreamProtocol::Axis
            }
        );
    }

    #[test]
    fn stream_direction_falls_back_to_the_port_name_without_a_port_table() {
        let parsed = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="top"><args>
  <arg name="a" addressQualifier="4" id="0" port="s_axis_a"/>
  <arg name="b" addressQualifier="4" id="1" port="m_axis_b"/>
</args></kernel></root>"#,
        )
        .expect("parse");
        assert!(matches!(
            arg(&parsed, "a").kind,
            ArgKind::Stream {
                dir: StreamDir::In,
                ..
            }
        ));
        assert!(matches!(
            arg(&parsed, "b").kind,
            ArgKind::Stream {
                dir: StreamDir::Out,
                ..
            }
        ));
    }

    #[test]
    fn scalar_width_falls_back_to_vitis_size_bytes() {
        // Real Vitis xclbin XML carries the C byte size, not a bit width.
        let parsed = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="n" addressQualifier="0" id="0" size="0x8" offset="0x10"/>
</args></kernel></root>"#,
        )
        .expect("parse");
        assert_eq!(arg(&parsed, "n").kind, ArgKind::Scalar { width: 64 });
    }

    #[test]
    fn typedef_id_width_comes_from_host_size() {
        // A `Pid` typedef: register-padded `size`, true width in `hostSize`.
        let parsed = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="pid" addressQualifier="0" id="0" type="Pid" hostSize="0x2" size="0x4"/>
</args></kernel></root>"#,
        )
        .expect("parse");
        assert_eq!(arg(&parsed, "pid").kind, ArgKind::Scalar { width: 16 });
    }

    #[test]
    fn explicit_bit_width_wins_over_size() {
        let parsed = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="n" addressQualifier="0" id="0" dataWidth="16" size="0x4"/>
</args></kernel></root>"#,
        )
        .expect("parse");
        assert_eq!(arg(&parsed, "n").kind, ArgKind::Scalar { width: 16 });
    }

    #[test]
    fn a_scalar_with_no_width_source_is_an_error_naming_the_arg() {
        let err = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="mystery" addressQualifier="0" id="0" type="WeirdStruct"/>
</args></kernel></root>"#,
        )
        .expect_err("no width source");
        let msg = err.to_string();
        assert!(msg.contains("mystery"), "{msg}");
        assert!(msg.contains("WeirdStruct"), "{msg}");
    }

    #[test]
    fn a_malformed_arg_id_is_an_error_not_arg_zero() {
        let err = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="n" addressQualifier="0" id="0x1" dataWidth="32"/>
</args></kernel></root>"#,
        )
        .expect_err("malformed id");
        assert!(err.to_string().contains("malformed id"), "{err}");
    }

    #[test]
    fn a_zero_stream_depth_is_an_error() {
        let err = parse(
            r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="s" addressQualifier="4" id="0" port="s" depth="0"/>
</args></kernel></root>"#,
        )
        .expect_err("zero depth");
        assert!(err.to_string().contains("invalid stream depth"), "{err}");
    }

    #[test]
    fn a_missing_kernel_name_is_an_error() {
        let err = parse(r#"<?xml version="1.0"?><root><args/></root>"#).expect_err("no kernel");
        assert!(err.to_string().contains("no kernel name"), "{err}");
    }
}
