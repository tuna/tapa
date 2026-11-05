//! Manifest staging for per-task HLS C++ sources.
//!
//! Per-task HLS C++ sources come from the task-graph manifest: `srcs`
//! paths relative to the work dir's `rewritten/` tree, which `tapa
//! analyze` (via `tapacc -emit-dir`) writes. This module is the one
//! place that verifies those files exist and resolves them to absolute
//! paths, for every synth consumer — the HLS jobs and the post-synth
//! utilization pass. The tree's ownership manifest additionally provides
//! the complete freshness set, including mirrored headers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use camino::Utf8PathBuf;
use tapa_ir::{Design, TaskLevel};

use crate::error::{CliError, Result};

/// Argument names that Vitis HLS treats as reserved keywords. Using one
/// of these as a port name produces inconsistent AXI/interface naming
/// downstream, so reject up front.
const RESERVED_PORT_NAMES: &[&str] = &[
    "begin", "end", "in", "input", "out", "output", "reg", "wire",
];
pub(super) const TREE_MANIFEST_NAME: &str = ".tapa-tree-manifest";
pub(super) const TREE_MANIFEST_HEADER: &str = "tapa-tree-ownership-v1\n";

/// Every task's rewritten sources, resolved absolute against the work
/// dir's `rewritten/` tree. Built once by [`stage_hls_sources`] before
/// any HLS job runs, so missing source or ownership metadata fails the
/// whole step up front instead of one parallel worker deep into synthesis.
#[derive(Debug)]
pub struct TaskSources {
    srcs: BTreeMap<String, Vec<Utf8PathBuf>>,
    freshness_deps: Vec<Utf8PathBuf>,
}

impl TaskSources {
    /// All resolved HLS translation units of `task_name`, non-empty for
    /// any task that passed staging.
    pub fn srcs(&self, task_name: &str) -> &[Utf8PathBuf] {
        match self.srcs.get(task_name) {
            Some(srcs) => srcs,
            None => &[],
        }
    }

