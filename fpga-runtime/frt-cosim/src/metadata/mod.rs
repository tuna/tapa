pub mod sax_control;
pub mod xo;
pub mod zip_pkg;

use crate::error::{CosimError, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Hls,
    Vitis,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamDir {
    In,
    Out,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamProtocol {
    Axis,
    ApFifo,
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
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

pub fn load_spec(path: &Path) -> Result<KernelSpec> {
    let bytes = std::fs::read(path)?;
    let cursor = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| CosimError::Metadata(e.to_string()))?;

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
) -> Result<KernelSpec> {
    let mut kernel_xml = None;
    let mut verilog_files = Vec::new();
    let mut tcl_files = Vec::new();
    let mut xci_files = Vec::new();
    let mut scalar_register_map = HashMap::new();

    let out_dir = make_extract_dir("xo")?;
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| CosimError::Metadata(e.to_string()))?;
        let name = file.name().to_owned();
        if name.ends_with('/') {
            continue;
        }
        let path = extract_file(&mut file, &name, &out_dir)?;
        if name.ends_with("kernel.xml") {
            kernel_xml = Some(std::fs::read_to_string(&path)?);
            continue;
        }
        // TAPA generates `<TopName>_control_s_axi.v`; hand-crafted XO files
        // may use the legacy `s_axi_control.v` name.
        if name.ends_with("_control_s_axi.v") || name.ends_with("s_axi_control.v") {
            scalar_register_map = sax_control::parse_register_map(&std::fs::read_to_string(&path)?);
        }
        if has_ext(&name, &["v", "sv", "vh", "dat"]) {
            verilog_files.push(path.clone());
        }
        if has_ext(&name, &["tcl"]) {
            tcl_files.push(path.clone());
        }
        // Only collect shallow XCI files (depth <= 2 components), matching
        // the old Python glob patterns `*.xci` and `*/*.xci`. Deeply nested
        // XCI files inside ip_repo/ are managed by the TCL create_ip scripts
        // and must not be added separately to avoid "IP name already in use".
        if has_ext(&name, &["xci"]) && !is_deeply_nested(&name) {
            xci_files.push(path);
        }
    }

    let xml = kernel_xml
        .ok_or_else(|| CosimError::Metadata(format!("no kernel.xml in {}", src.display())))?;
    let mut spec = xo::parse_kernel_xml(&xml, &out_dir)?;
    spec.verilog_files = verilog_files;
    spec.tcl_files = tcl_files;
    spec.xci_files = xci_files;
    spec.scalar_register_map = scalar_register_map;
    Ok(spec)
}

fn load_zip_spec<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    src: &Path,
) -> Result<KernelSpec> {
    let mut graph_yaml = None;
    let mut settings_yaml = None;
    let out_dir = make_extract_dir("zip")?;
    let mut verilog_files = Vec::new();
    let mut tcl_files = Vec::new();
    let mut xci_files = Vec::new();

    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| CosimError::Metadata(e.to_string()))?;
        let name = file.name().to_owned();
        if name.ends_with('/') {
            continue;
        }
        let path = extract_file(&mut file, &name, &out_dir)?;
        if name.ends_with("graph.yaml") {
            graph_yaml = Some(std::fs::read_to_string(&path)?);
            continue;
        }
        if name.ends_with("settings.yaml") {
            settings_yaml = Some(std::fs::read_to_string(&path)?);
            continue;
        }
        if has_ext(&name, &["v", "sv", "vh", "dat"]) {
            verilog_files.push(path.clone());
        }
        if has_ext(&name, &["tcl"]) {
            tcl_files.push(path.clone());
        }
        if has_ext(&name, &["xci"]) && !is_deeply_nested(&name) {
            xci_files.push(path);
        }
    }

    let yaml = graph_yaml
        .ok_or_else(|| CosimError::Metadata(format!("no graph.yaml in {}", src.display())))?;
    let mut spec = zip_pkg::parse_graph_yaml(&yaml, &out_dir)?;
    if spec.part_num.is_none() {
        spec.part_num = settings_yaml
            .as_deref()
            .and_then(parse_part_from_settings_yaml);
    }
    spec.verilog_files = verilog_files;
    spec.tcl_files = tcl_files;
    spec.xci_files = xci_files;
    Ok(spec)
}

fn make_extract_dir(tag: &str) -> Result<PathBuf> {
    let mut dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("frt-cosim-{tag}-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn parse_part_from_settings_yaml(settings_yaml: &str) -> Option<String> {
    let v: serde_yaml::Value = serde_yaml::from_str(settings_yaml).ok()?;
    v.get("part_num")
        .and_then(|x| x.as_str())
        .map(ToOwned::to_owned)
        .or_else(|| {
            v.get("part")
                .and_then(|x| x.as_str())
                .map(ToOwned::to_owned)
        })
}

/// Returns true if the zip entry path has more than 2 directory components.
/// The old Python cosim used glob `*.xci` and `*/*.xci` which only matched
/// files at depth 0 or 1. Deeply nested files (e.g. inside ip_repo/
/// subdirectories) were not included.
fn is_deeply_nested(zip_name: &str) -> bool {
    zip_name.matches('/').count() > 2
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
