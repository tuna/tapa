//! `settings.json` read / write. The Python writer emits whatever dict
//! analyze / synth / pack stored, so the Rust shape is `IndexMap` of
//! `serde_json::Value`.

use std::fs::File;
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
    let reader = BufReader::new(File::open(&path)?);
    let settings: Settings = serde_json::from_reader(reader)?;
    Ok(settings)
}

pub fn store_settings(work_dir: &Path, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(work_dir)?;
    let path = path_in(work_dir);
    let mut writer = BufWriter::new(File::create(&path)?);
    let mut ser = serde_json::Serializer::with_formatter(&mut writer, PythonFormatter);
    serde::Serialize::serialize(settings, &mut ser)?;
    Ok(())
}

/// JSON formatter matching `json.dump(...)` defaults from `CPython` 3.7+:
/// `, ` between items, `: ` between key and value, no indentation.
///
/// Re-defined here (and not imported from `tapa_task_graph::design`) because
/// `serde_json::ser::Formatter` is not `pub` in that crate's API surface.
#[derive(Debug, Default)]
pub(crate) struct PythonFormatter;

impl serde_json::ser::Formatter for PythonFormatter {
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

    fn begin_object_value<W: io::Write + ?Sized>(
        &mut self,
        writer: &mut W,
    ) -> io::Result<()> {
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
    fn writer_uses_python_separators() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::new();
        s.insert("a".to_string(), json!(1));
        s.insert("b".to_string(), json!(2));
        store_settings(dir.path(), &s).unwrap();
        let raw = std::fs::read_to_string(path_in(dir.path())).unwrap();
        assert_eq!(raw, r#"{"a": 1, "b": 2}"#);
    }
}
