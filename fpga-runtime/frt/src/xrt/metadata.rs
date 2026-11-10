//! xclbin-binary concerns.
//!
//! Finding the metadata XML inside the container, reading the platform
//! VBNV out of its header, and the AI Engine graph its `AIE_METADATA`
//! section describes. The XML itself is parsed by
//! [`frt_cosim::metadata::kernel_xml`], shared with the cosim runtime.

use crate::error::{FrtError, Result};
use std::collections::HashMap;

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

/// What an xclbin runs on, from `m_mode` in its `axlf` header (byte 332):
/// `XCLBIN_HW_EMU` (4) and `XCLBIN_HW_EMU_PR` (6) are hardware emulation,
/// `XCLBIN_SW_EMU` (5) software emulation, anything else hardware.
pub fn xclbin_target(xclbin: &[u8]) -> frt_cosim::metadata::kernel_xml::XclbinTarget {
    use frt_cosim::metadata::kernel_xml::XclbinTarget;
    const MODE_OFFSET: usize = 332;
    match xclbin.get(MODE_OFFSET..MODE_OFFSET + 2) {
        Some([4 | 6, 0]) => XclbinTarget::HwEmu,
        Some([5, 0]) => XclbinTarget::SwEmu,
        _ => XclbinTarget::Flat,
    }
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

/// `USER_METADATA`, where `tapa link` records the arguments of a top on
/// the AI Engine.
const USER_METADATA: u32 = 16;

/// The arguments of a top on the AI Engine, which has no PL kernel and so
/// no kernel in the xclbin's embedded metadata (`tapa-cli`'s `Arguments`).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AieTopArguments {
    pub top: String,
    pub gmios: Vec<GmioArgument>,
}

/// A stream argument of a top on the AI Engine: a GMIO moving one frame
/// per transfer.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GmioArgument {
    pub index: u32,
    /// The GMIO's logical name.
    pub name: String,
    /// The host writes it and the AI Engine reads it.
    pub input: bool,
    /// 32-bit words per stream element.
    pub element_words: u32,
    /// 32-bit words per frame, one transfer.
    pub transfer_words: u32,
}

/// The arguments of a top on the AI Engine, when the xclbin is one.
pub fn parse_aie_top_arguments(xclbin: &[u8]) -> Result<Option<AieTopArguments>> {
    let Some(section) = xclbin_section(xclbin, USER_METADATA) else {
        return Ok(None);
    };
    let text = section.split(|b| *b == 0).next().unwrap_or_default();
    serde_json::from_slice(text)
        .map(Some)
        .map_err(|e| FrtError::MetadataParse(format!("USER_METADATA is not TAPA's: {e}")))
}

/// `AIE_METADATA`: the AI Engine control configuration `aiecompiler` wrote.
const AIE_METADATA: u32 = 25;

/// An AI Engine tile, as the AIE metadata numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AieTile {
    pub column: u32,
    pub row: u32,
}

/// The AI Engine graph `tapa synth` generated, as the xclbin records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieGraph {
    /// `tapa_aie`, or `tapa_aie_forever` when some kernel never returns.
    pub name: String,
    pub run_forever: bool,
    /// The tiles of the graph's kernels.
    pub cores: Vec<AieTile>,
    /// Each kernel's iteration count: its tile and its address there.
    pub iterations: Vec<(AieTile, u32)>,
    pub rtps: Vec<AieRtp>,
    /// The kernels' drain flags (`<graph>.drain_<kernel>`), which a graph
    /// writing a GMIO has (see `tapa-xilinx`'s graph).
    pub drains: Vec<String>,
}

/// A runtime parameter carrying one scalar argument of the PL kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AieRtp {
    /// The hierarchical name XRT updates it by, `<graph>.rtp_<arg>_...`.
    pub alias: String,
    /// The PL kernel argument whose value it carries.
    pub arg: u32,
    pub bytes: usize,
    /// The tile holding its buffer.
    pub tile: AieTile,
    pub ping_address: u32,
    pub pong_address: u32,
}

