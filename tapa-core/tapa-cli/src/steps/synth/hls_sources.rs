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
    /// All resolved sources of `task_name`, non-empty for any task that
    /// passed staging. HLS consumes the whole list; freshness probes
    /// use it to observe every input file.
    pub fn srcs(&self, task_name: &str) -> &[Utf8PathBuf] {
        match self.0.get(task_name) {
            Some(srcs) => srcs,
            None => &[],
        }
    }
}

/// Verify every task's manifest and resolve it against the work dir's
/// `rewritten/` tree, keeping the reserved-port-name rejection.
///
/// Include dirs and guard defines are additionally restricted to
/// `[A-Za-z0-9_/]` and `[A-Za-z0-9_]` respectively. Both land in the
/// HLS TCL's `add_files -cflags` string, which Tcl reads as a
/// double-quoted word — rather than quoting them (which Vitis treats
/// literally and breaks flags), the pipeline asserts the safe charset
/// and rejects anything outside it with a clear error.
pub fn stage_hls_sources(work_dir: &Path, design: &Design) -> Result<TaskSources> {
    check_reserved_port_names(design)?;
    let tree = work_dir.join(crate::tapacc::REWRITTEN_DIR);
    let mut staged = BTreeMap::new();
    for (task_name, task) in &design.tasks {
        if task.srcs.is_empty() {
            return Err(CliError::Codegen(format!(
                "task `{task_name}` has an empty `srcs` manifest; \
                 re-run `tapa analyze` to regenerate it",
            )));
        }
        for dir in &task.include_dirs {
            if dir.is_empty() {
                continue; // "" is the tree root, always on the include path.
            }
            if dir.starts_with('/') || !dir.bytes().all(is_tcl_safe_path_byte) {
                return Err(CliError::Codegen(format!(
                    "task `{task_name}` include dir `{dir}` must be a \
                     tree-relative path over [A-Za-z0-9_/] only; it reaches the \
                     HLS TCL unquoted",
                )));
            }
        }
        for define in &task.defines {
            if define.is_empty()
                || !define
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(CliError::Codegen(format!(
                    "task `{task_name}` define `{define}` must be an identifier \
                     over [A-Za-z0-9_] only; it reaches the HLS TCL unquoted",
                )));
            }
        }
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

fn is_tcl_safe_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'/'
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
        task_with(srcs, &[], &[])
    }

    fn task_with(srcs: &[&str], include_dirs: &[&str], defines: &[&str]) -> Task {
        Task {
            level: TaskLevel::Lower,
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            include_dirs: include_dirs.iter().map(|s| (*s).to_string()).collect(),
            defines: defines.iter().map(|s| (*s).to_string()).collect(),
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
        assert_eq!(
            sources.srcs("Add"),
            [crate::util::utf8(
                dir.path()
                    .join(crate::tapacc::REWRITTEN_DIR)
                    .join("Add.cpp")
            )]
        );
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
    fn empty_srcs_manifest_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = design(&[("Add", task(&[]))]);
        let err = stage_hls_sources(dir.path(), &d).expect_err("empty must fail");
        assert!(err.to_string().contains("empty `srcs` manifest"), "{err}");
    }

    /// Include dirs and guard defines reach the HLS TCL's
    /// `add_files -cflags` string unquoted; the safe charset is
    /// asserted, anything else is a clear staging error.
    #[test]
    fn tcl_unsafe_manifest_entries_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        for (include_dirs, defines, culprit) in [
            (&["../escape"][..], &[][..], "../escape"),
            (&["/abs"][..], &[][..], "/abs"),
            (&["hyphen-dir"][..], &[][..], "hyphen-dir"),
            (
                &[""][..],
                &["TAPA_TASK_DEF_Bad-Key"][..],
                "TAPA_TASK_DEF_Bad-Key",
            ),
            (&[""][..], &[""][..], "must be an identifier"),
        ] {
            let d = design(&[("Add", task_with(&["Add.cpp"], include_dirs, defines))]);
            let err = stage_hls_sources(dir.path(), &d).expect_err(&format!(
                "include={include_dirs:?} defines={defines:?} must be rejected"
            ));
            assert!(
                err.to_string().contains(culprit),
                "expected `{culprit}` in: {err}"
            );
        }
        // The empty include dir (tree root) and well-formed entries pass.
        let d = design(&[(
            "Add",
            task_with(
                &["Add.cpp"],
                &["", "_external/ab12cd34"],
                &["TAPA_TASK_DEF_Add"],
            ),
        )]);
        stage_hls_sources(dir.path(), &d).expect("safe manifest must stage");
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
