//! Shared atomic write plumbing for work-directory files.
//!
//! Used by the state file ([`super::work`]) and by the steps that emit their
//! own JSON side artifacts (`templates_info.json`). Each caller owns its
//! exact bytes and borrows only the tempfile -> rename dance from here.

use std::io::Write;
use std::path::Path;

use crate::error::Result;

/// Atomically write raw `bytes` to `<work_dir>/<file_name>`.
///
/// Writes into a `NamedTempFile` created in `work_dir`, then renames it into
/// place so readers never observe a partially written file.
pub fn write_bytes_atomic(work_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<()> {
    fs_err::create_dir_all(work_dir)?;
    let path = work_dir.join(file_name);
    let mut temp = tempfile::NamedTempFile::new_in(work_dir)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    fs_err::rename(temp.path(), &path)?;
    Ok(())
}
