//! Task definition types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::instance::TaskInstance;
use crate::interconnect::InterconnectDefinition;
use crate::port::Port;

/// Level of a task in the hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskLevel {
    Lower,
    Upper,
}

/// A single task definition from `graph.json["tasks"]`.
///
/// We intentionally do **not** `deny_unknown_fields` here because
/// `tapacc` emits additional metadata (e.g. `readable_name` from the
/// C++ visitor) that the native synth pipeline does not consume. The
/// graph round-trips through the typed schema (analyze → graph.json →
/// transforms), so silently keeping unknown fields would erase them;
/// instead, any field added by `tapacc` is parked here explicitly as
/// `extra` so the typed path can re-emit it byte-identical. Every
/// other field keeps the strict check at a higher level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDefinition {
    /// C++ source code for this task.
    pub code: String,
    /// Task level: `"lower"` (leaf) or `"upper"` (composite).
    pub level: TaskLevel,
    /// Synthesis target (e.g. `"hls"`, `"ignore"`).
    pub target: String,
    /// Vendor string (e.g. `"xilinx"`).
    #[serde(default)]
    pub vendor: String,
    /// External ports / interface definitions.
    #[serde(default)]
    pub ports: Vec<Port>,
    /// Child task instantiations (upper tasks only).
    /// Maps task definition name → list of instantiations.
    #[serde(default)]
    pub tasks: BTreeMap<String, Vec<TaskInstance>>,
    /// FIFO / interconnect definitions (upper tasks only).
    #[serde(default)]
    pub fifos: BTreeMap<String, InterconnectDefinition>,
    /// Any extra fields `tapacc` attached (e.g. `readable_name`). Kept
    /// so round-trips through the typed schema preserve them.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
