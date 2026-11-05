//! xclbin-binary concerns.
//!
//! Finding the metadata XML inside the container, reading the platform
//! VBNV out of its header, and the AI Engine graph its `AIE_METADATA`
//! section describes. The XML itself is parsed by
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
    const EMBEDDED_METADATA: u32 = 2;

    if xclbin.len() < 8 || &xclbin[..8] != MAGIC {
        return Err(FrtError::MetadataParse("not an xclbin2 file".into()));
    }
    let section = xclbin_section(xclbin, EMBEDDED_METADATA).ok_or_else(|| {
        FrtError::MetadataParse("EMBEDDED_METADATA section not found in xclbin".into())
    })?;
    let xml = section.split(|byte| *byte == 0).next().unwrap_or_default();
    let text = std::str::from_utf8(xml).map_err(|error| {
        FrtError::MetadataParse(format!("EMBEDDED_METADATA is not UTF-8: {error}"))
    })?;
    if !text.contains("<kernel") {
        return Err(FrtError::MetadataParse(
            "EMBEDDED_METADATA contains no kernel".into(),
        ));
    }
    Ok(text.to_owned())
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

/// The payload of the first section of `kind` in an xclbin's table of
/// contents: `m_numSections` sits at byte 448 of the `axlf` header and the
/// 40-byte `axlf_section_header`s follow it at byte 456.
fn xclbin_section(xclbin: &[u8], kind: u32) -> Option<&[u8]> {
    const COUNT_OFFSET: usize = 448;
    const SECTIONS_OFFSET: usize = 456;
    const SECTION_HEADER: usize = 40;
    let read_u32 = |at: usize| Some(u32::from_le_bytes(xclbin.get(at..at + 4)?.try_into().ok()?));
    let read_u64 = |at: usize| Some(u64::from_le_bytes(xclbin.get(at..at + 8)?.try_into().ok()?));
    let count = usize::try_from(read_u32(COUNT_OFFSET)?).ok()?;
    (0..count).find_map(|i| {
        let header = SECTIONS_OFFSET + i * SECTION_HEADER;
        if read_u32(header)? != kind {
            return None;
        }
        let offset = usize::try_from(read_u64(header + 24)?).ok()?;
        let size = usize::try_from(read_u64(header + 32)?).ok()?;
        xclbin.get(offset..offset.checked_add(size)?)
    })
}

/// `AIE_METADATA`: the AI Engine control configuration `aiecompiler` wrote.
const AIE_METADATA: u32 = 25;

/// The AI Engine graph `tapa synth` generated, as the xclbin records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieGraph {
    /// `tapa_aie`, or `tapa_aie_forever` when some kernel never returns.
    pub name: String,
    pub run_forever: bool,
    pub iteration_addresses: Vec<u32>,
    pub rtps: Vec<AieRtp>,
}

/// A runtime parameter carrying one scalar argument of the PL kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieRtp {
    /// The hierarchical name XRT updates it by, `<graph>.rtp_<arg>_...`.
    pub alias: String,
    /// The PL kernel argument whose value it carries.
    pub arg: u32,
    pub bytes: usize,
    pub ping_address: u32,
    pub pong_address: u32,
}

/// A string-or-number JSON value as u64 (xclbinutil quotes; aiecompiler does not).
fn aie_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_str()
        .and_then(|text| text.parse::<u64>().ok())
        .or_else(|| value.as_u64())
}

fn aie_u32(value: &serde_json::Value, field: &str) -> Result<u32> {
    aie_u64(value)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| FrtError::MetadataParse(format!("invalid AIE {field}")))
}

