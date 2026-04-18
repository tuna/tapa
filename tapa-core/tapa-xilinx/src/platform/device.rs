//! `.xpfm` / `.hpfm` device config parsing.
//!
//! Ports `tapa/backend/device_config.py::get_device_info`: the `.xpfm`
//! directory contains a ZIP (`.xsa`/`.dsa`) holding a `<name>.hpfm` XML
//! document; we extract `part_num` and `clock_period` from the
//! `xd:platformInfo` node, following the `xd:` namespace used by the
//! Xilinx tooling.

use std::io::Read;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub part_num: String,
    pub clock_period: String,
}

/// Parse the `.hpfm` XML document body (namespace-aware).
///
/// Accepts any namespace prefix bound to
/// `http://www.xilinx.com/xd` (Python keys off an `xd:` prefix but the
/// underlying `ElementTree.find` call is namespace-URI driven).
pub fn parse_hpfm_xml(xml: &[u8]) -> Result<DeviceInfo> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut part_num: Option<String> = None;
    let mut clock_period: Option<String> = None;
    let mut in_platform_info = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => return Err(XilinxError::Xml(e)),
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => {
                let qname_owned = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let name = localname(&qname_owned);
                if name == "platformInfo" {
                    in_platform_info = true;
                }
                if in_platform_info {
                    match name {
                        "deviceInfo" => {
                            part_num = attr_value(e, "name")?;
                        }
                        "clock" => {
                            if attr_value(e, "id")?.as_deref() == Some("0") {
                                clock_period = attr_value(e, "period")?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let qname_owned = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let name = localname(&qname_owned);
                if in_platform_info || name == "deviceInfo" || name == "clock" {
                    match name {
                        "deviceInfo" => {
                            part_num = attr_value(e, "name")?;
                        }
                        "clock" => {
                            if attr_value(e, "id")?.as_deref() == Some("0") {
                                clock_period = attr_value(e, "period")?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let qname_owned = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let name = localname(&qname_owned);
                if name == "platformInfo" {
                    in_platform_info = false;
                }
            }
            Ok(_) => {}
        }
        buf.clear();
    }

    match (part_num, clock_period) {
        (Some(part_num), Some(clock_period)) => Ok(DeviceInfo {
            part_num,
            clock_period,
        }),
        (None, _) => Err(XilinxError::DeviceConfig {
            path: PathBuf::new(),
            detail: "cannot find part number in platform".into(),
        }),
        (_, None) => Err(XilinxError::DeviceConfig {
            path: PathBuf::new(),
            detail: "cannot find clock period in platform".into(),
        }),
    }
}

fn localname(qname: &str) -> &str {
    qname.rsplit_once(':').map_or(qname, |(_, n)| n)
}

fn attr_value(
    e: &quick_xml::events::BytesStart<'_>,
    name: &str,
) -> Result<Option<String>> {
    for attr in e.attributes() {
        let attr = attr.map_err(|err| XilinxError::HlsReportParse(err.to_string()))?;
        let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
        if localname(key) == name {
            let val = attr
                .unescape_value()
                .map_err(XilinxError::Xml)?
                .to_string();
            return Ok(Some(val));
        }
    }
    Ok(None)
}

/// Parse an `.xpfm`-adjacent ZIP (`.xsa` / `.dsa`) that holds one
/// `.hpfm` XML entry.
pub fn parse_xpfm(bytes: &[u8]) -> Result<DeviceInfo> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| XilinxError::DeviceConfig {
            path: PathBuf::new(),
            detail: format!("open archive: {e}"),
        })?;

    let hpfm_idx = (0..archive.len()).find(|&i| {
        archive
            .by_index(i)
            .ok()
            .is_some_and(|e| e.name().ends_with(".hpfm"))
    });
    let Some(idx) = hpfm_idx else {
        return Err(XilinxError::DeviceConfig {
            path: PathBuf::new(),
            detail: "archive missing .hpfm entry".into(),
        });
    };
    let mut entry = archive
        .by_index(idx)
        .map_err(|e| XilinxError::DeviceConfig {
            path: PathBuf::new(),
            detail: format!("open .hpfm entry: {e}"),
        })?;
    let mut xml = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut xml)?;
    parse_hpfm_xml(&xml)
}

/// Resolve the `.xsa`/`.dsa` file under `<platform_path>/hw/`, then
/// parse it. Matches `tapa/backend/device_config.py::get_device_info`.
pub fn parse_device_info(
    platform_path: &Path,
    part_num_override: Option<&str>,
    clock_period_override: Option<&str>,
) -> Result<DeviceInfo> {
    if !platform_path.is_dir() {
        return Err(XilinxError::PlatformNotFound(platform_path.to_path_buf()));
    }
    let hw = platform_path.join("hw");
    let entries = std::fs::read_dir(&hw).map_err(|_| XilinxError::PlatformNotFound(hw.clone()))?;
    let archive_path = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x == "xsa" || x == "dsa")
        })
        .ok_or_else(|| XilinxError::PlatformNotFound(hw.clone()))?;

    let bytes = std::fs::read(&archive_path)?;
    let mut info = parse_xpfm(&bytes).map_err(|e| match e {
        XilinxError::DeviceConfig { detail, .. } => XilinxError::DeviceConfig {
            path: archive_path.clone(),
            detail,
        },
        other => other,
    })?;
    if let Some(p) = part_num_override {
        info.part_num = p.to_string();
    }
    if let Some(c) = clock_period_override {
        info.clock_period = c.to_string();
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const HPFM_NO_NS: &str = r#"<?xml version="1.0"?>
<component>
  <platformInfo>
    <deviceInfo name="xcu250-figd2104-2L-e"/>
    <systemClocks>
      <clock id="0" period="3.333"/>
    </systemClocks>
  </platformInfo>
</component>"#;

    const HPFM_XD: &str = r#"<?xml version="1.0"?>
<xd:component xmlns:xd="http://www.xilinx.com/xd">
  <xd:platformInfo>
    <xd:deviceInfo xd:name="xcu250-figd2104-2L-e"/>
    <xd:systemClocks>
      <xd:clock xd:id="0" xd:period="3.333"/>
    </xd:systemClocks>
  </xd:platformInfo>
</xd:component>"#;

    #[test]
    fn parses_hpfm_without_namespace() {
        let info = parse_hpfm_xml(HPFM_NO_NS.as_bytes()).unwrap();
        assert_eq!(info.part_num, "xcu250-figd2104-2L-e");
        assert_eq!(info.clock_period, "3.333");
    }

    #[test]
    fn parses_hpfm_with_xd_prefix() {
        let info = parse_hpfm_xml(HPFM_XD.as_bytes()).unwrap();
        assert_eq!(info.part_num, "xcu250-figd2104-2L-e");
        assert_eq!(info.clock_period, "3.333");
    }

    #[test]
    fn missing_part_number_is_typed_error() {
        let xml = r#"<platformInfo><systemClocks><clock id="0" period="3"/></systemClocks></platformInfo>"#;
        let err = parse_hpfm_xml(xml.as_bytes()).unwrap_err();
        assert!(matches!(err, XilinxError::DeviceConfig { .. }));
    }

    #[test]
    fn missing_clock_period_is_typed_error() {
        let xml = r#"<platformInfo><deviceInfo name="x"/></platformInfo>"#;
        let err = parse_hpfm_xml(xml.as_bytes()).unwrap_err();
        assert!(matches!(err, XilinxError::DeviceConfig { .. }));
    }

    fn build_xpfm_zip(hpfm: &str) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut zw = zip::ZipWriter::new(&mut out);
            zw.start_file(
                "shell.hpfm",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zw.write_all(hpfm.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn parse_xpfm_round_trips_fixture() {
        let zip = build_xpfm_zip(HPFM_XD);
        let info = parse_xpfm(&zip).unwrap();
        assert_eq!(info.part_num, "xcu250-figd2104-2L-e");
        assert_eq!(info.clock_period, "3.333");
    }

    #[test]
    fn parse_xpfm_missing_hpfm_is_typed_error() {
        let mut out = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut zw = zip::ZipWriter::new(&mut out);
            zw.start_file("other.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(b"no hpfm").unwrap();
            zw.finish().unwrap();
        }
        let err = parse_xpfm(&out.into_inner()).unwrap_err();
        assert!(matches!(err, XilinxError::DeviceConfig { .. }));
    }

    #[test]
    fn parse_device_info_nonexistent_path_is_typed_error() {
        let err =
            parse_device_info(Path::new("/definitely/not/a/platform"), None, None).unwrap_err();
        assert!(matches!(err, XilinxError::PlatformNotFound(_)));
    }
}
