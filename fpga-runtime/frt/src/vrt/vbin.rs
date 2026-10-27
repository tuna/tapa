//! SLASH VBIN extraction and typed `system_map.xml` metadata.
//!
//! The extraction directory is owned by [`Vbin`], keeping extracted member
//! paths valid for the backend's lifetime.

use flate2::read::GzDecoder;
use quick_xml::de::from_str;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, EntryType};
use tempfile::TempDir;

use crate::error::{FrtError, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Hardware,
    Simulation,
    Emulation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgKind {
    Scalar,
    Buffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryTarget {
    Ddr,
    Hbm(u8),
    /// SLASH's allocator-selected HBM target (`HBM`, `HBM_VNOC`, or `MEM`).
    HbmVnoc,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Arg {
    pub index: u32,
    pub name: String,
    pub kind: ArgKind,
    pub offset: u32,
    /// Width in bits as recorded in the system map. Zero means 32, matching
    /// VRT's treatment of an omitted register width.
    pub range: u32,
    pub readable: bool,
    pub writable: bool,
    pub port: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kernel {
    pub name: String,
    pub base_address: u64,
    pub range: u64,
    pub args: Vec<Arg>,
    /// AXI port to its typed memory target.
    pub connections: BTreeMap<String, MemoryTarget>,
}

impl Kernel {
    pub(crate) fn arg(&self, index: u32) -> Result<&Arg> {
        self.args
            .iter()
            .find(|arg| arg.index == index)
            .ok_or_else(|| FrtError::Vrt(format!("unknown VRT argument index {index}")))
    }

    pub(crate) fn memory_target(&self, arg: &Arg) -> Result<MemoryTarget> {
        let port = arg
            .port
            .as_deref()
            .filter(|port| !port.is_empty())
            .ok_or_else(|| {
                FrtError::Vrt(format!(
                    "buffer argument {} (`{}`) has no AXI port",
                    arg.index, arg.name
                ))
            })?;
        self.connections.get(port).copied().ok_or_else(|| {
            FrtError::Vrt(format!(
                "kernel `{}` has no memory connection for port `{port}`",
                self.name
            ))
        })
    }
}

/// A fully extracted VBIN and the typed metadata shared by VRT backends.
#[derive(Debug)]
pub struct Vbin {
    /// Owns every extracted path for as long as a backend needs them.
    temp_dir: TempDir,
    pub platform: Platform,
    pub clock_frequency_hz: u64,
    pub kernel: Kernel,
    pub pdi_paths: Vec<PathBuf>,
    /// Path to the extracted `vpp_sim`, when present.
    #[allow(dead_code, reason = "hardware execution does not launch simulation")]
    pub simulation_executable: Option<PathBuf>,
}

impl Vbin {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let temp_dir = tempfile::tempdir()
            .map_err(|error| FrtError::Vrt(format!("cannot create VBIN temp dir: {error}")))?;
        let files = extract(path, temp_dir.path())?;
        let system_map = exactly_one(files.system_maps, "system_map.xml")?.ok_or_else(|| {
            FrtError::Vrt(format!("{} contains no system_map.xml", path.display()))
        })?;
        let xml = std::fs::read_to_string(&system_map).map_err(|error| {
            FrtError::Vrt(format!(
                "cannot read extracted `{}`: {error}",
                system_map.display()
            ))
        })?;
        let metadata = parse_system_map(&xml)?;
        let mut pdi_paths = files.pdis;
        pdi_paths.sort();
        Ok(Self {
            temp_dir,
            platform: metadata.platform,
            clock_frequency_hz: metadata.clock_frequency_hz,
            kernel: metadata.kernel,
            pdi_paths,
            simulation_executable: exactly_one(files.simulators, "vpp_sim")?,
        })
    }

    /// Root of the owned archive extraction directory.
    #[allow(
        dead_code,
        reason = "hardware execution only retains extracted PDI paths"
    )]
    pub(crate) fn extraction_dir(&self) -> &Path {
        self.temp_dir.path()
    }
}

#[derive(Default)]
struct ExtractedFiles {
    system_maps: Vec<PathBuf>,
    pdis: Vec<PathBuf>,
    simulators: Vec<PathBuf>,
}

fn exactly_one(mut paths: Vec<PathBuf>, name: &str) -> Result<Option<PathBuf>> {
    match paths.len() {
        0 => Ok(None),
        1 => Ok(paths.pop()),
        count => Err(FrtError::Vrt(format!(
            "VBIN contains {count} files named `{name}`; expected at most one"
        ))),
    }
}

fn extract(path: &Path, destination: &Path) -> Result<ExtractedFiles> {
    let file = File::open(path)
        .map_err(|error| FrtError::Vrt(format!("cannot open `{}`: {error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let gzip = reader
        .fill_buf()
        .map_err(|error| FrtError::Vrt(format!("cannot inspect `{}`: {error}", path.display())))?
        .starts_with(&[0x1f, 0x8b]);
    let input: Box<dyn Read> = if gzip {
        Box::new(GzDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    let mut archive = Archive::new(input);
    let entries = archive.entries().map_err(|error| {
        FrtError::Vrt(format!(
            "cannot read VBIN tar `{}`: {error}",
            path.display()
        ))
    })?;
    let mut files = ExtractedFiles::default();
    let mut seen_files = BTreeSet::new();

    for entry in entries {
        let mut entry = entry.map_err(|error| {
            FrtError::Vrt(format!("invalid VBIN tar `{}`: {error}", path.display()))
        })?;
        let entry_type = entry.header().entry_type();
        if entry_type != EntryType::Regular && entry_type != EntryType::Directory {
            return Err(FrtError::Vrt(format!(
                "VBIN entry `{}` is not a regular file or directory; links and special files are not allowed",
                entry.path().map_or_else(|_| "<invalid>".into(), |p| p.display().to_string())
            )));
        }
        let entry_path = entry.path().map_err(|error| {
            FrtError::Vrt(format!("invalid path in `{}`: {error}", path.display()))
        })?;
        let Some(relative) = safe_relative_path(&entry_path)? else {
            continue;
        };
        if entry_type == EntryType::Regular && !seen_files.insert(relative.clone()) {
            return Err(FrtError::Vrt(format!(
                "VBIN contains duplicate file `{}`",
                relative.display()
            )));
        }
        entry.set_preserve_permissions(false);
        entry.set_preserve_mtime(false);
        entry.set_unpack_xattrs(false);
        let unpacked = entry.unpack_in(destination).map_err(|error| {
            FrtError::Vrt(format!(
                "cannot extract VBIN entry `{}`: {error}",
                relative.display()
            ))
        })?;
        if !unpacked {
            return Err(FrtError::Vrt(format!(
                "VBIN entry `{}` escapes the extraction directory",
                relative.display()
            )));
        }
        if entry_type != EntryType::Regular {
            continue;
        }
        let extracted = destination.join(&relative);
        match relative.file_name().and_then(|name| name.to_str()) {
            Some("system_map.xml") => files.system_maps.push(extracted),
            Some("vpp_sim") => files.simulators.push(extracted),
            _ if relative.extension().and_then(|ext| ext.to_str()) == Some("pdi") => {
                files.pdis.push(extracted);
            }
            _ => {}
        }
    }
    Ok(files)
}

/// Normalize an archive path without ever allowing it to leave `destination`.
fn safe_relative_path(path: &Path) -> Result<Option<PathBuf>> {
    if path.is_absolute() {
        return Err(FrtError::Vrt(format!(
            "VBIN contains absolute path `{}`",
            path.display()
        )));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FrtError::Vrt(format!(
                    "VBIN contains unsafe path `{}`",
                    path.display()
                )));
            }
        }
    }
    Ok((!clean.as_os_str().is_empty()).then_some(clean))
}

#[derive(Debug)]
struct Metadata {
    platform: Platform,
    clock_frequency_hz: u64,
    kernel: Kernel,
}

#[derive(Deserialize)]
#[serde(rename = "SystemMap")]
struct RawSystemMap {
    #[serde(rename = "Platform")]
    platform: String,
    #[serde(rename = "ClockFrequency")]
    clock_frequency: String,
    #[serde(rename = "Kernel", default)]
    kernels: Vec<RawKernel>,
}

#[derive(Deserialize)]
struct RawKernel {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "BaseAddress")]
    base_address: String,
    #[serde(rename = "Range")]
    range: String,
    #[serde(rename = "functional_args", default)]
    args: RawArgs,
    #[serde(rename = "connection", default)]
    connections: Vec<RawConnection>,
}

#[derive(Default, Deserialize)]
struct RawArgs {
    #[serde(rename = "arg", default)]
    entries: Vec<RawArg>,
}

#[derive(Deserialize)]
struct RawArg {
    #[serde(rename = "@idx")]
    index: String,
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@type")]
    kind: String,
    #[serde(rename = "@offset")]
    offset: String,
    #[serde(rename = "@range", default)]
    range: String,
    #[serde(rename = "@r", default)]
    readable: String,
    #[serde(rename = "@w", default)]
    writable: String,
    #[serde(rename = "@port", default)]
    port: String,
}

#[derive(Deserialize)]
struct RawConnection {
    #[serde(rename = "@port")]
    port: String,
    #[serde(rename = "@target")]
    target: String,
}

fn parse_system_map(xml: &str) -> Result<Metadata> {
    let raw: RawSystemMap = from_str(xml)
        .map_err(|error| FrtError::Vrt(format!("cannot parse system_map.xml: {error}")))?;
    let platform = match raw.platform.trim() {
        "Hardware" => Platform::Hardware,
        "Simulation" => Platform::Simulation,
        "Emulation" => Platform::Emulation,
        other => {
            return Err(FrtError::Vrt(format!("unknown VBIN platform `{other}`")));
        }
    };
    let clock_frequency_hz = parse_u64(&raw.clock_frequency, "ClockFrequency")?;
    if clock_frequency_hz == 0 {
        return Err(FrtError::Vrt(
            "system_map.xml has a zero ClockFrequency".into(),
        ));
    }
    if raw.kernels.len() != 1 {
        return Err(FrtError::Vrt(format!(
            "system_map.xml contains {} kernels; a TAPA VBIN must contain exactly one",
            raw.kernels.len()
        )));
    }
    let raw_kernel = raw.kernels.into_iter().next().expect("length checked");
    let name = raw_kernel.name.trim().to_owned();
    if name.is_empty() {
        return Err(FrtError::Vrt(
            "system_map.xml has an empty kernel name".into(),
        ));
    }
    let base_address = parse_u64(&raw_kernel.base_address, "Kernel BaseAddress")?;
    let range = parse_u64(&raw_kernel.range, "Kernel Range")?;
    if !base_address.is_multiple_of(4) {
        return Err(FrtError::Vrt(format!(
            "kernel `{name}` base address {base_address:#x} is not 32-bit aligned"
        )));
    }
    let mut args = raw_kernel
        .args
        .entries
        .into_iter()
        .map(parse_arg)
        .collect::<Result<Vec<_>>>()?;
    args.sort_by_key(|arg| arg.index);
    if let Some(pair) = args.windows(2).find(|pair| pair[0].index == pair[1].index) {
        return Err(FrtError::Vrt(format!(
            "kernel `{name}` has duplicate argument index {}",
            pair[0].index
        )));
    }
    let mut connections = BTreeMap::new();
    for connection in raw_kernel.connections {
        let target = parse_memory_target(&connection.target)?;
        if connections
            .insert(connection.port.clone(), target)
            .is_some()
        {
            return Err(FrtError::Vrt(format!(
                "kernel `{name}` has duplicate connection for port `{}`",
                connection.port
            )));
        }
    }
    Ok(Metadata {
        platform,
        clock_frequency_hz,
        kernel: Kernel {
            name,
            base_address,
            range,
            args,
            connections,
        },
    })
}

fn parse_arg(raw: RawArg) -> Result<Arg> {
    let index = parse_u32(&raw.index, "argument idx")?;
    let kind = match raw.kind.trim() {
        "scalar" => ArgKind::Scalar,
        "buffer" => ArgKind::Buffer,
        other => {
            return Err(FrtError::Vrt(format!(
                "argument {index} has unsupported type `{other}`"
            )));
        }
    };
    let offset = parse_u32(&raw.offset, "argument offset")?;
    if !offset.is_multiple_of(4) {
        return Err(FrtError::Vrt(format!(
            "argument {index} offset {offset:#x} is not 32-bit aligned"
        )));
    }
    let range = if raw.range.trim().is_empty() {
        32
    } else {
        parse_u32(&raw.range, "argument range")?
    };
    if range > 64 {
        return Err(FrtError::Vrt(format!(
            "argument {index} is {range} bits wide; VRT supports at most 64"
        )));
    }
    Ok(Arg {
        index,
        name: raw.name,
        kind,
        offset,
        range,
        readable: parse_bool(&raw.readable, "argument r")?,
        writable: parse_bool(&raw.writable, "argument w")?,
        port: (!raw.port.trim().is_empty()).then(|| raw.port.trim().to_owned()),
    })
}

fn parse_memory_target(target: &str) -> Result<MemoryTarget> {
    let target = target.trim();
    if target == "MEM" || target == "HBM" || target == "HBM_VNOC" {
        return Ok(MemoryTarget::HbmVnoc);
    }
    if let Some(bank) = target.strip_prefix("HBM") {
        let bank = bank
            .parse::<u8>()
            .ok()
            .filter(|bank| *bank <= 63)
            .ok_or_else(|| FrtError::Vrt(format!("invalid HBM memory target `{target}`")))?;
        return Ok(MemoryTarget::Hbm(bank));
    }
    if let Some(bank) = target.strip_prefix("DDR") {
        if bank.len() == 1 && bank.bytes().all(|digit| digit.is_ascii_digit()) {
            return Ok(MemoryTarget::Ddr);
        }
    }
    Err(FrtError::Vrt(format!(
        "unsupported memory target `{target}`"
    )))
}

fn parse_u64(text: &str, field: &str) -> Result<u64> {
    let text = text.trim();
    let parsed = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map_or_else(|| text.parse::<u64>(), |hex| u64::from_str_radix(hex, 16));
    parsed.map_err(|error| FrtError::Vrt(format!("invalid {field} value `{text}`: {error}")))
}

fn parse_u32(text: &str, field: &str) -> Result<u32> {
    let value = parse_u64(text, field)?;
    u32::try_from(value)
        .map_err(|error| FrtError::Vrt(format!("invalid {field} value `{text}`: {error}")))
}

fn parse_bool(text: &str, field: &str) -> Result<bool> {
    if text.trim().is_empty() {
        return Ok(false);
    }
    Ok(parse_u32(text, field)? != 0)
}

/// Lower a scalar or device address to little-endian 32-bit register writes.
/// Values narrower than their recorded register are zero-padded, including
/// sub-u32 scalars; wider host values are rejected rather than truncated.
pub fn register_words(offset: u32, range: u32, value: &[u8]) -> Result<Vec<(u32, u32)>> {
    if !offset.is_multiple_of(4) {
        return Err(FrtError::Vrt(format!(
            "register offset {offset:#x} is not 32-bit aligned"
        )));
    }
    let bits = if range == 0 { 32 } else { range };
    if bits > 64 {
        return Err(FrtError::Vrt(format!(
            "register at {offset:#x} is {bits} bits wide; VRT supports at most 64"
        )));
    }
    let word_count = bits.div_ceil(32).max(1);
    let byte_count = usize::try_from(word_count * 4)
        .map_err(|error| FrtError::Vrt(format!("invalid register width: {error}")))?;
    let value_byte_count = usize::try_from(bits.div_ceil(8))
        .map_err(|error| FrtError::Vrt(format!("invalid value width: {error}")))?;
    if value.len() > value_byte_count {
        return Err(FrtError::Vrt(format!(
            "{}-byte value does not fit the {bits}-bit register at {offset:#x}",
            value.len()
        )));
    }
    let mut padded = vec![0; byte_count];
    padded[..value.len()].copy_from_slice(value);
    padded
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            let word_offset = u32::try_from(index * 4)
                .ok()
                .and_then(|delta| offset.checked_add(delta))
                .ok_or_else(|| FrtError::Vrt("register offset overflow".into()))?;
            Ok((word_offset, u32::from_le_bytes(*chunk)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_system_map, ArgKind, MemoryTarget, Platform, Vbin};
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;
    use std::path::Path;
    use tar::{Builder, Header};

    const VADD_MAP: &str = r#"<?xml version="1.0"?>
<SystemMap>
  <Platform>Hardware</Platform>
  <ClockFrequency>300000000</ClockFrequency>
  <ServiceLayer><Ethernet enabled="false" /></ServiceLayer>
  <Kernel>
    <Name>VecAdd_0</Name>
    <BaseAddress>0x20200000000</BaseAddress>
    <Range>0x1000</Range>
    <register offset="0" name="CTRL" access="RW" range="32" />
    <functional_args>
      <arg idx="0" name="a" type="buffer" offset="0x10" range="64" r="1" w="1" port="m_axi_a" />
      <arg idx="1" name="b" type="buffer" offset="0x18" range="64" r="1" w="1" port="m_axi_b" />
      <arg idx="2" name="n" type="scalar" offset="0x20" range="64" r="1" w="1" />
    </functional_args>
    <connection port="m_axi_a" target="HBM0" />
    <connection port="m_axi_b" target="DDR0" />
  </Kernel>
</SystemMap>"#;

    fn append(archive: &mut Builder<&mut Vec<u8>>, path: &str, bytes: &[u8], mode: u32) {
        let mut header = Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        // Write the name directly so the traversal fixture can represent a
        // malicious archive that tar::Builder would itself refuse to create.
        header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        archive
            .append(&header, bytes)
            .expect("append fixture member");
    }

    fn archive(path: &Path, xml: &str, gzip: bool, extra: Option<(&str, &[u8])>) {
        let mut tar = Vec::new();
        let mut builder = Builder::new(&mut tar);
        append(&mut builder, "system_map.xml", xml.as_bytes(), 0o644);
        append(&mut builder, "images/partial.pdi", b"PDI", 0o644);
        append(&mut builder, "vpp_sim", b"SIM", 0o755);
        if let Some((path, bytes)) = extra {
            append(&mut builder, path, bytes, 0o644);
        }
        builder.finish().expect("finish fixture tar");
        drop(builder);
        let bytes = if gzip {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&tar).expect("gzip fixture");
            encoder.finish().expect("finish fixture gzip")
        } else {
            tar
        };
        std::fs::write(path, bytes).expect("write fixture VBIN");
    }

    #[test]
    fn plain_and_gzip_vbins_expose_the_same_typed_metadata() {
        for gzip in [false, true] {
            let temp = tempfile::tempdir().expect("temp dir");
            let path = temp.path().join("vadd.vbin");
            archive(&path, VADD_MAP, gzip, None);
            let vbin = Vbin::open(&path).expect("open VBIN");
            assert_eq!(vbin.platform, Platform::Hardware);
            assert_eq!(vbin.clock_frequency_hz, 300_000_000);
            assert_eq!(vbin.kernel.name, "VecAdd_0");
            assert_eq!(vbin.kernel.base_address, 0x202_0000_0000);
            assert_eq!(vbin.kernel.args.len(), 3);
            assert_eq!(vbin.kernel.args[2].kind, ArgKind::Scalar);
            assert_eq!(vbin.kernel.connections["m_axi_a"], MemoryTarget::Hbm(0));
            assert_eq!(vbin.kernel.connections["m_axi_b"], MemoryTarget::Ddr);
            assert_eq!(vbin.pdi_paths.len(), 1);
            assert!(vbin.pdi_paths[0].is_file());
            assert!(vbin.simulation_executable.as_ref().expect("sim").is_file());
            assert!(vbin.extraction_dir().join("system_map.xml").is_file());
        }
    }

    #[test]
    fn parser_sorts_args_and_accepts_vrt_memory_aliases() {
        let xml = VADD_MAP
            .replace("idx=\"0\" name=\"a\"", "idx=\"2\" name=\"a\"")
            .replace("idx=\"2\" name=\"n\"", "idx=\"0\" name=\"n\"")
            .replace("target=\"HBM0\"", "target=\"MEM\"");
        let metadata = parse_system_map(&xml).expect("parse metadata");
        assert_eq!(metadata.kernel.args[0].name, "n");
        assert_eq!(metadata.kernel.args[2].name, "a");
        assert_eq!(
            metadata.kernel.connections["m_axi_a"],
            MemoryTarget::HbmVnoc
        );
    }

    #[test]
    fn parser_rejects_multiple_kernels_and_bad_memory_targets() {
        let second_kernel =
            "<Kernel><Name>other</Name><BaseAddress>0</BaseAddress><Range>0x1000</Range></Kernel>";
        let xml = VADD_MAP.replace("</SystemMap>", &format!("{second_kernel}</SystemMap>"));
        assert!(parse_system_map(&xml)
            .expect_err("two kernels")
            .to_string()
            .contains("exactly one"));
        let xml = VADD_MAP.replace("target=\"HBM0\"", "target=\"HOST\"");
        assert!(parse_system_map(&xml)
            .expect_err("HOST is not a device allocation")
            .to_string()
            .contains("unsupported memory target"));
    }

    #[test]
    fn extraction_rejects_unsafe_paths() {
        for (member, expected) in [("../escape", "unsafe path"), ("/absolute", "absolute path")] {
            let temp = tempfile::tempdir().expect("temp dir");
            let path = temp.path().join("unsafe.vbin");
            archive(&path, VADD_MAP, false, Some((member, b"bad")));
            let error = Vbin::open(&path).expect_err("unsafe path must fail");
            assert!(error.to_string().contains(expected), "{error}");
            assert!(!temp.path().join("escape").exists());
        }
    }

    #[test]
    fn register_writes_zero_pad_narrow_values() {
        assert_eq!(
            super::register_words(0x20, 32, &[0x34, 0x12]).expect("u16 scalar"),
            vec![(0x20, 0x1234)]
        );
        assert_eq!(
            super::register_words(0x10, 64, &0x0123_4567_89ab_cdefu64.to_le_bytes())
                .expect("device address"),
            vec![(0x10, 0x89ab_cdef), (0x14, 0x0123_4567)]
        );
        assert!(super::register_words(0x20, 16, &[1, 2, 3])
            .expect_err("wide scalar")
            .to_string()
            .contains("does not fit"));
    }
}
