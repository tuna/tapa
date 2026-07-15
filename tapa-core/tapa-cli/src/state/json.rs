//! Shared atomic JSON write plumbing for work-directory state files.
//!
//! All three state files (`design.json`, `graph.json`, `settings.json`) share
//! the same on-disk format: compact JSON with `, ` between items, `: ` between
//! key and value, no indentation, and no trailing newline. This module holds
//! the single formatter and the single tempfile -> serialize -> rename dance so
//! the three `store_*` helpers stay byte-for-byte identical.

use std::io::{self, BufWriter};
use std::path::Path;

use crate::error::Result;

/// Atomically write `value` as compact JSON to `<work_dir>/<file_name>`.
///
/// Serializes with [`JsonFormatter`] into a `NamedTempFile` created in
/// `work_dir`, then renames it into place so readers never observe a partially
/// written file.
#[allow(
    clippy::semicolon_outside_block,
    reason = "scoping block for BufWriter drop"
)]
pub fn write_json_atomic<T: serde::Serialize>(
    work_dir: &Path,
    file_name: &str,
    value: &T,
) -> Result<()> {
    fs_err::create_dir_all(work_dir)?;
    let path = work_dir.join(file_name);
    let mut temp = tempfile::NamedTempFile::new_in(work_dir)?;
    {
        let mut writer = BufWriter::new(&mut temp);
        let mut ser = serde_json::Serializer::with_formatter(&mut writer, JsonFormatter);
        value.serialize(&mut ser)?;
    }
    fs_err::rename(temp.path(), &path)?;
    Ok(())
}

/// JSON formatter that uses `, ` between items, `: ` between key and value,
/// no indentation, and no trailing newline.
///
/// This is the canonical copy for CLI work-directory state files.
/// `tapa-task-graph` still carries an identical formatter behind
/// `Design::to_writer`; the two collapse into one when that schema crate is
/// folded into `tapa-ir`.
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