    /// Every TAPA-owned file in the rewritten tree, plus the ownership
    /// manifest itself. This set is shared by all tasks: the manifest
    /// covers their common translation units and all mirrored headers.
    pub fn freshness_deps(&self) -> &[Utf8PathBuf] {
        &self.freshness_deps
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
    let (owned, freshness_deps) = stage_freshness_deps(&tree)?;
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
            if !is_tree_relative(src) {
                return Err(CliError::Codegen(format!(
                    "task `{task_name}` src `{src}` must be a path inside \
                     the rewritten tree, not an absolute or parent-escaping one",
                )));
            }
            if !owned.contains(src) {
                return Err(CliError::Codegen(format!(
                    "task `{task_name}` src `{src}` is not listed in the \
                     rewritten-tree ownership manifest; re-run `tapa analyze` \
                     to rebuild the rewritten tree",
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
    for key in &owned {
        if !tree.join(key).is_file() {
            return Err(invalid_tree_manifest(
                &tree.join(TREE_MANIFEST_NAME),
                format!("owned file `{key}` does not exist under {}", tree.display()),
            ));
        }
    }
    Ok(TaskSources {
        srcs: staged,
        freshness_deps,
    })
}

fn stage_freshness_deps(tree: &Path) -> Result<(BTreeSet<String>, Vec<Utf8PathBuf>)> {
    let manifest = tree.join(TREE_MANIFEST_NAME);
    let contents = fs::read_to_string(&manifest).map_err(|error| {
        CliError::Codegen(format!(
            "cannot read rewritten-tree ownership manifest `{}`: {error}; \
             re-run `tapa analyze` to rebuild the rewritten tree",
            manifest.display(),
        ))
    })?;
    let entries = contents
        .strip_prefix(TREE_MANIFEST_HEADER)
        .ok_or_else(|| invalid_tree_manifest(&manifest, "unrecognized format"))?;
    if !entries.is_empty() && !entries.ends_with('\n') {
        return Err(invalid_tree_manifest(
            &manifest,
            "the final entry is not newline-terminated",
        ));
    }

    let mut owned = BTreeSet::new();
    let mut deps = Vec::new();
    for key in entries.split_terminator('\n') {
        if !is_tree_relative(key) {
            return Err(invalid_tree_manifest(
                &manifest,
                format!("entry `{key}` is not a relative file key inside the tree"),
            ));
        }
        if !owned.insert(key.to_string()) {
            return Err(invalid_tree_manifest(
                &manifest,
                format!("duplicate entry `{key}`"),
            ));
        }
        deps.push(crate::util::utf8(tree.join(key)));
    }
    // Rewriting this file records both additions and removals, including
    // removals whose former paths are absent from the current entry set.
    deps.push(crate::util::utf8(manifest));
    Ok((owned, deps))
}

fn invalid_tree_manifest(manifest: &Path, reason: impl std::fmt::Display) -> CliError {
    CliError::Codegen(format!(
        "invalid rewritten-tree ownership manifest `{}`: {reason}; \
         re-run `tapa analyze` to rebuild the rewritten tree",
        manifest.display(),
    ))
}

fn is_tree_relative(path: &str) -> bool {
    !path.is_empty()
        && !Path::new(path).is_absolute()
        && !path.split(['/', '\\']).any(|component| component == "..")
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
            if RESERVED_PORT_NAMES.contains(&port.name.as_str()) {
                return Err(CliError::InvalidArg(format!(
                    "task `{task_name}` argument `{}` is a reserved keyword \
                     ({RESERVED_PORT_NAMES:?}); rename it before running synth — \
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
    use std::fs;

    use tapa_ir::port::ArgCategory;
    use tapa_ir::Task;

    use crate::steps::synth::fixtures;

    fn task(srcs: &[&str]) -> Task {
        task_with(srcs, &[], &[])
    }

    fn task_with(srcs: &[&str], include_dirs: &[&str], defines: &[&str]) -> Task {
        Task {
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            include_dirs: include_dirs.iter().map(|s| (*s).to_string()).collect(),
            defines: defines.iter().map(|s| (*s).to_string()).collect(),
            ..crate::testutil::task("Add")
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
        let src = format!("{name}.cpp");
        fs::write(tree.join(&src), "int main(){}\n").expect("write src");
        fixtures::write_manifest(dir, &[&src]);
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
        fixtures::write_manifest(dir.path(), &["Add.cpp"]);
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
        write_src(dir.path(), "Add");
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
    fn freshness_deps_cover_owned_headers_and_manifest_but_srcs_stay_tus() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        let tree = dir.path().join(crate::tapacc::REWRITTEN_DIR);
        fs::write(tree.join("Add.h"), "#define VALUE 1\n").expect("write header");
        fixtures::write_manifest(dir.path(), &["Add.cpp", "Add.h"]);

        let d = design(&[("Add", task(&["Add.cpp"]))]);
        let sources = stage_hls_sources(dir.path(), &d).expect("stage");
        assert_eq!(
            sources.srcs("Add"),
            [crate::util::utf8(tree.join("Add.cpp"))]
        );
        assert_eq!(
            sources.freshness_deps(),
            [
                crate::util::utf8(tree.join("Add.cpp")),
                crate::util::utf8(tree.join("Add.h")),
                crate::util::utf8(tree.join(TREE_MANIFEST_NAME)),
            ]
        );
    }

    #[test]
    fn missing_or_malformed_ownership_manifest_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        let d = design(&[("Add", task(&["Add.cpp"]))]);
        let manifest = dir
            .path()
            .join(crate::tapacc::REWRITTEN_DIR)
            .join(TREE_MANIFEST_NAME);

        fs::remove_file(&manifest).expect("remove manifest");
        let err = stage_hls_sources(dir.path(), &d).expect_err("missing manifest must fail");
        assert!(err.to_string().contains("ownership manifest"), "{err}");

        for malformed in [
            "not-a-tapa-manifest\n",
            "tapa-tree-ownership-v1\nAdd.cpp",
            "tapa-tree-ownership-v1\n\n",
            "tapa-tree-ownership-v1\nAdd.cpp\nAdd.cpp\n",
        ] {
            fs::write(&manifest, malformed).expect("write malformed manifest");
            let err = stage_hls_sources(dir.path(), &d).expect_err("malformed must fail");
            assert!(
                err.to_string()
                    .contains("invalid rewritten-tree ownership manifest"),
                "{err}"
            );
        }
    }

    #[test]
    fn ownership_manifest_paths_and_src_membership_are_validated() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Add");
        let d = design(&[("Add", task(&["Add.cpp"]))]);
        for bad in ["../outside.h", "/outside.h", ""] {
            fixtures::write_manifest(dir.path(), &[bad]);
            let err = stage_hls_sources(dir.path(), &d).expect_err("bad path must fail");
            assert!(err.to_string().contains("relative file key"), "{err}");
        }

        fixtures::write_manifest(dir.path(), &[]);
        let err = stage_hls_sources(dir.path(), &d).expect_err("unlisted src must fail");
        assert!(err.to_string().contains("not listed"), "{err}");
    }

    #[test]
    fn rejects_reserved_upper_port_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_src(dir.path(), "Top");
        let mut top = task(&["Top.cpp"]);
        top.level = TaskLevel::Upper;
        top.ports = vec![crate::testutil::port("in", ArgCategory::Mmap, "int*", 32)];
        let d = design(&[("Top", top)]);
        let err = stage_hls_sources(dir.path(), &d).expect_err("must reject reserved name");
        assert!(
            matches!(err, crate::error::CliError::InvalidArg(ref m)
                if m.contains("reserved keyword") && m.contains("`in`")),
            "expected reserved-keyword diagnostic: {err:?}"
        );
    }
}
