//! `design.json` topology bridge — typed mirror of the dict produced by
//! `tapa/steps/common.py::store_design`.
//!
//! Field declaration order matches Python's literal dict construction so
//! [`Design::to_writer`] re-emits keys in the same order Python writes them.

use std::io::{self, Read, Write};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ParseError;
use crate::port::Port;

/// Per-task topology dict. Mirrors `tapa/task.py::Task.to_topology_dict`.
#[allow(
    clippy::derive_partial_eq_without_eq,
    reason = "fields hold serde_json::Value, which is not Eq (Number may be f64)"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskTopology {
    pub name: String,
    /// `"upper"` or `"lower"`.
    pub level: String,
    pub code: String,
    #[serde(default)]
    pub ports: Vec<Port>,
    /// Child task instantiations; `{}` for leaf tasks, populated for upper.
    #[serde(default)]
    pub tasks: IndexMap<String, Value>,
    /// FIFO definitions; `{}` for leaf tasks.
    #[serde(default)]
    pub fifos: IndexMap<String, Value>,
    /// `target_type` from the Python `Task` constructor; may be absent.
    pub target: Option<String>,
    pub is_slot: bool,
    /// Per-task self area dict (resource → number).
    #[serde(default)]
    pub self_area: IndexMap<String, Value>,
    /// Per-task total area dict (self + descendants).
    #[serde(default)]
    pub total_area: IndexMap<String, Value>,
    /// Stringified clock period (Python writes `str(decimal.Decimal(...))`).
    pub clock_period: String,
}

/// Root `design.json` payload. Mirrors `tapa/steps/common.py::store_design`.
#[allow(
    clippy::derive_partial_eq_without_eq,
    reason = "transitively holds serde_json::Value through TaskTopology"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Design {
    /// Top-level task name.
    pub top: String,
    /// Target flow string, e.g. `"xilinx-vitis"`.
    pub target: String,
    /// Tasks keyed by name. Insertion order matches Python's topological
    /// sort so a Rust → Rust round-trip preserves byte-equality.
    pub tasks: IndexMap<String, TaskTopology>,
    /// Floorplan slot → region mapping. Python writes `null` when unset.
    pub slot_task_name_to_fp_region: Option<IndexMap<String, String>>,
}

impl Design {
    /// Parse from a JSON string with field-path error diagnostics.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let de = &mut serde_json::Deserializer::from_str(json);
        serde_path_to_error::deserialize(de).map_err(|e| ParseError::Schema {
            path: e.path().to_string(),
            message: e.inner().to_string(),
        })
    }

    /// Parse from any reader (the `design.json` file handle).
    pub fn from_reader<R: Read>(mut reader: R) -> Result<Self, ParseError> {
        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        Self::from_json(&buf)
    }

    /// Serialize using the Python-compatible compact formatter.
    ///
    /// Matches `json.dump(..., default=...)` byte-for-byte: separators
    /// `", "` and `": "`, no indentation, no trailing newline.
    pub fn to_json(&self) -> Result<String, ParseError> {
        let mut buf = Vec::new();
        self.to_writer(&mut buf)?;
        String::from_utf8(buf).map_err(|e| ParseError::Schema {
            path: "<root>".to_string(),
            message: e.to_string(),
        })
    }

    /// Serialize to any writer.
    pub fn to_writer<W: Write>(&self, writer: &mut W) -> Result<(), ParseError> {
        let mut serializer =
            serde_json::Serializer::with_formatter(writer, PythonFormatter);
        self.serialize(&mut serializer)?;
        Ok(())
    }
}

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> Self {
        Self::Schema {
            path: "<io>".to_string(),
            message: e.to_string(),
        }
    }
}

