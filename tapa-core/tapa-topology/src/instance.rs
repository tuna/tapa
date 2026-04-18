//! Instance types for the topology model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tapa_task_graph::port::ArgCategory;

/// Argument connecting parent scope to child task port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgDesign {
    /// Resolved name in parent scope (port/FIFO name or literal).
    pub arg: String,
    /// Category (typed enum matching Python semantics).
    pub cat: ArgCategory,
    /// Extra unknown fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A single instantiation of a child task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceDesign {
    /// Arguments: maps child-port name → connection info.
    #[serde(default)]
    pub args: BTreeMap<String, ArgDesign>,
    /// Bulk-synchronous step (can be negative).
    #[serde(default)]
    pub step: i64,
    /// Extra unknown fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
