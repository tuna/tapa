use crate::error::{FrtError, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq)]
pub enum XrtArgKind {
    Scalar { width: u32 },
    Mmap { data_width: u32 },
    Stream { width: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct XrtArg {
    pub name: String,
    pub id: u32,
    pub kind: XrtArgKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct XrtMetadata {
    pub top_name: String,
    pub args: Vec<XrtArg>,
    pub platform: String,
    pub mode: XclbinMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
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
                    let mut data_width = 32u32;
                    for a in e.attributes().flatten() {
                        let v = String::from_utf8_lossy(&a.value).into_owned();
                        match a.key.as_ref() {
                            b"name" => name = v,
                            b"id" => id = v.parse().unwrap_or(0),
                            b"addressQualifier" => qualifier = v.parse().unwrap_or(0),
                            b"dataWidth" | b"width" => data_width = v.parse().unwrap_or(32),
                            _ => {}
                        }
                    }
                    let kind = match qualifier {
                        0 => XrtArgKind::Scalar { width: data_width },
                        1 => XrtArgKind::Mmap { data_width },
                        4 => XrtArgKind::Stream { width: data_width },
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
        .filter_map(|(i, w)| if w == xml_header { Some(i) } else { None })
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
