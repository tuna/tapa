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
use serde::{Deserialize, Deserializer};
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
#[derive(Default, Deserialize)]
#[serde(default)]
struct PortInfo {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@mode")]
    mode: String,
    #[serde(rename = "@dataWidth", deserialize_with = "optional_width")]
    data_width: Option<u32>,
}

/// Raw attributes retain Vitis's distinct bit-width and byte-size units.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawArg {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@id", deserialize_with = "arg_id")]
    id: u32,
    #[serde(rename = "@addressQualifier", deserialize_with = "arg_qualifier")]
    qualifier: u32,
    #[serde(rename = "@port")]
    port: String,
    #[serde(
        rename = "@dataWidth",
        alias = "@width",
        deserialize_with = "optional_width"
    )]
    data_width: Option<u32>,
    #[serde(rename = "@addrWidth", deserialize_with = "optional_width")]
    addr_width: Option<u32>,
    #[serde(rename = "@depth", deserialize_with = "stream_depth")]
    depth: Option<u32>,
    #[serde(rename = "@hostSize", deserialize_with = "optional_size")]
    host_size_bytes: Option<u32>,
    #[serde(rename = "@size", deserialize_with = "optional_size")]
    size_bytes: Option<u32>,
    #[serde(rename = "@type")]
    c_type: String,
}

#[derive(Default, Deserialize)]
struct Ports {
    #[serde(rename = "port", default)]
    entries: Vec<PortInfo>,
}

#[derive(Default, Deserialize)]
struct Args {
    #[serde(rename = "arg", default)]
    entries: Vec<RawArg>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawKernel {
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "$value")]
    entries: Vec<KernelElement>,
}

/// XO metadata groups declarations; Vitis flattens them in xclbin metadata.
/// Keeping one ordered sequence also handles mixed layouts without reordering args.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum KernelElement {
    Port(PortInfo),
    Ports(Ports),
    Arg(RawArg),
    Args(Args),
    #[serde(other)]
    Other,
}

fn optional_width<'de, D: Deserializer<'de>>(de: D) -> std::result::Result<Option<u32>, D::Error> {
    Ok(String::deserialize(de)?.parse().ok())
}

fn optional_size<'de, D: Deserializer<'de>>(de: D) -> std::result::Result<Option<u32>, D::Error> {
    Ok(parse_size_bytes(&String::deserialize(de)?))
}

pub fn parse(xml: &str) -> Result<KernelXml> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    reader.config_mut().expand_empty_elements = true;
    let mut kernel: Option<RawKernel> = None;
    let mut platform = String::new();
    let mut target = XclbinTarget::Flat;

    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"kernel" => {
                    if kernel.is_some() {
                        return Err(CosimError::Metadata(
                            "multiple <kernel> elements in kernel metadata XML; cosim packages must contain exactly one kernel".into(),
                        ));
                    }
                    reader
                        .read_to_end(e.name())
                        .map_err(|e| CosimError::Metadata(e.to_string()))?;
                    kernel = Some(
                        quick_xml::de::from_str(&xml[start..reader.buffer_position() as usize])
                            .map_err(|e| CosimError::Metadata(e.to_string()))?,
                    );
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
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CosimError::Metadata(e.to_string())),
            _ => {}
        }
    }

    let kernel = kernel
        .filter(|kernel| !kernel.name.is_empty())
        .ok_or_else(|| {
            CosimError::Metadata("no kernel name found in kernel metadata XML".into())
        })?;
    let mut ports = Vec::new();
    let mut raw_args = Vec::new();
    for entry in kernel.entries {
        match entry {
            KernelElement::Port(port) => ports.push(port),
            KernelElement::Ports(group) => ports.extend(group.entries),
            KernelElement::Arg(arg) => raw_args.push(arg),
            KernelElement::Args(group) => raw_args.extend(group.entries),
            KernelElement::Other => {}
        }
    }
    let ports = ports
        .into_iter()
        .filter(|port| !port.name.is_empty())
        .map(|port| (port.name.clone(), port))
        .collect();
    // Resolve only after reading all ports: XML child order does not change the ABI.
    let args = raw_args
        .into_iter()
        .map(|arg| resolve_arg(arg, &ports))
        .collect::<Result<_>>()?;
    Ok(KernelXml {
        top_name: kernel.name,
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

fn parse_attr(value: &str, attr: &str) -> Result<u32> {
    value.parse().map_err(|_parse_err| {
        CosimError::Metadata(format!("malformed {attr} {value:?} in kernel metadata XML"))
    })
}

fn arg_id<'de, D: Deserializer<'de>>(de: D) -> std::result::Result<u32, D::Error> {
    parse_attr(&String::deserialize(de)?, "id").map_err(serde::de::Error::custom)
}

fn arg_qualifier<'de, D: Deserializer<'de>>(de: D) -> std::result::Result<u32, D::Error> {
    parse_attr(&String::deserialize(de)?, "addressQualifier").map_err(serde::de::Error::custom)
}

fn stream_depth<'de, D: Deserializer<'de>>(de: D) -> std::result::Result<Option<u32>, D::Error> {
    let value = String::deserialize(de)?;
    value
        .parse::<u32>()
        .ok()
        .filter(|depth| *depth > 0)
        .map(Some)
        .ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid stream depth {value:?} in kernel metadata XML (want an integer >= 1)"
            ))
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
                .map(|p| p.data_width.unwrap_or(DEFAULT_DATA_WIDTH))
                .or(arg.data_width)
                .unwrap_or(DEFAULT_DATA_WIDTH),
            addr_width: arg.addr_width.unwrap_or(DEFAULT_ADDR_WIDTH),
        },
        4 => ArgKind::Stream {
            width: port
                .map(|p| p.data_width.unwrap_or(DEFAULT_DATA_WIDTH))
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
      <kernel name="vadd">
        <port name="m_axi_a" mode="master" dataWidth="512"/>
        <arg name="a" addressQualifier="1" id="0" port="m_axi_a" size="0x8"/>
        <arg name="n" addressQualifier="0" id="1" type="uint32_t" hostSize="0x4"/>
      </kernel>
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
  <args>
    <arg name="a" addressQualifier="1" id="0" port="m_axi_a" dataWidth="32"/>
    <arg name="s" addressQualifier="4" id="1" port="s&amp;t" depth="8"/>
    <arg name="t" addressQualifier="4" id="2" port="t"/>
  </args>
  <ports>
    <port name="m_axi_a" mode="master" dataWidth="512"/>
    <port name="s&amp;t" mode="read_only" dataWidth="128"/>
    <port name="t" mode="write_only" dataWidth="64"/>
  </ports>
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
