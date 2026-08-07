//! xclbin-binary concerns.
//!
//! Finding the metadata XML inside the container, and reading the platform
//! VBNV out of its header. The XML itself is parsed by
//! [`frt_cosim::metadata::kernel_xml`], shared with the cosim runtime.

use crate::error::{FrtError, Result};
use frt_cosim::metadata::kernel_xml::{self, KernelXml};

/// Parse the metadata XML embedded in an xclbin.
///
/// A thin adapter over the shared reader: only the error type differs.
pub fn parse_embedded_xml(xml: &str) -> Result<KernelXml> {
    kernel_xml::parse(xml).map_err(|e| FrtError::MetadataParse(e.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An xclbin2 container holding `sections` as NUL-separated blobs.
    fn xclbin_with(sections: &[&str]) -> Vec<u8> {
        let mut buf = b"xclbin2\0".to_vec();
        buf.resize(416, 0);
        for section in sections {
            buf.extend_from_slice(section.as_bytes());
            buf.push(0);
        }
        buf
    }

    #[test]
    fn embedded_metadata_is_picked_out_of_the_container() {
        const METADATA: &str = r#"<?xml version="1.0"?><project><kernel name="vadd"/></project>"#;
        let xclbin = xclbin_with(&[METADATA]);
        assert_eq!(extract_embedded_xml(&xclbin).expect("extract"), METADATA);
    }

    #[test]
    fn other_embedded_xml_sections_are_skipped() {
        // An xclbin also carries IP-catalog and system-diagram XML; only the
        // section describing a kernel is the metadata we want.
        const IP_LAYOUT: &str = r#"<?xml version="1.0"?><ip_catalog><ip name="axi"/></ip_catalog>"#;
        const METADATA: &str = r#"<?xml version="1.0"?><project><kernel name="vadd"/></project>"#;
        let xclbin = xclbin_with(&[IP_LAYOUT, METADATA]);
        assert_eq!(extract_embedded_xml(&xclbin).expect("extract"), METADATA);
    }

    #[test]
    fn a_container_without_metadata_is_an_error() {
        let xclbin = xclbin_with(&[r#"<?xml version="1.0"?><ip_catalog/>"#]);
        let err = extract_embedded_xml(&xclbin).expect_err("no kernel section");
        assert!(err.to_string().contains("EMBEDDED_METADATA"), "{err}");
    }

    #[test]
    fn a_file_that_is_not_an_xclbin_is_an_error() {
        let err = extract_embedded_xml(b"not an xclbin at all").expect_err("bad magic");
        assert!(err.to_string().contains("xclbin2"), "{err}");
    }

    #[test]
    fn extract_platform_vbnv_from_header() {
        // Build a minimal xclbin-like buffer with the VBNV at offset 352.
        let mut buf = vec![0u8; 416];
        let vbnv = b"xilinx_u250_gen3x16_xdma_4_1_202210_1";
        buf[352..352 + vbnv.len()].copy_from_slice(vbnv);
        assert_eq!(
            extract_platform_vbnv(&buf).as_deref(),
            Some("xilinx_u250_gen3x16_xdma_4_1_202210_1")
        );
    }

    #[test]
    fn extract_platform_vbnv_empty_returns_none() {
        let buf = vec![0u8; 416];
        assert_eq!(extract_platform_vbnv(&buf), None);
    }

    #[test]
    fn extract_platform_vbnv_short_buffer_returns_none() {
        let buf = vec![0u8; 100]; // Too short
        assert_eq!(extract_platform_vbnv(&buf), None);
    }
}
