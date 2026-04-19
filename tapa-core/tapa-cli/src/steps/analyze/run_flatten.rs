//! `tapa-cpp` (flatten) preprocessor invocation for `tapa analyze`.
//!
//! Splits one source file per `tapa-cpp` invocation, writing the
//! preprocessed result to `<work_dir>/flatten/flatten-<digest>-<basename>`
//! and returning the list of generated paths.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::error::{CliError, Result};

/// Run `tapa-cpp` once per input file and write the preprocessed source
/// to `<work_dir>/flatten/flatten-<digest>-<basename>`. When a
/// `clang-format` binary is on `PATH` (probed in the same order as
/// Python's `tapa.util.clang_format` — `clang-format-20` …
/// `clang-format-5`, then `clang-format`), the captured stdout is
/// pretty-printed before it lands on disk. The running byte counter
/// is gated on `quota_in_bytes`, mirroring
/// `--clang-format-quota-in-bytes`: once formatting *this* file would
/// push the cumulative formatted size past the quota, fall back to
/// the raw `tapa-cpp` bytes for that file (and every subsequent one).
pub(super) fn run_flatten(
    tapa_cpp: &Path,
    files: &[PathBuf],
    cflags: &[String],
    work_dir: &Path,
    quota_in_bytes: u64,
) -> Result<Vec<PathBuf>> {
    let flatten_dir = work_dir.join("flatten");
    fs::create_dir_all(&flatten_dir)?;
    let clang_format = find_clang_format();
    let mut formatted_total: u64 = 0;
    let mut out = Vec::<PathBuf>::with_capacity(files.len());
    for file in files {
        let abs = fs::canonicalize(file).unwrap_or_else(|_| file.clone());
        let digest = sha256_truncated_hex(abs.display().to_string().as_bytes());
        let basename = file.file_name().map_or_else(
            || "input.cpp".to_string(),
            |s| s.to_string_lossy().into_owned(),
        );
        let flatten_path = flatten_dir.join(format!("flatten-{digest}-{basename}"));

        let mut cmd = Command::new(tapa_cpp);
        cmd.args([
            "-x",
            "c++",
            "-E",
            "-CC",
            "-P",
            "-fkeep-system-includes",
            "-D__SYNTHESIS__",
            "-DAESL_SYN",
            "-DAP_AUTOCC",
            "-DTAPA_TARGET_DEVICE_",
            "-DTAPA_TARGET_STUB_",
        ]);
        for flag in cflags {
            cmd.arg(flag);
        }
        cmd.arg(file);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());
        let output = cmd.output().map_err(|e| CliError::TapaccNotExecutable {
            path: tapa_cpp.to_path_buf(),
            reason: e.to_string(),
        })?;
        if !output.status.success() {
            return Err(CliError::TapaccFailed {
                code: output.status.code().unwrap_or(-1),
                stderr: format!("tapa-cpp on {}", file.display()),
            });
        }
        let bytes = match clang_format.as_deref() {
            Some(fmt) => {
                let new_total = formatted_total.saturating_add(output.stdout.len() as u64);
                if new_total > quota_in_bytes {
                    output.stdout
                } else {
                    formatted_total = new_total;
                    run_clang_format(fmt, &output.stdout)?
                }
            }
            None => output.stdout,
        };
        fs::write(&flatten_path, &bytes)?;
        out.push(flatten_path);
    }
    Ok(out)
}

/// Probe `PATH` for the `clang-format` binary the same way
/// `tapa.util.clang_format` does: try `clang-format-{20..5}` from
/// newest to oldest, fall back to bare `clang-format`. Returns
/// `None` when nothing is installed (Python returns the input
/// unchanged in that case; this matches).
fn find_clang_format() -> Option<PathBuf> {
    use std::sync::OnceLock;
    static CACHED: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            for v in (5u32..=20).rev() {
                if let Some(p) = which_in_path(&format!("clang-format-{v}")) {
                    return Some(p);
                }
            }
            which_in_path("clang-format")
        })
        .clone()
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Pipe `code` through `clang-format`, returning the formatted bytes.
/// Errors propagate as `CliError::Io` so an unexpected formatter
/// failure surfaces (Python's helper would also raise).
fn run_clang_format(clang_format: &Path, code: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(clang_format)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .expect("clang-format stdin was piped")
        .write_all(code)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(CliError::TapaccFailed {
            code: out.status.code().unwrap_or(-1),
            stderr: format!("clang-format exited {:?}", out.status.code()),
        });
    }
    Ok(out.stdout)
}

/// Truncate a SHA-256 digest to the first 8 hex characters, matching
/// Python's `hashlib.sha256(...).hexdigest()[:8]`.
pub(super) fn sha256_truncated_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        let _ = write!(s, "{byte:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_truncated_matches_python_eight_hex_chars() {
        // Python: hashlib.sha256(b"foo").hexdigest()[:8] == "2c26b46b"
        assert_eq!(sha256_truncated_hex(b"foo"), "2c26b46b");
    }
}
