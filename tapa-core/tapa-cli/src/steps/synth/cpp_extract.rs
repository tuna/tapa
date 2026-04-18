//! Extract per-task HLS C++ source files from `design.json` into
//! `<work_dir>/cpp/<task>.cpp`.
//!
//! Mirrors `tapa/program/hls.py::ProgramHlsMixin._extract_cpp("hls")`.

use std::fs;
use std::path::{Path, PathBuf};

use tapa_task_graph::Design;

use crate::error::Result;

pub fn cpp_path_for(work_dir: &Path, task_name: &str) -> PathBuf {
    work_dir.join("cpp").join(format!("{task_name}.cpp"))
}

pub fn extract_hls_sources(work_dir: &Path, design: &Design) -> Result<()> {
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
    use tapa_task_graph::TaskTopology;

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
}
