pub mod kernel_xml;
pub mod sax_control;
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
/// one-byte minimum so a zero-width scalar still binds one byte.
pub fn normalized_scalar_bytes(width_bits: u32, raw: Option<&[u8]>) -> Vec<u8> {
    let expected = (width_bits as usize).div_ceil(8).max(1);
    let mut out = raw.map(<[u8]>::to_vec).unwrap_or_default();
    match out.len().cmp(&expected) {
        std::cmp::Ordering::Less => out.resize(expected, 0),
        std::cmp::Ordering::Greater => out.truncate(expected),
        std::cmp::Ordering::Equal => {}
    }
    out
}

pub fn load_spec(path: &Path) -> Result<(KernelSpec, tempfile::TempDir)> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| CosimError::Metadata(e.to_string()))?;

    match path.extension().and_then(|e| e.to_str()) {
        Some("xo") => load_xo_spec(&mut zip, path),
        Some("zip") => load_zip_spec(&mut zip, path),
        other => Err(CosimError::Metadata(format!(
            "unsupported cosim package extension: {other:?}"
        ))),
    }
}

fn load_xo_spec<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    src: &Path,
) -> Result<(KernelSpec, tempfile::TempDir)> {
    let mut kernel_xml = None;
    let mut scalar_register_map = HashMap::new();
    let mut files = ExtractedFiles::default();

    let extract_dir = make_extract_dir("xo")?;
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| CosimError::Metadata(e.to_string()))?;
        let name = file.name().to_owned();
        if name.ends_with('/') {
            continue;
        }
        let path = extract_file(&mut file, &name, extract_dir.path())?;
        if name.ends_with("kernel.xml") {
            kernel_xml = Some(std::fs::read_to_string(&path)?);
            continue;
        }
        // TAPA generates `<TopName>_control_s_axi.v`; hand-crafted XO files
        // may use the legacy `s_axi_control.v` name.
        if name.ends_with("_control_s_axi.v") || name.ends_with("s_axi_control.v") {
            scalar_register_map = sax_control::parse_register_map(&std::fs::read_to_string(&path)?);
        }
        files.classify(&name, path);
    }

    let xml = kernel_xml
        .ok_or_else(|| CosimError::Metadata(format!("no kernel.xml in {}", src.display())))?;
    let mut spec = xo::parse_kernel_xml(&xml, extract_dir.path())?;
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
    let extract_dir = make_extract_dir("zip")?;
    let mut files = ExtractedFiles::default();

    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| CosimError::Metadata(e.to_string()))?;
        let name = file.name().to_owned();
        if name.ends_with('/') {
            continue;
        }
        let path = extract_file(&mut file, &name, extract_dir.path())?;
        if name.ends_with(tapa_ir::work_state::FILE_NAME) {
            state_json = Some(std::fs::read_to_string(&path)?);
            continue;
        }
        files.classify(&name, path);
    }

    let json = state_json.ok_or_else(|| {
        CosimError::Metadata(format!(
            "no {} in {}",
            tapa_ir::work_state::FILE_NAME,
            src.display(),
        ))
    })?;
    let state =
        tapa_ir::WorkState::from_json(&json).map_err(|e| CosimError::Metadata(e.to_string()))?;
    let mut spec = zip_pkg::spec_from_task_graph(&state.graph)?;
    spec.part_num = state.flow.part_num;
    spec.verilog_files = files.verilog;
    spec.tcl_files = files.tcl;
    spec.xci_files = files.xci;
    Ok((spec, extract_dir))
}

fn make_extract_dir(tag: &str) -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new()
        .prefix(&format!("frt-cosim-{tag}-"))
        .tempdir()?)
}

/// Returns true if the zip entry path has more than 2 directory components.
/// Only files at depth 0 or 1 are collected. Deeply nested files (e.g. inside
/// `ip_repo`/subdirectories) are not included.
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
        if has_ext(name, &["v", "sv", "vh", "dat"]) {
            self.verilog.push(path.clone());
        }
        if has_ext(name, &["tcl"]) {
            self.tcl.push(path.clone());
        }
        if has_ext(name, &["xci"]) && !is_deeply_nested(name) {
            self.xci.push(path);
        }
    }
}

fn has_ext(name: &str, exts: &[&str]) -> bool {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    exts.iter().any(|x| ext.eq_ignore_ascii_case(x))
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

    #[test]
    fn widths_round_up_to_bytes_with_a_one_byte_minimum() {
        assert_eq!(normalized_scalar_bytes(0, None), vec![0x00]);
        assert_eq!(normalized_scalar_bytes(1, None), vec![0x00]);
        assert_eq!(normalized_scalar_bytes(9, None), vec![0x00, 0x00]);
    }
}
