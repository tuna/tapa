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

pub fn extract_embedded_xml(xclbin: &[u8]) -> Result<String> {
    const MAGIC: &[u8; 8] = b"xclbin2\0";
    const EMBEDDED_METADATA_KIND: u32 = 25;
    const OFF_NUM_SECTIONS: usize = 328 + 8 + 8 + 8 + 2 + 1 + 1 + 4 + 4 + 16 + 64 + 16 + 16;
    const SECTION_HDR_SIZE: usize = 4 + 16 + 8 + 8;

    if xclbin.len() < OFF_NUM_SECTIONS + 4 || &xclbin[..8] != MAGIC {
        return Err(FrtError::MetadataParse("not an xclbin2 file".into()));
    }

    let num_sections = u32::from_le_bytes(
        xclbin[OFF_NUM_SECTIONS..OFF_NUM_SECTIONS + 4]
            .try_into()
            .map_err(|_| FrtError::MetadataParse("header decode failed".into()))?,
    ) as usize;
    let sections_start = OFF_NUM_SECTIONS + 4;

    for i in 0..num_sections {
        let base = sections_start + i * SECTION_HDR_SIZE;
        if base + SECTION_HDR_SIZE > xclbin.len() {
            break;
        }
        let kind = u32::from_le_bytes(
            xclbin[base..base + 4]
                .try_into()
                .map_err(|_| FrtError::MetadataParse("kind decode failed".into()))?,
        );
        let offset = u64::from_le_bytes(
            xclbin[base + 20..base + 28]
                .try_into()
                .map_err(|_| FrtError::MetadataParse("offset decode failed".into()))?,
        ) as usize;
        let size = u64::from_le_bytes(
            xclbin[base + 28..base + 36]
                .try_into()
                .map_err(|_| FrtError::MetadataParse("size decode failed".into()))?,
        ) as usize;
        if kind == EMBEDDED_METADATA_KIND {
            let bytes = xclbin
                .get(offset..offset + size)
                .ok_or_else(|| FrtError::MetadataParse("EMBEDDED_METADATA out of bounds".into()))?;
            return String::from_utf8(bytes.to_vec())
                .map_err(|e| FrtError::MetadataParse(e.to_string()));
        }
    }

    Err(FrtError::MetadataParse(
        "EMBEDDED_METADATA section not found".into(),
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
}
