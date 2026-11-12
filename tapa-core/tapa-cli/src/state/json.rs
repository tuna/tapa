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
/// Writes into a temporary file created in `work_dir`, then renames it into
/// place so readers never observe a partially written file.
pub fn write_bytes_atomic(work_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<()> {
    fs_err::create_dir_all(work_dir)?;
    let path = work_dir.join(file_name);
    let mut temp = staging_file_in(work_dir)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    fs_err::rename(temp.path(), &path)?;
    Ok(())
}

/// A temporary file in `dir` that is renamed into place as a work-dir file.
/// It gets the mode `File::create` would give it (`0o666` less the umask), not
/// `NamedTempFile`'s private `0o600`: work directories and the bitstreams
/// published into them are shared between users, CI jobs, and containers.
pub fn staging_file_in(dir: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o666));
    builder.tempfile_in(dir)
}

#[cfg(test)]
mod tests {
    /// A rename-into-place file must be as readable as one `File::create`
    /// writes in the same directory under the same umask.
    #[cfg(unix)]
    #[test]
    fn atomically_written_files_get_the_mode_file_create_gives() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        super::write_bytes_atomic(dir.path(), "tapa.json", b"{}").expect("write");
        fs_err::File::create(dir.path().join("plain")).expect("create");
        let mode = |name: &str| {
            fs_err::metadata(dir.path().join(name))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("tapa.json"), mode("plain"));
    }
}