/// JSON formatter matching `json.dump(...)` defaults from `CPython` 3.7+:
/// `, ` between items, `: ` between key and value, no indentation.
#[derive(Debug, Default)]
struct PythonFormatter;

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

    /// A minimal Python-shaped `design.json` payload for round-trip tests.
    /// Constructed verbatim like `json.dump({...}, fp)` would emit.
    fn sample_design_json() -> String {
        // Note: separators ", " and ": " mirror CPython json defaults.
        r#"{"top": "VecAdd", "target": "xilinx-vitis", "tasks": {"Add": {"name": "Add", "level": "lower", "code": "void Add() {}", "ports": [{"cat": "istream", "name": "a", "type": "float", "width": 32}], "tasks": {}, "fifos": {}, "target": "hls", "is_slot": false, "self_area": {}, "total_area": {}, "clock_period": "0"}, "VecAdd": {"name": "VecAdd", "level": "upper", "code": "void VecAdd() {}", "ports": [], "tasks": {"Add": [{"args": {"a": {"arg": "a_q", "cat": "istream"}}, "step": 0}]}, "fifos": {"a_q": {"depth": 2, "produced_by": ["A", 0], "consumed_by": ["Add", 0]}}, "target": "hls", "is_slot": false, "self_area": {}, "total_area": {}, "clock_period": "3.33"}}, "slot_task_name_to_fp_region": null}"#
            .to_string()
    }

    #[test]
    fn round_trip_byte_equal() {
        let json = sample_design_json();
        let design = Design::from_json(&json).expect("parse design.json");
        let emitted = design.to_json().expect("serialize design.json");
        assert_eq!(
            emitted, json,
            "Rust round-trip must preserve Python's byte sequence",
        );
    }

    #[test]
    fn task_order_preserved() {
        let json = sample_design_json();
        let design = Design::from_json(&json).expect("parse");
        let names: Vec<&String> = design.tasks.keys().collect();
        assert_eq!(
            names,
            vec![&"Add".to_string(), &"VecAdd".to_string()],
            "topological insertion order must survive parse + emit",
        );
    }

    #[test]
    fn missing_top_is_typed_error() {
        let json = r#"{"target": "xilinx-hls", "tasks": {}, "slot_task_name_to_fp_region": null}"#;
        let err = Design::from_json(json).expect_err("missing `top` must fail");
        let message = err.to_string();
        assert!(
            message.contains("top"),
            "error must point at the missing field; got {message}",
        );
    }

    #[test]
    fn unknown_task_topology_field_rejected() {
        let json = r#"{"top": "T", "target": "xilinx-hls", "tasks": {"T": {"name": "T", "level": "lower", "code": "", "ports": [], "tasks": {}, "fifos": {}, "target": null, "is_slot": false, "self_area": {}, "total_area": {}, "clock_period": "0", "bogus_field": 1}}, "slot_task_name_to_fp_region": null}"#;
        let err = Design::from_json(json)
            .expect_err("unknown task-topology field must fail");
        let message = err.to_string();
        assert!(
            message.contains("bogus_field") || message.contains("unknown field"),
            "error must mention the offending field; got {message}",
        );
        assert!(
            message.contains("tasks.T") || message.contains('T'),
            "error must include a path pointer; got {message}",
        );
    }

    #[test]
    fn slot_mapping_round_trips() {
        let json = r#"{"top": "T", "target": "xilinx-hls", "tasks": {"T": {"name": "T", "level": "lower", "code": "", "ports": [], "tasks": {}, "fifos": {}, "target": "hls", "is_slot": true, "self_area": {}, "total_area": {}, "clock_period": "0"}}, "slot_task_name_to_fp_region": {"T_slot": "SLR0_x0y0"}}"#;
        let design = Design::from_json(json).expect("parse");
        assert_eq!(
            design
                .slot_task_name_to_fp_region
                .as_ref()
                .and_then(|m| m.get("T_slot").map(String::as_str)),
            Some("SLR0_x0y0"),
        );
        assert_eq!(design.to_json().unwrap(), json);
    }

    #[test]
    fn null_target_round_trips() {
        let json = r#"{"top": "T", "target": "xilinx-hls", "tasks": {"T": {"name": "T", "level": "lower", "code": "", "ports": [], "tasks": {}, "fifos": {}, "target": null, "is_slot": false, "self_area": {}, "total_area": {}, "clock_period": "0"}}, "slot_task_name_to_fp_region": null}"#;
        let design = Design::from_json(json).expect("parse");
        assert!(design.tasks["T"].target.is_none());
        assert_eq!(design.to_json().unwrap(), json);
    }

    #[test]
    fn from_reader_works() {
        let json = sample_design_json();
        let design = Design::from_reader(json.as_bytes()).expect("from_reader");
        assert_eq!(design.top, "VecAdd");
        assert_eq!(design.target, "xilinx-vitis");
    }
}
