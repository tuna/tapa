//! `.xpfm` / `.hpfm` device config parsing.
//!
//! Implements: the `.xpfm`
//! directory contains a ZIP (`.xsa`/`.dsa`) holding a `<name>.hpfm` XML
//! document; we extract `part_num` and `clock_period` from the
//! `xd:platformInfo` node, following the `xd:` namespace used by the
//! Xilinx tooling.

use std::io::Read;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub part_num: String,
    pub clock_period: String,
}

#[derive(Debug, Deserialize)]
struct HpfmXml {
    #[serde(alias = "platformInfo", alias = "xd:platformInfo")]
    platform_info: PlatformInfo,
}

#[derive(Debug, Deserialize)]
struct PlatformInfo {
    #[serde(alias = "deviceInfo", alias = "xd:deviceInfo")]
    device_info: DeviceInfoNode,
    #[serde(alias = "systemClocks", alias = "xd:systemClocks")]
    system_clocks: SystemClocks,
}

#[derive(Debug, Deserialize)]
struct DeviceInfoNode {
    #[serde(rename = "@name")]
    name: String,
}

#[derive(Debug, Deserialize)]
struct SystemClocks {
    #[serde(alias = "clock", alias = "xd:clock", default)]
    clock: Vec<Clock>,
}

#[derive(Debug, Deserialize)]
struct Clock {
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@period")]
    period: String,
}

/// Parse the `.hpfm` XML document body (namespace-aware).
///
/// Accepts any namespace prefix bound to
/// `http://www.xilinx.com/xd` (keys off an `xd:` prefix but the
/// underlying `ElementTree.find` call is namespace-URI driven).
pub fn parse_hpfm_xml(xml: &[u8]) -> Result<DeviceInfo> {
    let parsed: HpfmXml = quick_xml::de::from_reader(xml).map_err(|e| {
        XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: format!("cannot parse hpfm xml: {e}"),
        }
    })?;

    let clock = parsed
        .platform_info
        .system_clocks
        .clock
        .into_iter()
        .find(|c| c.id == "0")
        .ok_or_else(|| XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: "cannot find clock period in platform".into(),
        })?;

    Ok(DeviceInfo {
        part_num: parsed.platform_info.device_info.name,
        clock_period: clock.period,
    })
}

/// Parse an `.xpfm`-adjacent ZIP (`.xsa` / `.dsa`) that holds one
/// `.hpfm` XML entry.
pub fn parse_xpfm(bytes: &[u8]) -> Result<DeviceInfo> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| {
        XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: format!("open archive: {e}"),
        }
    })?;

    let hpfm_idx = (0..archive.len()).find(|&i| {
        archive
            .by_index(i)
            .ok()
            .is_some_and(|e| e.name().ends_with(".hpfm"))
    });
    let Some(idx) = hpfm_idx else {
        return Err(XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: "archive missing .hpfm entry".into(),
        });
    };
    let mut entry = archive
        .by_index(idx)
        .map_err(|e| XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: format!("open .hpfm entry: {e}"),
        })?;
    let mut xml = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut xml)?;
    parse_hpfm_xml(&xml)
}

/// Resolve the `.xsa`/`.dsa` file under `<platform_path>/hw/`, then
/// parse it. Matches the behavior.
pub fn parse_device_info(
    platform_path: &Utf8PathBuf,
    part_num_override: Option<&str>,
    clock_period_override: Option<&str>,
) -> Result<DeviceInfo> {
    if !platform_path.is_dir() {
        return Err(XilinxError::PlatformNotFound(platform_path.clone()));
    }
    let hw = platform_path.join("hw");
    let entries = std::fs::read_dir(&hw).map_err(|_| XilinxError::PlatformNotFound(hw.clone()))?;
    let archive_path = entries
        .filter_map(|e| e.ok())
        .map(|e| Utf8PathBuf::from_path_buf(e.path()).unwrap_or_else(|p| Utf8PathBuf::from(p.to_string_lossy().into_owned())))
        .find(|p| {
            p.extension()
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
            zw.start_file("shell.hpfm", zip::write::SimpleFileOptions::default())
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
            parse_device_info(&Utf8PathBuf::from("/definitely/not/a/platform"), None, None).unwrap_err();
        assert!(matches!(err, XilinxError::PlatformNotFound(_)));
    }
}
