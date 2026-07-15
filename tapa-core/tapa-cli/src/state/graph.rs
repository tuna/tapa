//! `graph.json` read / write helpers.
//!
//! Uses `serde_json::Value` instead of the strict `tapa_task_graph::Graph`
//! type because downstream steps accept a richer schema than the
//! tapacc-output flavor.

use std::io::BufReader;
use std::path::Path;

use serde_json::Value;

use crate::error::{CliError, Result};
use crate::state::json::write_json_atomic;

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
    let reader = BufReader::new(fs_err::File::open(&path)?);
    let value: Value = serde_json::from_reader(reader)?;
    Ok(value)
}

/// Persist the graph using the shared spaced compact formatter.
pub fn store_graph(work_dir: &Path, graph: &Value) -> Result<()> {
    write_json_atomic(work_dir, FILE_NAME, graph)
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
