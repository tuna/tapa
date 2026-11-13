pub mod kernel_xml;
pub mod xo;
pub mod zip_pkg;

use crate::error::{CosimError, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Hls,
    Vitis,
}

impl Mode {
    /// The mode string the testbench templates speak.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hls => "hls",
            Self::Vitis => "vitis",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDir {
    In,
    Out,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamProtocol {
    Axis,
    ApFifo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgKind {
    Scalar {
        width: u32,
    },
    Mmap {
        data_width: u32,
        addr_width: u32,
    },
    Stream {
        width: u32,
        depth: u32,
        dir: StreamDir,
        protocol: StreamProtocol,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgSpec {
    pub name: String,
    pub id: u32,
    pub kind: ArgKind,
}

#[derive(Debug, Clone)]
pub struct KernelSpec {
    pub top_name: String,
    pub mode: Mode,
    pub args: Vec<ArgSpec>,
    pub part_num: Option<String>,
    pub verilog_files: Vec<PathBuf>,
    pub tcl_files: Vec<PathBuf>,
    pub xci_files: Vec<PathBuf>,
    pub scalar_register_map: HashMap<String, u32>,
    /// The upper task instances the design's PL kernel was built with
    /// inlined (see [`tapa_ir::InlinedInstance`]); none without the
    /// design's task graph.
    pub inlined: Vec<tapa_ir::InlinedInstance>,
    /// The AI Engine tasks' `async_mmap` ports the host program serves.
    pub memories: Vec<tapa_ir::MemoryCrossing>,
    /// The top's frame arguments rings in device memory carry (see
    /// [`tapa_ir::Ring`]); none without the design's task graph.
    pub rings: Vec<tapa_ir::Ring>,
}

/// An argument's offset in a control slave's register map (see
/// [`KernelSpec::scalar_register_map`]), matched without case sensitivity;
/// an mmap's register is its address, spelled `<name>_offset`.
pub(crate) fn register_offset(map: &HashMap<String, u32>, name: &str) -> Option<u32> {
    let lookup = |key: &str| {
        map.get(key).copied().or_else(|| {
            map.iter().find_map(|(candidate, offset)| {
                candidate.eq_ignore_ascii_case(key).then_some(*offset)
            })
        })
    };
    lookup(name).or_else(|| lookup(&format!("{name}_offset")))
}

/// Normalize a scalar argument's raw bytes to exactly the width declared
/// in the kernel metadata.
///
/// This is the single scalar-binding rule shared by the XRT device and the
/// cosimulation testbenches, so the same scalar binds identically on
/// hardware and in simulation: a buffer shorter than the declared width is
/// zero-extended (little-endian high-order padding), a longer buffer is
/// truncated, and a missing or empty value becomes an all-zero value of
/// the declared width. The width is rounded up to whole bytes, with a
/// one-byte minimum so a zero-width scalar still binds one byte, and the
/// bits above the width in its last byte are cleared: a host `ap_int<12>`
/// holding -5 stores `fb ff`, sign-extended past its 12 bits, and a
/// simulator port expects those bits zero.
pub fn normalized_scalar_bytes(width_bits: u32, raw: Option<&[u8]>) -> Vec<u8> {
    let expected = (width_bits as usize).div_ceil(8).max(1);
    let mut out = raw.map(<[u8]>::to_vec).unwrap_or_default();
    match out.len().cmp(&expected) {
        std::cmp::Ordering::Less => out.resize(expected, 0),
        std::cmp::Ordering::Greater => out.truncate(expected),
        std::cmp::Ordering::Equal => {}
    }
    let partial = width_bits % 8;
    if partial != 0 {
        if let Some(last) = out.last_mut() {
            *last &= (1u8 << partial) - 1;
        }
    }
    out
}

pub fn load_spec(path: &Path) -> Result<(KernelSpec, tempfile::TempDir)> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut zip = zip::ZipArchive::new(reader).map_err(CosimError::metadata)?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("xo") => load_xo_spec(&mut zip, path),
        Some("zip") if zip_contains_kernel_xml(&mut zip)? => load_xo_spec(&mut zip, path),
        Some("zip") => load_zip_spec(&mut zip, path),
        other => Err(CosimError::Metadata(format!(
            "unsupported cosim package extension: {other:?}"
        ))),
    }
}

/// `.zip` is used by both the HLS and SLASH pack flows. SLASH carries the
/// same kernel.xml + RTL contract as an XO, so content, not the suffix, picks
/// the metadata reader and leaves existing fast-cosim setup unchanged.
fn zip_contains_kernel_xml<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<bool> {
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(CosimError::metadata)?;
        if entry
            .name()
            .map_err(CosimError::metadata)?
            .ends_with("kernel.xml")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn load_xo_spec<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    src: &Path,
) -> Result<(KernelSpec, tempfile::TempDir)> {
    let mut xml_text = None;
    let mut part_num = None;
    let mut inlined = Vec::new();
    let mut memories = Vec::new();
    let mut rings = Vec::new();
    let mut scalar_register_map = HashMap::new();

    let (extract_dir, files) = extract_archive(zip, "xo", |name, path, files| {
        if name.ends_with("kernel.xml") {
            xml_text = Some(std::fs::read_to_string(path)?);
            return Ok(());
        }
        if name.ends_with(tapa_ir::work_state::FILE_NAME) {
            let state = tapa_ir::WorkState::from_json(&std::fs::read_to_string(path)?)
                .map_err(CosimError::metadata)?;
            part_num = state.flow.part_num;
            inlined = tapa_ir::inlined_instances(&state.graph).map_err(CosimError::metadata)?;
            memories = tapa_ir::memory_crossings(&state.graph).map_err(CosimError::metadata)?;
            rings = state.graph.rings();
            return Ok(());
        }
        // TAPA generates `<TopName>_control_s_axi.v`; pre-2021 Vitis HLS
        // and hand-crafted XO files may use `s_axi_control.v`.
        if name.ends_with("_control_s_axi.v") || name.ends_with("s_axi_control.v") {
            scalar_register_map =
                tapa_protocol::control_register_offsets(&std::fs::read_to_string(path)?)
                    .into_iter()
                    .collect();
        }
        files.classify(name, path.to_path_buf());
        Ok(())
    })?;

    let xml = xml_text.ok_or_else(|| {
        CosimError::Metadata(format!("no kernel.xml found in `{}`", src.display()))
    })?;
    let parsed = kernel_xml::parse(&xml)?;
    xo::check_register_offsets(&parsed, &scalar_register_map)?;
    let mut spec = xo::spec(parsed);
    spec.part_num = part_num;
    spec.inlined = inlined;
    spec.memories = memories;
    spec.rings = rings;
    spec.verilog_files = files.verilog;
    spec.tcl_files = files.tcl;
    spec.xci_files = files.xci;
    spec.scalar_register_map = scalar_register_map;
    Ok((spec, extract_dir))
}

/// Load a `tapa pack` HLS archive.
///
/// The archive carries exactly one metadata entry — `tapa.json`, the
/// work-directory state file plus the cosim port metadata `tapa pack`
/// stamps for the archive reader — which strict-parses back into
/// [`tapa_ir::WorkState`]: the same types `tapa pack` wrote it from.
fn load_zip_spec<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    src: &Path,
) -> Result<(KernelSpec, tempfile::TempDir)> {
    let mut state_json = None;

    let (extract_dir, files) = extract_archive(zip, "zip", |name, path, files| {
        if name.ends_with(tapa_ir::work_state::FILE_NAME) {
            state_json = Some(std::fs::read_to_string(path)?);
            return Ok(());
        }
        files.classify(name, path.to_path_buf());
        Ok(())
    })?;

    let json = state_json.ok_or_else(|| {
        CosimError::Metadata(format!(
            "no {} in {}",
            tapa_ir::work_state::FILE_NAME,
            src.display(),
        ))
    })?;
    let state = tapa_ir::WorkState::from_json(&json).map_err(CosimError::metadata)?;
    let mut spec = zip_pkg::spec_from_task_graph(&state.graph)?;
    spec.part_num = state.flow.part_num;
    spec.inlined = tapa_ir::inlined_instances(&state.graph).map_err(CosimError::metadata)?;
    spec.memories = tapa_ir::memory_crossings(&state.graph).map_err(CosimError::metadata)?;
    spec.verilog_files = files.verilog;
    spec.tcl_files = files.tcl;
    spec.xci_files = files.xci;
    Ok((spec, extract_dir))
}

/// Extract every real file entry into a fresh `frt-cosim-<tag>-` dir,
/// feeding each (name, extracted path) to the format's own handler; entries
/// the handler does not claim land in the classified file lists.
fn extract_archive<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    tag: &str,
    mut on_entry: impl FnMut(&str, &Path, &mut ExtractedFiles) -> Result<()>,
) -> Result<(tempfile::TempDir, ExtractedFiles)> {
    let mut files = ExtractedFiles::default();
    let extract_dir = make_extract_dir(tag)?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(CosimError::metadata)?;
        let name = file.name().map_err(CosimError::metadata)?.into_owned();
        if name.ends_with('/') {
            continue;
        }
        let path = extract_file(&mut file, &name, extract_dir.path())?;
        on_entry(&name, &path, &mut files)?;
    }
    Ok((extract_dir, files))
}