/// The TAPA graph in the xclbin's `AIE_METADATA`, if it has one.
pub fn parse_aie_graph(xclbin: &[u8]) -> Result<Option<AieGraph>> {
    let Some(section) = xclbin_section(xclbin, AIE_METADATA) else {
        return Ok(None);
    };
    let text = section.split(|b| *b == 0).next().unwrap_or_default();
    let json: serde_json::Value = serde_json::from_slice(text)
        .map_err(|e| FrtError::MetadataParse(format!("AIE_METADATA is not JSON: {e}")))?;
    let meta = &json["aie_metadata"];
    let Some(graph) = meta["graphs"]
        .as_object()
        .into_iter()
        .flat_map(serde_json::Map::values)
        .find(|graph| {
            matches!(
                graph["name"].as_str(),
                Some("tapa_aie" | "tapa_aie_forever")
            )
        })
    else {
        return Ok(None);
    };
    let name = graph["name"].as_str().expect("matched graph name");
    let iteration_addresses = graph["iteration_memory_addresses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| aie_u32(value, "iteration-memory address"))
        .collect::<Result<Vec<_>>>()?;
    let prefix = format!("{name}.rtp_");
    let mut rtps = Vec::new();
    for rtp in meta["RTPs"]
        .as_object()
        .into_iter()
        .flat_map(serde_json::Map::values)
    {
        let Some(alias) = rtp["alias_name"].as_str() else {
            continue;
        };
        let Some(rest) = alias.strip_prefix(&prefix) else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let arg = digits.parse::<u32>().ok();
        // xclbinutil stores every value as a string; aiecompiler's own file
        // has numbers.
        let size = &rtp["number_of_bytes"];
        let bytes = aie_u64(size).and_then(|n| usize::try_from(n).ok());
        let (Some(arg), Some(bytes)) = (arg, bytes) else {
            return Err(FrtError::MetadataParse(format!(
                "AI Engine runtime parameter `{alias}` names no kernel argument or size"
            )));
        };
        rtps.push(AieRtp {
            alias: alias.to_owned(),
            arg,
            bytes,
            ping_address: aie_u32(&rtp["ping_buffer_address"], "RTP ping address")?,
            pong_address: aie_u32(&rtp["pong_buffer_address"], "RTP pong address")?,
        });
    }
    Ok(Some(AieGraph {
        name: name.to_owned(),
        run_forever: name == "tapa_aie_forever",
        iteration_addresses,
        rtps,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An xclbin whose table of contents holds one section.
    fn xclbin_with_section(kind: u32, payload: &[u8]) -> Vec<u8> {
        let mut buf = b"xclbin2\0".to_vec();
        buf.resize(496, 0);
        buf[448..452].copy_from_slice(&1u32.to_le_bytes());
        buf[456..460].copy_from_slice(&kind.to_le_bytes());
        buf[480..488].copy_from_slice(&496u64.to_le_bytes());
        buf[488..496].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    /// The runtime finds each runtime parameter's value by the argument
    /// index `tapa synth` puts in its alias, and how to run the graph by
    /// the graph's name (`tapa_xilinx::tools::aie::graph::rtp_name`). The
    /// values are strings, as xclbinutil writes them.
    #[test]
    fn the_aie_graph_and_its_runtime_parameters_come_from_aie_metadata() {
        let json = br#"{"aie_metadata": {
            "graphs": {"graph0": {"id": "0", "name": "tapa_aie_forever",
                "iteration_memory_addresses": ["8196"]}},
            "RTPs": {
                "rtp0": {"alias_name": "tapa_aie_forever.rtp_3_Scale_0_factor",
                         "port_name": "tapa_aie_forever.k_Scale_0.in[1]",
                         "number_of_bytes": "4",
                         "ping_buffer_address": 8160, "pong_buffer_address": 8160},
                "rtp1": {"alias_name": "other.rtp_0_x", "number_of_bytes": "8"}
            }}}"#;
        let mut payload = json.to_vec();
        payload.push(0);
        let graph = parse_aie_graph(&xclbin_with_section(AIE_METADATA, &payload))
            .expect("parse")
            .expect("a graph");
        assert_eq!(
            graph,
            AieGraph {
                name: "tapa_aie_forever".into(),
                run_forever: true,
                iteration_addresses: vec![0x2004],
                rtps: vec![AieRtp {
                    alias: "tapa_aie_forever.rtp_3_Scale_0_factor".into(),
                    arg: 3,
                    bytes: 4,
                    ping_address: 0x1fe0,
                    pong_address: 0x1fe0,
                }],
            }
        );
        let pl_only = xclbin_with_section(2, b"<?xml?>");
        assert_eq!(parse_aie_graph(&pl_only).expect("parse"), None);
    }

    #[test]
    fn embedded_metadata_stops_at_its_section_boundary() {
        const METADATA: &str = r#"<?xml version="1.0"?><project><kernel name="vadd"/></project>"#;
        let mut xclbin = xclbin_with_section(2, METADATA.as_bytes());
        xclbin.extend_from_slice(br#"{"trailing": "<kernel is not metadata>"}"#);
        assert_eq!(extract_embedded_xml(&xclbin).expect("extract"), METADATA);
    }

    #[test]
    fn a_container_without_metadata_is_an_error() {
        let xclbin = xclbin_with_section(25, br#"<?xml version="1.0"?><ip_catalog/>"#);
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
