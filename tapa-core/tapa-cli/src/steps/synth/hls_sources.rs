//! Manifest staging for per-task HLS C++ sources.
//!
//! Schema v3 replaced the inline per-task `code` string with a file
//! manifest: `srcs` paths relative to the work dir's `rewritten/` tree,
//! which `tapa analyze` (via `tapacc -emit-dir`) writes. This module is
//! the one place that verifies those files exist and resolves them to
//! absolute paths, for every synth consumer — the HLS jobs and the
//! post-synth utilization pass.

use std::collections::BTreeMap;
use std::path::Path;

use camino::Utf8PathBuf;
use tapa_ir::{Design, TaskLevel};

use crate::error::{CliError, Result};

/// Argument names that Vitis HLS treats as reserved keywords. Using one
/// of these as a port name produces inconsistent AXI/interface naming
/// downstream, so reject up front.
const DISABLED_MMAP_NAMES: &[&str] = &[
    "begin", "end", "in", "input", "out", "output", "reg", "wire",
];

/// Every task's rewritten sources, resolved absolute against the work
/// dir's `rewritten/` tree. Built once by [`stage_hls_sources`] before
/// any HLS job runs, so a missing source fails the whole step up front
/// instead of one parallel worker deep into synthesis.
#[derive(Debug, Clone, Default)]
pub struct TaskSources(BTreeMap<String, Vec<Utf8PathBuf>>);

impl TaskSources {
    /// The one HLS input file of `task_name`.
    ///
    /// The manifest is a list because the schema allows several
    /// translation units per task, but the HLS job path still consumes a
    /// single file per task; a task listing any other count is rejected
    /// here rather than silently compiling only part of it.
    pub fn sole_source(&self, task_name: &str) -> Result<&Utf8PathBuf> {
        match self.srcs(task_name) {
            [only] => Ok(only),
            [] => Err(CliError::Codegen(format!(
                "task `{task_name}` has an empty `srcs` manifest; \
                 re-run `tapa analyze` to regenerate it",
            ))),
            many => Err(CliError::Codegen(format!(
                "task `{task_name}` lists {} srcs; HLS consumes exactly one \
                 source file per task",
                many.len(),
            ))),
        }
    }

    /// All resolved sources of `task_name`, for freshness probes that
    /// must observe every input file.
    pub fn srcs(&self, task_name: &str) -> &[Utf8PathBuf] {
        match self.0.get(task_name) {
            Some(srcs) => srcs,
            None => &[],
        }
    }
}

/// Verify every task's manifest and resolve it against the work dir's
/// `rewritten/` tree, keeping the reserved-port-name rejection.
pub fn stage_hls_sources(work_dir: &Path, design: &Design) -> Result<TaskSources> {
    check_reserved_port_names(design)?;
    let tree = work_dir.join(crate::tapacc::REWRITTEN_DIR);
    let mut staged = BTreeMap::new();
    for (task_name, task) in &design.tasks {
        let mut resolved = Vec::with_capacity(task.srcs.len());
        for src in &task.srcs {
            if Path::new(src).is_absolute() || src.split(['/', '\\']).any(|c| c == "..") {
                return Err(CliError::Codegen(format!(
                    "task `{task_name}` src `{src}` must be a path inside \
                     the rewritten tree, not an absolute or parent-escaping one",
                )));
            }
            let path = tree.join(src);
            if !path.is_file() {
                return Err(CliError::Codegen(format!(
                    "task `{task_name}` src `{src}` does not exist under {}; \
                     re-run `tapa analyze` to rebuild the rewritten tree",
                    tree.display(),
                )));
            }
            resolved.push(crate::util::utf8(path));
        }
        staged.insert(task_name.clone(), resolved);
    }
    Ok(TaskSources(staged))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    use tapa_ir::{
        port::{ArgCategory, Port},
        SynthTarget, Task,
    };

    fn task(srcs: &[&str]) -> Task {
        Task {
            level: TaskLevel::Lower,
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            readable_name: String::new(),
            synth: SynthTarget::Hls,
            ports: Vec::new(),
            tasks: BTreeMap::new(),
            fifos: BTreeMap::new(),
            self_area: None,
            total_area: None,
            clock_period: None,
        }
    }

    fn design(tasks: &[(&str, Task)]) -> Design {
        Design {
            schema_version: tapa_ir::graph::SCHEMA_VERSION,
            top: tasks[0].0.to_string(),
            target: tapa_ir::Target::XilinxHls,
            cflags: Vec::new(),
            tasks: tasks
                .iter()
                .map(|(name, t)| ((*name).to_string(), t.clone()))
                .collect(),
        }
    }

    fn write_src(dir: &Path, name: &str) {
        let tree = dir.join(crate::tapacc::REWRITTEN_DIR);
        fs::create_dir_all(&tree).expect("mkdir rewritten");
        fs::write(tree.join(format!("{name}.cpp")), "int main(){}\n").expect("write src");
    }

    #[test]
    fn resolves_tree_relative_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        let d = design(&[("Add", task(&["Add.cpp"]))]);
        let sources = stage_hls_sources(dir.path(), &d).expect("stage");
        let resolved = sources.sole_source("Add").expect("sole source");
        assert_eq!(
            resolved,
            &crate::util::utf8(
                dir.path()
                    .join(crate::tapacc::REWRITTEN_DIR)
                    .join("Add.cpp")
            )
        );
        assert_eq!(sources.srcs("Add").len(), 1);
    }

    #[test]
    fn missing_src_names_the_task_and_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = design(&[("Add", task(&["Add.cpp"]))]);
        let err = stage_hls_sources(dir.path(), &d).expect_err("missing src must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("`Add`") && msg.contains("Add.cpp"),
            "got: {msg}"
        );
    }

    #[test]
    fn escaping_src_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        fs::write(dir.path().join("evil.cpp"), "x").expect("write decoy");
        for bad in ["../evil.cpp", "/etc/passwd"] {
            let d = design(&[("Add", task(&[bad]))]);
            let err = stage_hls_sources(dir.path(), &d).expect_err("escaping src must fail");
            assert!(
                err.to_string().contains("inside the rewritten tree"),
                "{err}"
            );
        }
    }

    #[test]
    fn sole_source_rejects_empty_and_multi() {
        let sources = TaskSources::default();
        let err = sources.sole_source("Add").expect_err("empty must fail");
        assert!(err.to_string().contains("empty `srcs` manifest"), "{err}");

        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        let d = design(&[("Add", task(&["Add.cpp", "Add2.cpp"]))]);
        write_src(dir.path(), "Add2");
        let sources = stage_hls_sources(dir.path(), &d).expect("stage");
        let err = sources.sole_source("Add").expect_err("multi must fail");
        assert!(err.to_string().contains("exactly one"), "{err}");
    }

    #[test]
    fn rejects_reserved_upper_port_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Top");
        let mut top = task(&["Top.cpp"]);
        top.level = TaskLevel::Upper;
        top.ports = vec![Port {
            cat: ArgCategory::Mmap,
            name: "in".to_string(),
            ctype: "int*".to_string(),
            width: 32,
            chan_count: None,
            chan_size: None,
            stream_depth: None,
            mmap_addr_width: None,
        }];
        let d = design(&[("Top", top)]);
        let err = stage_hls_sources(dir.path(), &d).expect_err("must reject reserved name");
        assert!(
            matches!(err, crate::error::CliError::InvalidArg(ref m)
                if m.contains("reserved keyword") && m.contains("`in`")),
            "expected reserved-keyword diagnostic: {err:?}"
        );
    }
}
