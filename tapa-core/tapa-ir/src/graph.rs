//! Root task-graph container.
//!
//! `TaskGraph` is the single root type for both the `tapacc` output and the
//! post-synthesis design model; it is persisted inside the work dir's
//! `tapa.json`, which `tapa pack` copies verbatim into the `.zip` archive for
//! `frt-cosim` to read back.
//! Field declaration order is stable so serializing re-emits keys
//! deterministically; `tasks` uses [`BTreeMap`] so keys come out
//! alphabetically, matching the sorted order `tapa analyze` writes.

use std::collections::BTreeMap;
use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::error::ParseError;
use crate::target::Target;
use crate::task::Task;

/// Root of the unified task graph.
#[allow(
    clippy::derive_partial_eq_without_eq,
    reason = "transitively holds serde_json::Value through Task"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskGraph {
    /// Producer schema version, stamped by `tapacc`. Absent means a
    /// pre-versioning producer, accepted as version 1. A value above
    /// [`SCHEMA_VERSION`] is rejected with a clear regenerate message
    /// instead of a field-level misparse.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Name of the top-level task.
    pub top: String,
    /// Compilation flow target, e.g. [`Target::XilinxVitis`]. Serializes as
    /// the wire string `"xilinx-vitis"` / `"xilinx-hls"`.
    pub target: Target,
    /// C++ compiler flags.
    #[serde(default)]
    pub cflags: Vec<String>,
    /// Task definitions keyed by task name.
    pub tasks: BTreeMap<String, Task>,
}

/// The task-graph schema version this crate reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

const fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl TaskGraph {
    /// Parse a task-graph payload with field-path error diagnostics.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let de = &mut serde_json::Deserializer::from_str(json);
        let graph: Self = serde_path_to_error::deserialize(de).map_err(|e| ParseError::Schema {
            path: e.path().to_string(),
            message: e.inner().to_string(),
        })?;
        if graph.schema_version > SCHEMA_VERSION {
            return Err(ParseError::UnsupportedSchemaVersion {
                found: graph.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(graph)
    }

    /// Parse from any reader.
    pub fn from_reader<R: Read>(mut reader: R) -> Result<Self, ParseError> {
        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        Self::from_json(&buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_schema_version_defaults_to_current() {
        let g =
            TaskGraph::from_json(r#"{"top": "T", "target": "xilinx-hls", "tasks": {}}"#).unwrap();
        assert_eq!(g.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn newer_schema_version_is_rejected_with_regenerate_message() {
        let err = TaskGraph::from_json(
            r#"{"schema_version": 999, "top": "T", "target": "xilinx-hls", "tasks": {}}"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("999") && msg.contains("regenerate"), "{msg}");
    }

    #[test]
    fn parses_named_child_instances() {
        TaskGraph::from_json(
            r#"{
              "cflags": [],
              "top": "Top",
              "target": "xilinx-hls",
              "tasks": {
                "Leaf": {
                    "readable_name": "Leaf",
                  "code": "void Leaf() {}",
                  "level": "lower",
                  "synth": "hls",
                  "ports": [],
                  "tasks": {},
                  "fifos": {}
                },
                "Top": {
                    "readable_name": "Top",
                  "code": "void Top() {}",
                  "level": "upper",
                  "synth": "hls",
                  "ports": [],
                  "tasks": {
                    "Leaf": [
                      {"name": "Leaf_0", "args": {}, "step": 0}
                    ]
                  },
                  "fifos": {}
                }
              }
            }"#,
        )
        .expect("graph parses");
    }

    #[test]
    fn missing_root_target_rejected() {
        // The flow target is required at the root now that it is the single
        // home for the vendor flow (analyze injects it before parsing).
        let err = TaskGraph::from_json(
            r#"{"cflags":[],"top":"T","tasks":{"T":{"code":"","level":"upper","synth":"hls","readable_name":"T","ports":[],"tasks":{},"fifos":{}}}}"#,
        )
        .expect_err("missing root `target` must fail");
        assert!(
            err.to_string().contains("target"),
            "error must point at the missing target field; got {err}",
        );
    }
}
