//! `settings.json` read / write. Steps may store heterogeneous values,
//! so the Rust shape is `IndexMap` of
//! `serde_json::Value`.

use std::io::{self, BufReader, BufWriter};
use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::error::{CliError, Result};

pub type Settings = IndexMap<String, Value>;

const FILE_NAME: &str = "settings.json";

pub fn path_in(work_dir: &Path) -> std::path::PathBuf {
    work_dir.join(FILE_NAME)
}

pub fn load_settings(work_dir: &Path) -> Result<Settings> {
    let path = path_in(work_dir);
    if !path.exists() {
        return Err(CliError::MissingState {
            name: FILE_NAME.to_string(),
            path,
        });
    }
    let reader = BufReader::new(fs_err::File::open(&path)?);
    let settings: Settings = serde_json::from_reader(reader)?;
    Ok(settings)
}

#[allow(
    clippy::semicolon_outside_block,
    reason = "scoping block for BufWriter drop"
)]
pub fn store_settings(work_dir: &Path, settings: &Settings) -> Result<()> {
    fs_err::create_dir_all(work_dir)?;
    let path = path_in(work_dir);
    let mut temp = tempfile::NamedTempFile::new_in(work_dir)?;
    {
        let mut writer = BufWriter::new(&mut temp);
        let mut ser = serde_json::Serializer::with_formatter(&mut writer, JsonFormatter);
        serde::Serialize::serialize(settings, &mut ser)?;
    }
    fs_err::rename(temp.path(), &path)?;
    Ok(())
}

/// JSON formatter that uses `, ` between items, `: ` between key and value,
/// no indentation, and no trailing newline.
///
/// Re-defined here (and not imported from `tapa_task_graph::design`) because
/// `serde_json::ser::Formatter` is not `pub` in that crate's API surface.
#[derive(Debug, Default)]
pub(crate) struct JsonFormatter;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_via_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::new();
        s.insert("target".to_string(), json!("xilinx-hls"));
        s.insert("part_num".to_string(), json!("xcvu37p"));
        s.insert("synthed".to_string(), json!(true));
        store_settings(dir.path(), &s).unwrap();
        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn missing_settings_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_settings(dir.path()).expect_err("must fail");
        assert!(matches!(err, CliError::MissingState { .. }));
    }

    #[test]
    fn writer_uses_spaced_separators() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::new();
        s.insert("a".to_string(), json!(1));
        s.insert("b".to_string(), json!(2));
        store_settings(dir.path(), &s).unwrap();
        let raw = fs_err::read_to_string(path_in(dir.path())).unwrap();
        assert_eq!(raw, r#"{"a": 1, "b": 2}"#);
    }
}
