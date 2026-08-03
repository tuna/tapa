use crate::error::{FrtError, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XrtArgKind {
    Scalar { width: u32 },
    Mmap { data_width: u32 },
    Stream { width: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrtArg {
    pub name: String,
    pub id: u32,
    pub kind: XrtArgKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XrtMetadata {
    pub top_name: String,
    pub args: Vec<XrtArg>,
    pub platform: String,
    pub mode: XclbinMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XclbinMode {
    Flat,
    HwEmu,
    SwEmu,
}

pub fn parse_embedded_xml(xml: &str) -> Result<XrtMetadata> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut top_name = String::new();
    let mut platform = String::new();
    let mut mode = XclbinMode::Flat;
    let mut args = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => match e.name().as_ref() {
                b"kernel" => {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == b"name" {
                            top_name = String::from_utf8_lossy(&a.value).into_owned();
                        }
                    }
                }
                b"platform" => {
                    if platform.is_empty() {
                        for a in e.attributes().flatten() {
                            let key = a.key.as_ref();
                            if key == b"name" || key == b"vbnv" || key == b"platformVBNV" {
                                let value = String::from_utf8_lossy(&a.value).trim().to_owned();
                                if !value.is_empty() {
                                    platform = value;
                                    break;
                                }
                            }
                        }
                    }
                }
                b"core" => {
                    for a in e.attributes().flatten() {
                        if a.key.as_ref() == b"target" {
                            let target = String::from_utf8_lossy(&a.value).to_ascii_lowercase();
                            if target.contains("hw_em") || target.contains("hw_emu") {
                                mode = XclbinMode::HwEmu;
                            } else if target.contains("csim")
                                || target.contains("sw_emu")
                                || target.contains("sw_em")
                            {
                                mode = XclbinMode::SwEmu;
                            }
                        }
                    }
                }
                b"arg" => {
                    let mut name = String::new();
                    let mut id = 0u32;
                    let mut qualifier = 0u32;
                    // `dataWidth`/`width` come from hand-written fixtures and
                    // cosim-style XML; real Vitis xclbin XML instead carries
                    // the C byte size of the argument (`size="0x8"` for a
                    // `uint64_t` scalar, `size="0x2"` for `uint16_t`).
                    let mut data_width = None;
                    let mut size_bytes = None;
                    let mut c_type = String::new();
                    for a in e.attributes().flatten() {
                        let v = String::from_utf8_lossy(&a.value).into_owned();
                        match a.key.as_ref() {
                            b"name" => name = v,
                            b"id" => id = v.parse().unwrap_or(0),
                            b"addressQualifier" => qualifier = v.parse().unwrap_or(0),
                            b"dataWidth" | b"width" => data_width = v.parse().ok(),
                            b"size" => size_bytes = parse_size_bytes(&v),
                            b"type" => c_type = v,
                            _ => {}
                        }
                    }
                    let kind = match qualifier {
                        // Scalar register width must match the kernel's
                        // declaration or the OpenCL driver rejects the arg.
                        // The C `type` carries the logical width; `size` can
                        // instead hold the 4-byte-aligned s_axi register
                        // footprint of a narrower scalar (e.g. `uint16_t`
                        // with `size="0x4"`), so type wins over size.
                        0 => XrtArgKind::Scalar {
                            width: data_width
                                .or_else(|| c_scalar_type_bits(&c_type))
                                .or_else(|| size_bytes.map(|b| b.saturating_mul(8)))
                                .unwrap_or(32),
                        },
                        // For mmap args `size` is the 8-byte pointer size, not
                        // the bus data width, so it is not a width fallback.
                        1 => XrtArgKind::Mmap {
                            data_width: data_width.unwrap_or(32),
                        },
                        4 => XrtArgKind::Stream {
                            width: data_width.unwrap_or(32),
                        },
                        q => return Err(FrtError::MetadataParse(format!("unknown qualifier {q}"))),
                    };
                    args.push(XrtArg { name, id, kind });
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(FrtError::MetadataParse(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if top_name.is_empty() {
        return Err(FrtError::MetadataParse(
            "kernel name missing from embedded XML".into(),
        ));
    }

    Ok(XrtMetadata {
        top_name,
        args,
        platform,
        mode,
    })
}

/// Extract the platform VBNV string from the xclbin2 binary header.
///
/// The old C++ runtime read `axlf_top->m_header.m_platformVBNV` (a 64-byte
/// null-terminated string at offset 352) which always contains the full
/// platform identifier (e.g. `xilinx_u250_gen3x16_xdma_4_1_202210_1`).
/// The XML `<platform name="...">` attribute may carry a shorter value in
/// some xclbin versions, so we prefer the header field.
pub fn extract_platform_vbnv(xclbin: &[u8]) -> Option<String> {
    const PLATFORM_VBNV_OFFSET: usize = 352;
    const PLATFORM_VBNV_LEN: usize = 64;

    if xclbin.len() < PLATFORM_VBNV_OFFSET + PLATFORM_VBNV_LEN {
        return None;
    }
    let raw = &xclbin[PLATFORM_VBNV_OFFSET..PLATFORM_VBNV_OFFSET + PLATFORM_VBNV_LEN];
    let end = raw
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(PLATFORM_VBNV_LEN);
    let s = std::str::from_utf8(&raw[..end]).ok()?.trim().to_owned();
    if s.is_empty() {
        return None;
    }
    // Validate: a Xilinx VBNV looks like "xilinx_u250_gen3x16_xdma_4_1_202210_1"
    // — only alphanumeric, underscores, hyphens, and dots.
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return None;
    }
    Some(s)
}

/// Bit width of a primitive C scalar type name from Vitis arg metadata
/// (`type="uint16_t"`); returns None for pointers, composites, and other
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

pub fn extract_embedded_xml(xclbin: &[u8]) -> Result<String> {
    const MAGIC: &[u8; 8] = b"xclbin2\0";

    if xclbin.len() < 8 || &xclbin[..8] != MAGIC {
        return Err(FrtError::MetadataParse("not an xclbin2 file".into()));
    }

    // The EMBEDDED_METADATA section is an XML document embedded in the xclbin.
    // Rather than depending on the exact struct layout (which varies across
    // xclbin versions), scan for the XML header and extract the document.
    let xml_header = b"<?xml";
    for start in xclbin
        .windows(xml_header.len())
        .enumerate()
        .filter_map(|(i, w)| (w == xml_header).then_some(i))
    {
        // Find the end of this XML document (null terminator or end of valid UTF-8)
        let remaining = &xclbin[start..];
        let end = remaining
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(remaining.len());
        let candidate = &remaining[..end];
        // The EMBEDDED_METADATA XML contains a <project> or <root> element with
        // kernel metadata. Ignore other XML fragments (e.g., IP catalog data).
        if let Ok(text) = std::str::from_utf8(candidate) {
            if text.contains("<kernel") {
                return Ok(text.to_owned());
            }
        }
    }

    Err(FrtError::MetadataParse(
        "EMBEDDED_METADATA section not found in xclbin".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KERNEL_XML: &str = r#"<?xml version="1.0"?>
<root><kernel name="vadd"><args>
  <arg name="a" addressQualifier="1" id="0" dataWidth="512" addrWidth="64"/>
  <arg name="n" addressQualifier="0" id="1" dataWidth="32"/>
</args></kernel></root>"#;

    const TARGETED_XML: &str = r#"<?xml version="1.0"?>
<project>
  <platform name="xilinx_u250_gen3x16_xdma_3_1_202020_1">
    <device>
      <core target="hw_em">
        <kernel name="vadd"><args>
          <arg name="a" addressQualifier="1" id="0" dataWidth="512" />
        </args></kernel>
      </core>
    </device>
  </platform>
</project>"#;

    #[test]
    fn parse_kernel_xml_extracts_args() {
        let meta = parse_embedded_xml(KERNEL_XML).expect("parse");
        assert_eq!(meta.top_name, "vadd");
        assert_eq!(meta.args.len(), 2);
    }

    #[test]
    fn scalar_width_falls_back_to_vitis_size_bytes() {
        // Real Vitis xclbin XML carries the C byte size, not a bit width.
        const VITIS_XML: &str = r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="wide" addressQualifier="0" id="0" size="0x8" type="uint64_t"/>
  <arg name="narrow" addressQualifier="0" id="1" size="0x2" type="uint16_t"/>
  <arg name="plain" addressQualifier="0" id="2" size="0x4" type="int"/>
  <arg name="ptr" addressQualifier="1" id="3" size="0x8" type="int*"/>
  <arg name="regpad" addressQualifier="0" id="4" size="0x4" type="uint16_t"/>
  <arg name="nosize" addressQualifier="0" id="5" type="uint64_t"/>
  <arg name="custom" addressQualifier="0" id="6" size="0x8" type="my_struct_t"/>
</args></kernel></root>"#;
        let meta = parse_embedded_xml(VITIS_XML).expect("parse");
        let width = |id| {
            meta.args.iter().find(|a| a.id == id).map(|a| match a.kind {
                XrtArgKind::Scalar { width } | XrtArgKind::Stream { width } => width,
                XrtArgKind::Mmap { data_width } => data_width,
            })
        };
        assert_eq!(width(0), Some(64));
        assert_eq!(width(1), Some(16));
        assert_eq!(width(2), Some(32));
        // mmap `size` is the 8-byte pointer size, not the bus width.
        assert_eq!(width(3), Some(32));
        // A register-padded scalar still binds its logical C width.
        assert_eq!(width(4), Some(16));
        // `type` alone (no `size`) is enough.
        assert_eq!(width(5), Some(64));
        // Non-primitive types fall back to `size`.
        assert_eq!(width(6), Some(64));
    }

    #[test]
    fn explicit_bit_width_wins_over_size() {
        const BOTH_XML: &str = r#"<?xml version="1.0"?>
<root><kernel name="k"><args>
  <arg name="s" addressQualifier="0" id="0" dataWidth="512" size="0x8"/>
</args></kernel></root>"#;
        let meta = parse_embedded_xml(BOTH_XML).expect("parse");
        assert!(matches!(
            meta.args[0].kind,
            XrtArgKind::Scalar { width: 512 }
        ));
    }

    #[test]
    fn parse_embedded_xml_extracts_platform_and_mode() {
        let meta = parse_embedded_xml(TARGETED_XML).expect("parse");
        assert_eq!(meta.top_name, "vadd");
        assert_eq!(meta.platform, "xilinx_u250_gen3x16_xdma_3_1_202020_1");
        assert_eq!(meta.mode, XclbinMode::HwEmu);
    }

    #[test]
    fn extract_platform_vbnv_from_header() {
        // Build a minimal xclbin-like buffer with the VBNV at offset 352.
        let mut buf = vec![0u8; 416]; // 352 + 64
        buf[..8].copy_from_slice(b"xclbin2\0");
        let vbnv = b"xilinx_u250_gen3x16_xdma_4_1_202210_1";
        buf[352..352 + vbnv.len()].copy_from_slice(vbnv);
        let result = extract_platform_vbnv(&buf);
        assert_eq!(
            result.as_deref(),
            Some("xilinx_u250_gen3x16_xdma_4_1_202210_1")
        );
    }

    #[test]
    fn extract_platform_vbnv_empty_returns_none() {
        let mut buf = vec![0u8; 416];
        buf[..8].copy_from_slice(b"xclbin2\0");
        assert_eq!(extract_platform_vbnv(&buf), None);
    }

    #[test]
    fn extract_platform_vbnv_short_buffer_returns_none() {
        let buf = vec![0u8; 100]; // Too short
        assert_eq!(extract_platform_vbnv(&buf), None);
    }
}
