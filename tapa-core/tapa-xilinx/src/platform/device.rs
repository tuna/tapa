//! `.xpfm` / `.hpfm` device config parsing.
//!
//! Implements: the `.xpfm`
//! directory contains a ZIP (`.xsa`/`.dsa`) holding a `<name>.hpfm` XML
//! document; we extract `part_num` and `clock_period` from the
//! `xd:platformInfo` node, following the `xd:` namespace used by the
//! Xilinx tooling.

use camino::Utf8PathBuf;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;

use crate::error::{Result, XilinxError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub part_num: String,
    pub clock_period: String,
}

/// Local-name helper: strips any namespace prefix.
fn local_name(qname: &str) -> &str {
    qname.rsplit_once(':').map_or(qname, |(_, n)| n)
}

/// Extract an attribute value by its local name.
fn attr_value(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Result<Option<String>> {
    for attr in e.attributes() {
        let attr = attr.map_err(|err| XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: format!("cannot parse an attribute of the .hpfm platform XML: {err}"),
        })?;
        let key: &str = attr.key.as_ref();
        if local_name(key) == name {
            let val = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(XilinxError::Xml)?
                .to_string();
            return Ok(Some(val));
        }
    }
    Ok(None)
}

/// Parse the `.hpfm` XML document body (namespace-aware).
///
/// Scans for `platformInfo` / `xd:platformInfo` and extracts
/// `deviceInfo@name` and the period of the platform's default clock: the
/// `clock` whose `id` is `systemClocks@defaultClock`, falling back to id
/// `"0"`. On Alveo shells the default is id 0; a Versal base platform
/// declares another (VCK190: id 2, 312.5 MHz, while id 0 runs at 625 MHz),
/// and `v++` wires the kernel to the default. This is resilient to
/// different nesting depths (e.g. root `platform` vs root `component`).
pub fn parse_hpfm_xml(xml: &[u8]) -> Result<DeviceInfo> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut part_num: Option<String> = None;
    let mut default_clock: Option<String> = None;
    let mut periods: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut in_platform_info = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => {
                return Err(XilinxError::DeviceConfig {
                    path: Utf8PathBuf::new(),
                    detail: format!("cannot parse the .hpfm platform XML: {e}"),
                });
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                let name = local_name(e.name().0);
                if name == "platformInfo" {
                    in_platform_info = true;
                } else if in_platform_info {
                    match name {
                        "deviceInfo" => {
                            part_num = attr_value(e, "name")?;
                        }
                        "systemClocks" => {
                            default_clock = attr_value(e, "defaultClock")?;
                        }
                        "clock" => {
                            if let (Some(id), Some(period)) =
                                (attr_value(e, "id")?, attr_value(e, "period")?)
                            {
                                periods.entry(id).or_insert(period);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = local_name(e.name().0);
                if name == "platformInfo" {
                    in_platform_info = false;
                }
            }
            Ok(_) => {}
        }
        buf.clear();
    }

    let clock_period = default_clock
        .and_then(|id| periods.get(&id).cloned())
        .or_else(|| periods.get("0").cloned());
    match (part_num, clock_period) {
        (Some(part_num), Some(clock_period)) => Ok(DeviceInfo {
            part_num,
            clock_period,
        }),
        (None, _) => Err(XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: "the platform XML does not declare a part number".into(),
        }),
        (_, None) => Err(XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: "the platform XML does not declare a clock period".into(),
        }),
    }
}

/// Parse an `.xpfm`-adjacent ZIP (`.xsa` / `.dsa`) that holds one
/// `.hpfm` XML entry.
pub fn parse_xpfm(bytes: &[u8]) -> Result<DeviceInfo> {
    parse_hpfm_xml(&archive_hpfm(bytes)?)
}

