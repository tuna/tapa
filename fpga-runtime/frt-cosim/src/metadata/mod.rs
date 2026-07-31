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
        // TAPA generates `<TopName>_control_s_axi.v`.
        if name.ends_with("_control_s_axi.v") {
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
/// verbatim copy of the work-directory state file — which strict-parses back
/// into [`tapa_ir::WorkState`]: the same types `tapa pack` wrote it from.
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

fn extract_file(file: &mut zip::read::ZipFile<'_>, name: &str, out_dir: &Path) -> Result<PathBuf> {
    let rel = Path::new(name);
    let out = out_dir.join(rel);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut fp = std::fs::File::create(&out)?;
    std::io::copy(file, &mut fp)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const KERNEL_XML: &str = r#"<?xml version="1.0"?>
<root>
  <kernel name="vadd">
    <args>
      <arg name="a" addressQualifier="1" id="0" port="m_axi_a" dataWidth="512" addrWidth="64"/>
    </args>
  </kernel>
</root>"#;

    const CONTROL_S_AXI: &str = "localparam ADDR_A_DATA_0 = 6'h10;\n";

    fn xo_archive(entries: &[(&str, &str)]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).expect("start file");
            writer.write_all(contents.as_bytes()).expect("write file");
        }
        let cursor = writer.finish().expect("finish zip");
        zip::ZipArchive::new(cursor).expect("open zip")
    }

    /// The XO scan keys the scalar register map on the TAPA/Vitis
    /// `<TopName>_control_s_axi.v` name. A file named `s_axi_control.v`
    /// (the pre-2021 spelling) must not be picked up as the register-map
    /// source, even when its contents match the new localparam format.
    #[test]
    fn s_axi_control_v_is_not_the_scalar_register_map_source() {
        let mut archive = xo_archive(&[
            ("kernel.xml", KERNEL_XML),
            ("s_axi_control.v", CONTROL_S_AXI),
        ]);
        let (spec, _dir) = load_xo_spec(&mut archive, Path::new("test.xo")).expect("load xo spec");
        assert!(
            spec.scalar_register_map.is_empty(),
            "s_axi_control.v must not be parsed as the scalar register map",
        );
        assert!(
            spec.verilog_files
                .iter()
                .any(|p| p.ends_with("s_axi_control.v")),
            "the file is still classified as RTL",
        );
    }

    /// Positive control for the scan: the `<TopName>_control_s_axi.v` name
    /// is what actually feeds the scalar register map.
    #[test]
    fn control_s_axi_v_feeds_the_scalar_register_map() {
        let mut archive = xo_archive(&[
            ("kernel.xml", KERNEL_XML),
            ("vadd_control_s_axi.v", CONTROL_S_AXI),
        ]);
        let (spec, _dir) = load_xo_spec(&mut archive, Path::new("test.xo")).expect("load xo spec");
        assert_eq!(spec.scalar_register_map.get("a").copied(), Some(0x10));
    }
}