fn make_extract_dir(tag: &str) -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new()
        .prefix(&format!("frt-cosim-{tag}-"))
        .tempdir()?)
}

/// Files more than two directories deep (e.g. inside `ip_repo`
/// subdirectories) are not collected.
fn is_deeply_nested(zip_name: &str) -> bool {
    zip_name.matches('/').count() > 2
}

#[derive(Default)]
struct ExtractedFiles {
    verilog: Vec<PathBuf>,
    tcl: Vec<PathBuf>,
    xci: Vec<PathBuf>,
}

impl ExtractedFiles {
    fn classify(&mut self, name: &str, path: PathBuf) {
        let name_path = Path::new(name);
        if crate::has_ext(name_path, &["v", "sv", "vh", "dat"], true) {
            self.verilog.push(path.clone());
        }
        if crate::has_ext(name_path, &["tcl"], true) {
            self.tcl.push(path.clone());
        }
        if crate::has_ext(name_path, &["xci"], true) && !is_deeply_nested(name) {
            self.xci.push(path);
        }
    }
}

/// Join a zip entry name under `out_dir`, refusing entries that would
/// land outside it — absolute paths and `..` components ("zip slip"):
/// archives come from user-supplied `.xo`/`.zip` files.
fn safe_extract_path(name: &str, out_dir: &Path) -> Result<PathBuf> {
    use std::path::Component;
    let rel = Path::new(name);
    let contained = rel
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !contained {
        return Err(CosimError::Metadata(format!(
            "archive entry {name:?} would extract outside the staging directory"
        )));
    }
    Ok(out_dir.join(rel))
}