/// The `.hpfm` XML entry of a platform archive (`.xsa` / `.dsa`).
fn archive_hpfm(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| {
        XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: format!("cannot open the platform archive: {e}"),
        }
    })?;

    let hpfm_idx = (0..archive.len()).find(|&i| {
        archive
            .by_index(i)
            .is_ok_and(|e| e.name().is_ok_and(|n| n.ends_with(".hpfm")))
    });
    let Some(idx) = hpfm_idx else {
        return Err(XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: "the platform archive contains no .hpfm entry".into(),
        });
    };
    let mut entry = archive
        .by_index(idx)
        .map_err(|e| XilinxError::DeviceConfig {
            path: Utf8PathBuf::new(),
            detail: format!("cannot read the .hpfm entry: {e}"),
        })?;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "ZIP entry sizes always fit in usize on target platforms"
    )]
    let mut xml = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut xml)?;
    Ok(xml)
}

/// The hardware archive (`.xsa`/`.dsa`) of the platform at `platform_path`:
/// the one its `.xpfm` names as `hardwarePlatform`, which is what `v++`
/// links against. A `hw/` directory may hold several (an extensible
/// platform keeps its base `.xsa` beside the one it ships), so directory
/// order is no answer. Without an `.xpfm`, the sole archive under `hw/`.
fn platform_archive(platform_path: &Utf8PathBuf) -> Result<Utf8PathBuf> {
    if !platform_path.is_dir() {
        return Err(XilinxError::PlatformNotFound(platform_path.clone()));
    }
    let xpfm = std::fs::read_dir(platform_path)?
        .filter_map(std::result::Result::ok)
        .map(|entry| crate::util::utf8(entry.path()))
        .find(|path| path.extension() == Some("xpfm"));
    if let Some(xpfm) = xpfm {
        let named = xpfm_hardware_platform(&std::fs::read(&xpfm)?).map_err(|detail| {
            XilinxError::DeviceConfig {
                path: xpfm.clone(),
                detail,
            }
        })?;
        let archive = platform_path.join(named);
        return if archive.is_file() {
            Ok(archive)
        } else {
            Err(XilinxError::PlatformNotFound(archive))
        };
    }
    let hw = platform_path.join("hw");
    let entries = std::fs::read_dir(&hw).map_err(|_| XilinxError::PlatformNotFound(hw.clone()))?;
    let archives: Vec<Utf8PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| crate::util::utf8(e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "xsa" || x == "dsa"))
        .collect();
    match archives.as_slice() {
        [archive] => Ok(archive.clone()),
        [] => Err(XilinxError::PlatformNotFound(hw)),
        _ => Err(XilinxError::DeviceConfig {
            path: hw,
            detail: "the platform has several hardware archives and no `.xpfm` naming one".into(),
        }),
    }
}

