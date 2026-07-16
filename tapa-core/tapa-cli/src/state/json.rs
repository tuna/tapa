//! Shared atomic JSON write plumbing for work-directory state files.
//!
//! All three state files (`design.json`, `graph.json`, `settings.json`) share
//! the same on-disk format: compact JSON with `, ` between items, `: ` between
//! key and value, no indentation, and no trailing newline. This module holds
//! the single formatter and the single tempfile -> serialize -> rename dance so
//! the three `store_*` helpers stay byte-for-byte identical.

use std::io::{self, Write};
use std::path::Path;

use crate::error::Result;

/// Atomically write raw `bytes` to `<work_dir>/<file_name>`.
///
/// Writes into a `NamedTempFile` created in `work_dir`, then renames it into
/// place so readers never observe a partially written file. Callers that emit
/// JSON in a non-state format (e.g. spaceless `serde_json::to_vec`) use this
/// directly to keep their exact bytes while gaining atomicity.
pub fn write_bytes_atomic(work_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<()> {
    fs_err::create_dir_all(work_dir)?;
    let path = work_dir.join(file_name);
    let mut temp = tempfile::NamedTempFile::new_in(work_dir)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    fs_err::rename(temp.path(), &path)?;
    Ok(())
}

/// Atomically write `value` as compact JSON (`, `/`: ` spaced, no indent) to
/// `<work_dir>/<file_name>` via [`write_bytes_atomic`].
pub fn write_json_atomic<T: serde::Serialize>(
    work_dir: &Path,
    file_name: &str,
    value: &T,
) -> Result<()> {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, JsonFormatter);
    value.serialize(&mut ser)?;
    write_bytes_atomic(work_dir, file_name, &buf)
}

/// JSON formatter that uses `, ` between items, `: ` between key and value,
/// no indentation, and no trailing newline.
///
/// This is the canonical (and only) copy of the work-directory state
/// formatter; `tapa-ir` types serialize through it via
/// [`write_json_atomic`].
#[derive(Debug, Default)]
pub struct JsonFormatter;

impl serde_json::ser::Formatter for JsonFormatter {
    fn begin_array_value<W: io::Write + ?Sized>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W: io::Write + ?Sized>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W: io::Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }
}