impl AieRtp {
    /// The bytes this parameter programs: the scalar bound to its kernel
    /// argument, zero-padded to `bytes`. A missing binding means the
    /// parameter names an argument that is not a scalar.
    pub fn scalar_value(&self, values: &HashMap<u32, Vec<u8>>) -> Result<Vec<u8>> {
        let mut value = values.get(&self.arg).cloned().ok_or_else(|| {
            FrtError::Xrt(format!(
                "runtime parameter `{}` carries argument {}, which is not a scalar",
                self.alias, self.arg
            ))
        })?;
        value.resize(self.bytes, 0);
        Ok(value)
    }
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
    let numbers = |field: &str| {
        graph[field]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| aie_u32(value, field))
            .collect::<Result<Vec<_>>>()
    };
    let tiles = |columns: &str, rows: &str| -> Result<Vec<AieTile>> {
        let (columns, rows) = (numbers(columns)?, numbers(rows)?);
        if columns.len() != rows.len() {
            return Err(FrtError::MetadataParse(format!(
                "AIE graph `{name}` has {} columns but {} rows",
                columns.len(),
                rows.len()
            )));
        }
        Ok(columns
            .into_iter()
            .zip(rows)
            .map(|(column, row)| AieTile { column, row })
            .collect())
    };
    let cores = tiles("core_columns", "core_rows")?;
    let iteration_tiles = tiles("iteration_memory_columns", "iteration_memory_rows")?;
    let iteration_addresses = numbers("iteration_memory_addresses")?;
    if iteration_tiles.len() != iteration_addresses.len() {
        return Err(FrtError::MetadataParse(format!(
            "AIE graph `{name}` has {} iteration-memory tiles but {} addresses",
            iteration_tiles.len(),
            iteration_addresses.len()
        )));
    }
    let iterations = iteration_tiles
        .into_iter()
        .zip(iteration_addresses)
        .collect();
    let prefix = format!("{name}.rtp_");
    let drain_prefix = format!("{name}.drain_");
    let (mut rtps, mut drains) = (Vec::new(), Vec::new());
    for rtp in meta["RTPs"]
        .as_object()
        .into_iter()
        .flat_map(serde_json::Map::values)
    {
        let Some(alias) = rtp["alias_name"].as_str() else {
            continue;
        };
        if alias.starts_with(&drain_prefix) {
            drains.push(alias.to_owned());
            continue;
        }
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
            tile: AieTile {
                column: aie_u32(&rtp["ping_buffer_column"], "RTP column")?,
                row: aie_u32(&rtp["ping_buffer_row"], "RTP row")?,
            },
            ping_address: aie_u32(&rtp["ping_buffer_address"], "RTP ping address")?,
            pong_address: aie_u32(&rtp["pong_buffer_address"], "RTP pong address")?,
        });
    }
    Ok(Some(AieGraph {
        name: name.to_owned(),
        run_forever: name == "tapa_aie_forever",
        cores,
        iterations,
        rtps,
        drains,
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
                "core_columns": ["24", "25"], "core_rows": ["0", "0"],
                "iteration_memory_columns": ["24", "25"],
                "iteration_memory_rows": ["0", "0"],
                "iteration_memory_addresses": ["8196", "16388"]}},
            "RTPs": {
                "rtp0": {"alias_name": "tapa_aie_forever.rtp_3_Scale_0_factor",
                         "port_name": "tapa_aie_forever.k_Scale_0.in[1]",
                         "number_of_bytes": "4",
                         "ping_buffer_column": "24", "ping_buffer_row": "0",
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
                cores: vec![
                    AieTile { column: 24, row: 0 },
                    AieTile { column: 25, row: 0 }
                ],
                iterations: vec![
                    (AieTile { column: 24, row: 0 }, 0x2004),
                    (AieTile { column: 25, row: 0 }, 0x4004)
                ],
                rtps: vec![AieRtp {
                    alias: "tapa_aie_forever.rtp_3_Scale_0_factor".into(),
                    arg: 3,
                    bytes: 4,
                    tile: AieTile { column: 24, row: 0 },
                    ping_address: 0x1fe0,
                    pong_address: 0x1fe0,
                }],
                drains: Vec::new(),
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
