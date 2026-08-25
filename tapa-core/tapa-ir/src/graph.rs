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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskGraph {
    /// Producer schema version, stamped by `tapacc`. Absent means a
    /// pre-versioning (v1) producer, whose invoke-site constants this
    /// schema would misread as wire names, so both directions of mismatch
    /// are rejected with a clear regenerate message instead of a
    /// field-level misparse.
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
pub const SCHEMA_VERSION: u32 = 2;

/// A pre-versioning producer's graph is version 1 by definition.
const fn default_schema_version() -> u32 {
    1
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
        if graph.schema_version < SCHEMA_VERSION {
            return Err(ParseError::OutdatedSchemaVersion {
                found: graph.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        graph.validate_literals()?;
        Ok(graph)
    }

    /// Reject invalid invoke-site constants at the boundary, so a malformed
    /// literal fails here instead of rendering invalid Verilog downstream.
    fn validate_literals(&self) -> Result<(), ParseError> {
        for (task_name, task) in &self.tasks {
            for (child_name, instances) in &task.tasks {
                for instance in instances {
                    for (port_name, arg) in &instance.args {
                        if let crate::instance::ArgSource::Literal(value) = &arg.arg {
                            let path =
                                format!("tasks.{task_name}.tasks.{child_name}.args.{port_name}");
                            if !value.is_valid() {
                                return Err(ParseError::Schema {
                                    path,
                                    message: format!(
                                        "invalid constant {value}: the value must fit \
                                         a width of 1..=64 bits"
                                    ),
                                });
                            }
                            // Only a scalar port can take a constant. Every
                            // stream/mmap consumer resolves args by name and
                            // would silently skip the binding, leaving a
                            // child port unconnected.
                            if arg.cat != crate::port::ArgCategory::Scalar {
                                return Err(ParseError::Schema {
                                    path,
                                    message: format!(
                                        "constant {value} bound to a `{}` port; only \
                                         scalar ports take constants",
                                        arg.cat.as_str()
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(())
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
    fn missing_schema_version_is_rejected_as_pre_versioning() {
        // A pre-versioning graph's invoke constants are strings that this
        // schema would silently read as wire names; refuse it like the
        // WorkState v4 bump refuses stale work states.
        let err = TaskGraph::from_json(r#"{"top": "T", "target": "xilinx-hls", "tasks": {}}"#)
            .expect_err("pre-versioning graph must be rejected");
        assert!(
            matches!(err, ParseError::OutdatedSchemaVersion { found: 1, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("regenerate"), "{err}");
    }

    #[test]
    fn invalid_invoke_constant_is_rejected_at_the_boundary() {
        let payload = r#"{
            "schema_version": 2, "top": "T", "target": "xilinx-hls", "cflags": [],
            "tasks": {
                "T": {"level": "upper", "code": "", "readable_name": "T", "synth": "hls",
                    "ports": [], "fifos": {},
                    "tasks": {"A": [{"args": {
                        "n": {"arg": {"width": 8, "value": 300}, "cat": "scalar"}
                    }, "step": 0}]}},
                "A": {"level": "lower", "code": "", "readable_name": "A", "synth": "hls",
                    "ports": []}
            }
        }"#;
        let err = TaskGraph::from_json(payload).expect_err("8'd300 must not parse");
        assert!(err.to_string().contains("invalid constant"), "{err}");
    }

    #[test]
    fn current_schema_version_is_accepted() {
        let g = TaskGraph::from_json(
            r#"{"schema_version": 2, "top": "T", "target": "xilinx-hls", "tasks": {}}"#,
        )
        .unwrap();
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
              "schema_version": 2,
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
            r#"{"schema_version":2,"cflags":[],"top":"T","tasks":{"T":{"code":"","level":"upper","synth":"hls","readable_name":"T","ports":[],"tasks":{},"fifos":{}}}}"#,
        )
        .expect_err("missing root `target` must fail");
        assert!(
            err.to_string().contains("target"),
            "error must point at the missing target field; got {err}",
        );
    }
}
