//! Verbatim port of `tapa/common/paths.py::find_resource` and
//! `tapa/steps/analyze.py::find_clang_binary`.
//!
//! The Python implementation walks every parent of `__file__` and tries
//! each entry from `POTENTIAL_PATHS`, returning the first match. Bazel
//! runfiles support is intentionally out of scope — see the plan.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

use crate::error::{CliError, Result};

/// Mirror of `tapa/common/paths.py::POTENTIAL_PATHS`. Order is preserved so
/// a higher-priority match wins (matches the Python tuple-iteration order).
pub static POTENTIAL_PATHS: &[(&str, &[&str])] = &[
    ("fpga-runtime-include", &["fpga-runtime", "usr/include"]),
    (
        "fpga-runtime-lib",
        &["fpga-runtime/cargo", "fpga-runtime", "usr/lib"],
    ),
    ("tapa-cpp-binary", &["tapa-cpp/tapa-cpp", "usr/bin/tapa-cpp"]),
    (
        "tapa-extra-runtime-include",
        &[
            "tapa-system-include/tapa-extra-runtime-include",
            "tapa-lib/extra-runtime-include",
            "usr/include",
        ],
    ),
    ("tapa-lib-include", &["tapa-lib", "usr/include"]),
    ("tapa-lib-lib", &["tapa-lib", "usr/lib"]),
    (
        "tapa-system-include",
        &[
            "tapa-system-include/tapa-system-include",
            "usr/share/tapa/system-include",
        ],
    ),
    ("tapacc-binary", &["tapacc/tapacc", "usr/bin/tapacc"]),
]
;

/// Override the search anchor for tests. Mirrors Python's
/// `Path(__file__).absolute().parents` walk: when the override is set, we
/// walk its parents instead of the binary's parents.
fn search_anchor() -> PathBuf {
    static OVERRIDE: OnceLock<Option<PathBuf>> = OnceLock::new();
    OVERRIDE
        .get_or_init(|| std::env::var_os("TAPA_CLI_SEARCH_ANCHOR").map(PathBuf::from))
        .clone()
        .unwrap_or_else(|| {
            std::env::current_exe()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| PathBuf::from("."))
        })
}

/// Resolve a `POTENTIAL_PATHS` key to an absolute path. Walks the parents
/// of [`search_anchor`] in order, trying each candidate suffix; the first
/// existing path wins.
pub fn find_resource(name: &str) -> Result<PathBuf> {
    find_resource_from(name, &search_anchor())
}

/// Same as [`find_resource`] but with an explicit anchor (used in tests).
pub fn find_resource_from(name: &str, anchor: &Path) -> Result<PathBuf> {
    let suffixes = POTENTIAL_PATHS
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
        .ok_or_else(|| {
            CliError::TapaccNotFound {
                name: name.to_string(),
                searched: format!("unknown resource key `{name}`"),
            }
        })?;

    let mut tried: Vec<String> = Vec::new();
    for suffix in suffixes {
        let mut cursor: Option<&Path> = Some(anchor);
        while let Some(parent) = cursor {
            let candidate = parent.join(suffix);
            if candidate.exists() {
                return Ok(candidate);
            }
            tried.push(candidate.display().to_string());
            cursor = parent.parent();
        }
    }
    Err(CliError::TapaccNotFound {
        name: name.to_string(),
        searched: tried.join(", "),
    })
}

/// Resolve a clang-family helper (`tapacc-binary`, `tapa-cpp-binary`).
/// Verifies the resolved file prints a parseable `--version`; otherwise
/// surfaces a typed error.
pub fn find_clang_binary(name: &str) -> Result<PathBuf> {
    let path = find_resource(name)?;
    verify_clang_version(&path)?;
    Ok(path
        .canonicalize()
        .unwrap_or(path))
}

/// Regex matching Python's `re.compile(R"version (\d+)(\.\d+)*")` from
/// `tapa/steps/analyze.py::find_clang_binary`. Requires the literal
/// `"version "` followed by at least one numeric segment so unparseable
/// `--version` output (e.g. plain "ok") fails fast.
fn version_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"version (\d+)(\.\d+)*").expect("regex compile"))
}

fn verify_clang_version(path: &Path) -> Result<()> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| CliError::TapaccNotExecutable {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(CliError::TapaccNotExecutable {
            path: path.to_path_buf(),
            reason: format!("`--version` exited {}", output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if version_regex().is_match(&stdout) {
        Ok(())
    } else {
        Err(CliError::TapaccNotExecutable {
            path: path.to_path_buf(),
            reason: format!(
                "`--version` output does not match `version (\\d+)(\\.\\d+)*`: {stdout}"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_via_first_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested/deep");
        fs::create_dir_all(&nested).unwrap();
        let target = dir.path().join("tapacc/tapacc");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"#!/bin/sh\nexit 0").unwrap();

        let resolved = find_resource_from("tapacc-binary", &nested).unwrap();
        assert_eq!(resolved, target);
    }

    #[test]
    fn falls_back_to_usr_layout() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        let target = dir.path().join("usr/bin/tapacc");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, b"#!/bin/sh\nexit 0").unwrap();

        let resolved = find_resource_from("tapacc-binary", &nested).unwrap();
        assert_eq!(resolved, target);
    }

    #[test]
    fn missing_resource_carries_searched_paths() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_resource_from("tapacc-binary", dir.path()).unwrap_err();
        if let CliError::TapaccNotFound { name, searched } = &err {
            assert_eq!(name, "tapacc-binary");
            assert!(
                searched.contains("tapacc"),
                "searched paths must mention the suffix; got `{searched}`",
            );
        } else {
            panic!("expected TapaccNotFound, got {err:?}");
        }
    }

    #[test]
    fn unknown_resource_key_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = find_resource_from("does-not-exist", dir.path()).unwrap_err();
        assert!(matches!(err, CliError::TapaccNotFound { .. }));
    }

    #[test]
    fn version_regex_matches_clang_output() {
        assert!(version_regex().is_match("clang version 18.1.0\n"));
        assert!(version_regex().is_match("Apple clang version 16.0.0 (clang-1600.0.26.6)"));
    }

    #[test]
    fn version_regex_rejects_unparseable_output() {
        // AC-4 negative test: a binary that prints `--version` text
        // without a parseable `version <num>` token must fail.
        assert!(!version_regex().is_match(""));
        assert!(!version_regex().is_match("hello world"));
        assert!(!version_regex().is_match("version foo"));
        assert!(!version_regex().is_match("version v18"));
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_binary_yields_typed_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("tapacc/tapacc");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        // Write a non-executable file (mode 0644).
        std::fs::write(&target, b"not a real binary").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let err = verify_clang_version(&target).unwrap_err();
        assert!(matches!(err, CliError::TapaccNotExecutable { .. }));
    }
}