fn extract_file(file: &mut impl Read, name: &str, out_dir: &Path) -> Result<PathBuf> {
    let out = safe_extract_path(name, out_dir)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut fp = std::fs::File::create(&out)?;
    std::io::copy(file, &mut fp)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::normalized_scalar_bytes;
    use super::safe_extract_path;

    #[test]
    fn extract_paths_stay_under_the_staging_dir() {
        let out = std::path::Path::new("/staging");
        assert_eq!(
            safe_extract_path("rtl/top.v", out).expect("plain relative path"),
            out.join("rtl/top.v")
        );
        assert_eq!(
            safe_extract_path("./report.json", out).expect("cur-dir component"),
            out.join("./report.json")
        );
        for evil in ["../evil.v", "rtl/../../evil.v", "/etc/evil"] {
            assert!(
                safe_extract_path(evil, out).is_err(),
                "{evil:?} must be rejected"
            );
        }
    }

    #[test]
    fn short_buffers_are_zero_padded_to_width() {
        assert_eq!(normalized_scalar_bytes(16, Some(&[0x12])), vec![0x12, 0x00]);
        assert_eq!(
            normalized_scalar_bytes(128, Some(&[1, 2, 3, 4])),
            vec![1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn oversized_buffers_are_truncated_to_width() {
        assert_eq!(
            normalized_scalar_bytes(16, Some(&[0x12, 0x34, 0x56])),
            vec![0x12, 0x34]
        );
    }

    #[test]
    fn exactly_sized_buffers_pass_through_unchanged() {
        assert_eq!(
            normalized_scalar_bytes(16, Some(&[0xab, 0xcd])),
            vec![0xab, 0xcd]
        );
        let exact: Vec<u8> = (0u8..16).collect();
        assert_eq!(normalized_scalar_bytes(128, Some(&exact)), exact);
    }

    #[test]
    fn unset_or_empty_buffers_yield_an_all_zero_value_of_width() {
        assert_eq!(normalized_scalar_bytes(16, None), vec![0x00, 0x00]);
        assert_eq!(normalized_scalar_bytes(16, Some(&[])), vec![0x00, 0x00]);
        assert_eq!(normalized_scalar_bytes(128, None), vec![0u8; 16]);
    }

    /// An odd-width scalar carries exactly its bits: a vendor host
    /// `ap_int<12>` holding -5 stores `fb ff` and `ap_int<80>` holding -3
    /// sixteen bytes of sign extension; the simulator port gets only the 12
    /// or 80 bits, and an unsigned value its zero extension.
    #[test]
    fn bits_above_an_odd_width_are_cleared() {
        assert_eq!(
            normalized_scalar_bytes(12, Some(&[0xfb, 0xff])),
            [0xfb, 0x0f]
        );
        let mut minus_three = vec![0xff; 16];
        minus_three[0] = 0xfd;
        let mut expected = vec![0xff; 10];
        expected[0] = 0xfd;
        assert_eq!(normalized_scalar_bytes(80, Some(&minus_three)), expected);
        assert_eq!(
            normalized_scalar_bytes(12, Some(&[0x34, 0x02])),
            [0x34, 0x02]
        );
        assert_eq!(normalized_scalar_bytes(1, Some(&[0xff])), [0x01]);
    }

    #[test]
    fn widths_round_up_to_bytes_with_a_one_byte_minimum() {
        assert_eq!(normalized_scalar_bytes(0, None), vec![0x00]);
        assert_eq!(normalized_scalar_bytes(1, None), vec![0x00]);
        assert_eq!(normalized_scalar_bytes(9, None), vec![0x00, 0x00]);
    }
}
