//! `settings.json` read / write. Steps may store heterogeneous values,
//! so the Rust shape is `IndexMap` of
//! `serde_json::Value`.

use std::io::BufReader;
use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::error::{CliError, Result};
use crate::state::json::write_json_atomic;

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

pub fn store_settings(work_dir: &Path, settings: &Settings) -> Result<()> {
    write_json_atomic(work_dir, FILE_NAME, settings)
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
