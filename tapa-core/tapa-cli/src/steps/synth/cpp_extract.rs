//! Extract per-task HLS C++ source files from the task graph into
//! `<work_dir>/cpp/<task>.cpp`.

use std::fs;
use std::path::{Path, PathBuf};

use tapa_ir::{Design, TaskLevel};

use crate::error::{CliError, Result};

/// Argument names that Vitis HLS treats as reserved keywords. Same set
/// as `tapa.safety_check::DISABLED_MMAP_NAME_LIST`. Using one
/// of these as a port name produces inconsistent AXI/interface naming
/// downstream, so reject up front (ran the same check before
/// extracting C++).
const DISABLED_MMAP_NAMES: &[&str] = &[
    "begin", "end", "in", "input", "out", "output", "reg", "wire",
];

fn check_reserved_port_names(design: &Design) -> Result<()> {
    for (task_name, task) in &design.tasks {
        if task.level != TaskLevel::Upper {
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
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use tapa_ir::{
        port::{ArgCategory, Port},
        SynthTarget, Task,
    };

    #[test]
    fn writes_cpp_per_task() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Add".to_string(),
            Task {
                level: TaskLevel::Lower,
                code: "void Add() {}\n".to_string(),
                ports: Vec::new(),
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let design = Design {
            top: "Add".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
        };
        extract_hls_sources(dir.path(), &design).expect("extract");
        let cpp = fs::read_to_string(cpp_path_for(dir.path(), "Add")).expect("read");
        assert_eq!(cpp, "void Add() {}\n");
    }

    #[test]
    fn rejects_reserved_upper_port_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "Top".to_string(),
            Task {
                level: TaskLevel::Upper,
                code: "void Top() {}\n".to_string(),
                ports: vec![Port {
                    cat: ArgCategory::Mmap,
                    name: "in".to_string(),
                    ctype: "int*".to_string(),
                    width: 32,
                    chan_count: None,
                    chan_size: None,
                }],
                tasks: BTreeMap::new(),
                fifos: BTreeMap::new(),
                readable_name: String::new(),
                synth: SynthTarget::Hls,
                self_area: IndexMap::new(),
                total_area: IndexMap::new(),
                clock_period: "0".to_string(),
            },
        );
        let design = Design {
            top: "Top".to_string(),
            target: tapa_ir::Target::XilinxHls,
            tasks,
            cflags: Vec::new(),
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