/// The `<path>/<name>` of an `.xpfm`'s `hardwarePlatform` element.
fn xpfm_hardware_platform(xml: &[u8]) -> std::result::Result<Utf8PathBuf, String> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => return Err(format!("cannot parse the .xpfm platform XML: {e}")),
            Ok(Event::Eof) => return Err("the .xpfm names no `hardwarePlatform`".into()),
            Ok(Event::Start(ref e) | Event::Empty(ref e))
                if local_name(e.name().0) == "hardwarePlatform" =>
            {
                let attr = |name| {
                    attr_value(e, name)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("the .xpfm `hardwarePlatform` has no `{name}`"))
                };
                return Ok(Utf8PathBuf::from(attr("path")?).join(attr("name")?));
            }
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// Whether the platform installed at `platform_path` is a Versal device,
/// its `deviceInfo` declaring the `versal` architecture: `v++ --link` makes
/// an `.xsa` for it, which `v++ --package` turns into the xclbin.
pub fn platform_is_versal(platform_path: &Utf8PathBuf) -> Result<bool> {
    let archive_path = platform_archive(platform_path)?;
    let xml = archive_hpfm(&std::fs::read(&archive_path)?)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => {
                return Err(XilinxError::DeviceConfig {
                    path: archive_path,
                    detail: format!("cannot parse the .hpfm platform XML: {e}"),
                });
            }
            Ok(Event::Eof) => return Ok(false),
            Ok(Event::Start(ref e) | Event::Empty(ref e))
                if local_name(e.name().0) == "deviceInfo" =>
            {
                return Ok(attr_value(e, "architecture")?.as_deref() == Some("versal"));
            }
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// Resolve the `.xsa`/`.dsa` file under `<platform_path>/hw/`, then
/// parse it.
pub fn parse_device_info(
    platform_path: &Utf8PathBuf,
    part_num_override: Option<&str>,
    clock_period_override: Option<&str>,
) -> Result<DeviceInfo> {
    let archive_path = platform_archive(platform_path)?;
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
    fn versal_platform_takes_its_declared_default_clock() {
        // The VCK190 base platform: id 0 runs at 625 MHz, but v++ wires
        // kernels to the declared default, id 2.
        let xml = r#"<xd:platformInfo xmlns:xd="http://www.xilinx.com/xd">
          <xd:deviceInfo xd:name="xcvc1902-vsva2197-2MP-e-S"/>
          <xd:systemClocks xd:defaultClock="2">
            <xd:clock xd:name="CPU" xd:period="1000.000000"/>
            <xd:clock xd:id="0" xd:period="1.600000"/>
            <xd:clock xd:id="2" xd:period="3.200000"/>
          </xd:systemClocks>
        </xd:platformInfo>"#;
        let info = parse_hpfm_xml(xml.as_bytes()).unwrap();
        assert_eq!(info.clock_period, "3.200000");
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

    /// A Versal platform's `deviceInfo` names its architecture (the
    /// VCK5000's, abridged); an Alveo shell's names another.
    #[test]
    fn platform_is_versal_reads_the_device_architecture() {
        let platform = |hpfm: &str| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("hw")).unwrap();
            std::fs::write(dir.path().join("hw/hw.xsa"), build_xpfm_zip(hpfm)).unwrap();
            platform_is_versal(&crate::util::utf8(dir.path())).unwrap()
        };
        assert!(platform(
            r#"<xd:platformInfo xmlns:xd="http://www.xilinx.com/xd">
              <xd:deviceInfo xd:name="xcvc1902-vsvd1760-2MP-e-S" xd:architecture="versal"/>
            </xd:platformInfo>"#
        ));
        assert!(!platform(
            r#"<platformInfo><deviceInfo name="xcu55c-fsvh2892-2L-e" architecture="virtexuplusHBM"/></platformInfo>"#
        ));
    }

    /// An extensible platform keeps its base `.xsa` beside the one its
    /// `.xpfm` names (`rapidstream_vhk158_pfm_202320_1`): the named one is
    /// the platform, whatever order the directory lists them in.
    #[test]
    fn the_xpfm_names_the_hardware_archive() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::util::utf8(dir.path());
        std::fs::create_dir(root.join("hw")).unwrap();
        let hpfm = |part: &str| {
            build_xpfm_zip(&format!(
                r#"<xd:platformInfo xmlns:xd="http://www.xilinx.com/xd">
                  <xd:deviceInfo xd:name="{part}"/>
                  <xd:systemClocks xd:defaultClock="0">
                    <xd:clock xd:id="0" xd:period="5.000050"/>
                  </xd:systemClocks>
                </xd:platformInfo>"#
            ))
        };
        std::fs::write(root.join("hw/a_base.xsa"), hpfm("xcbase")).unwrap();
        std::fs::write(root.join("hw/z_ext.xsa"), hpfm("xcext")).unwrap();
        assert!(matches!(
            parse_device_info(&root, None, None),
            Err(XilinxError::DeviceConfig { .. })
        ));
        std::fs::write(
            root.join("pfm.xpfm"),
            r#"<sdx:platform xmlns:sdx="http://www.xilinx.com/sdx"><sdx:hardwarePlatforms>
               <sdx:hardwarePlatform sdx:name="z_ext.xsa" sdx:path="hw"/>
               <sdx:hwEmuPlatform sdx:path="hw_emu" sdx:name="hw_emu.xsa"/>
               </sdx:hardwarePlatforms></sdx:platform>"#,
        )
        .unwrap();
        let info = parse_device_info(&root, None, None).expect("named archive");
        assert_eq!(info.part_num, "xcext");
    }

    #[test]
    fn parse_device_info_nonexistent_path_is_typed_error() {
        let err = parse_device_info(&Utf8PathBuf::from("/definitely/not/a/platform"), None, None)
            .unwrap_err();
        assert!(matches!(err, XilinxError::PlatformNotFound(_)));
    }
}
