//! `tapa-cpp` (flatten) preprocessor invocation for `tapa analyze`.
//!
//! Splits one source file per `tapa-cpp` invocation, writing the
//! preprocessed result to `<work_dir>/flatten/flatten-<digest>-<basename>`
//! and returning the list of generated paths.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::error::{CliError, Result};

/// Run `tapa-cpp` once per input file and write the preprocessed source
/// to `<work_dir>/flatten/flatten-<digest>-<basename>`.
pub(super) fn run_flatten(
    tapa_cpp: &Path,
    files: &[PathBuf],
    cflags: &[String],
    work_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let flatten_dir = work_dir.join("flatten");
    fs::create_dir_all(&flatten_dir)?;
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
        fs::write(&flatten_path, &output.stdout)?;
        out.push(flatten_path);
    }
    Ok(out)
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
