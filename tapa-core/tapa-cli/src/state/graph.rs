//! `graph.json` read / write helpers.
//!
//! Uses `serde_json::Value` instead of the strict `tapa_task_graph::Graph`
//! type because the legacy Python `tapa.common.graph.Graph` class accepts
//! a richer schema than the tapacc-output flavor.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use serde_json::Value;

use crate::error::{CliError, Result};

const FILE_NAME: &str = "graph.json";

pub fn path_in(work_dir: &Path) -> std::path::PathBuf {
    work_dir.join(FILE_NAME)
}

pub fn load_graph(work_dir: &Path) -> Result<Value> {
    let path = path_in(work_dir);
    if !path.exists() {
        return Err(CliError::MissingState {
            name: FILE_NAME.to_string(),
            path,
        });
    }
    let reader = BufReader::new(File::open(&path)?);
    let value: Value = serde_json::from_reader(reader)?;
    Ok(value)
}

/// Persist the graph using compact JSON (no separator-with-space; matches
/// `json.dump` defaults — see [`crate::state::settings`] for the rationale).
pub fn store_graph(work_dir: &Path, graph: &Value) -> Result<()> {
    std::fs::create_dir_all(work_dir)?;
    let path = path_in(work_dir);
    let mut writer = BufWriter::new(File::create(&path)?);
    write_python_json(&mut writer, graph)?;
    Ok(())
}

/// Serialize `value` with the Python-style separators `, ` and `: `.
pub(crate) fn write_python_json<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    use serde::Serialize;
    let formatter = crate::state::settings::PythonFormatter;
    let mut ser = serde_json::Serializer::with_formatter(writer, formatter);
    value.serialize(&mut ser)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip_preserves_byte_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let g = json!({
            "cflags": ["-std=c++17"],
            "tasks": {"T": {"code": "void T() {}", "level": "lower"}},
            "top": "T",
        });
        store_graph(dir.path(), &g).unwrap();
        let loaded = load_graph(dir.path()).unwrap();
        assert_eq!(loaded, g);
    }
}
