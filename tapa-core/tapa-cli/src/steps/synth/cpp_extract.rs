//! Extract per-task HLS C++ source files from `design.json` into
//! `<work_dir>/cpp/<task>.cpp`.
//!
//! Mirrors `tapa/program/hls.py::ProgramHlsMixin._extract_cpp("hls")`.

use std::fs;
use std::path::{Path, PathBuf};

use tapa_task_graph::Design;

use crate::error::{CliError, Result};

/// Argument names that Vitis HLS treats as reserved keywords. Same set
/// as Python's `tapa.safety_check::DISABLED_MMAP_NAME_LIST`. Using one
/// of these as a port name produces inconsistent AXI/interface naming
/// downstream, so reject up front (Python ran the same check before
/// extracting C++).
const DISABLED_MMAP_NAMES: &[&str] = &[
    "begin", "end", "in", "input", "out", "output", "reg", "wire",
];

fn check_reserved_port_names(design: &Design) -> Result<()> {
    for (task_name, task) in &design.tasks {
        if task.level != "upper" {
            continue;
        }
        for port in &task.ports {
            if DISABLED_MMAP_NAMES.contains(&port.name.as_str()) {
                return Err(CliError::InvalidArg(format!(
                    "task `{task_name}` argument `{}` is a reserved keyword \
                     ({DISABLED_MMAP_NAMES:?}); rename it before running synth — \
                     Vitis HLS would otherwise emit inconsistent AXI/interface \
                     naming.",
                    port.name,
                )));
            }
        }
    }
    Ok(())
}

pub fn cpp_path_for(work_dir: &Path, task_name: &str) -> PathBuf {
    work_dir.join("cpp").join(format!("{task_name}.cpp"))
}

pub fn extract_hls_sources(work_dir: &Path, design: &Design) -> Result<()> {
    check_reserved_port_names(design)?;
    let cpp_dir = work_dir.join("cpp");
    fs::create_dir_all(&cpp_dir)?;
    for (task_name, task) in &design.tasks {
        let path = cpp_path_for(work_dir, task_name);
        let content = task.code.as_bytes();
        if let Ok(existing) = fs::read(&path) {
            if existing.as_slice() == content {
                continue;
            }
        }
        fs::write(&path, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use tapa_task_graph::{
        port::{ArgCategory, Port},
        TaskTopology,
    };

    #[test]
    fn writes_cpp_per_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Add".to_string(),
            TaskTopology {
                name: "Add".to_string(),
                level: "lower".to_string(),
                code: "void Add() {}\n".to_string(),
                ports: Vec::new(),
                tasks: IndexMap::new(),
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let design = Design {
            top: "Add".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        };
        extract_hls_sources(dir.path(), &design).expect("extract");
        let cpp = fs::read_to_string(cpp_path_for(dir.path(), "Add")).expect("read");
        assert_eq!(cpp, "void Add() {}\n");
    }

    #[test]
    fn rejects_reserved_upper_port_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = IndexMap::new();
        tasks.insert(
            "Top".to_string(),
            TaskTopology {
                name: "Top".to_string(),
                level: "upper".to_string(),
                code: "void Top() {}\n".to_string(),
                ports: vec![Port {
                    cat: ArgCategory::Mmap,
                    name: "in".to_string(),
                    ctype: "int*".to_string(),
                    width: 32,
                    chan_count: None,
                    chan_size: None,
                }],
                tasks: IndexMap::new(),
                fifos: IndexMap::new(),
                target: Some("hls".to_string()),
                is_slot: false,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let design = Design {
            top: "Top".to_string(),
            target: "xilinx-hls".to_string(),
            tasks,
            slot_task_name_to_fp_region: None,
        };
        let err = extract_hls_sources(dir.path(), &design).expect_err("must reject reserved name");
        assert!(
            matches!(err, crate::error::CliError::InvalidArg(ref m)
                if m.contains("reserved keyword") && m.contains("`in`")),
            "expected reserved-keyword diagnostic: {err:?}"
        );
        assert!(
            !dir.path().join("cpp").join("Top.cpp").exists(),
            "must not write any cpp/* files when validation fails"
        );
    }
}
