//! Task definition types.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::instance::TaskInstance;
use crate::interconnect::InterconnectDefinition;
use crate::port::Port;
use crate::synth_target::SynthTarget;

/// Level of a task in the hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskLevel {
    Lower,
    Upper,
}

/// A single task in the unified task graph.
///
/// The tapacc output and the post-synthesis design share this one type:
/// analyze emits the structural fields, synth populates the post-synthesis
/// annotation block (`clock_period`, `self_area`, `total_area`) in place,
/// and report reads them back. The annotations are absent in tapacc output
/// and omitted from the wire form until populated.
#[allow(
    clippy::derive_partial_eq_without_eq,
    reason = "area dicts hold serde_json::Value, which is not Eq (Number may be f64)"
)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Task level: `"lower"` (leaf) or `"upper"` (composite).
    pub level: TaskLevel,
    /// C++ source code for this task.
    pub code: String,
    /// Human-readable task name emitted by `tapacc` (e.g. the demangled
    /// template specialization). Required: `tapacc` emits it unconditionally
    /// for every task, equal to the task name for non-template tasks.
    pub readable_name: String,
    /// Per-task synthesis policy (`"hls"` / `"ignore"`).
    pub synth: SynthTarget,
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
    /// Post-synthesis achieved clock-period estimate (seconds, stringified).
    /// Seeded empty by analyze, written by synth, read by report.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub clock_period: String,
    /// Per-task self area dict (resource → number). Post-synthesis.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub self_area: IndexMap<String, Value>,
    /// Per-task total area dict (self + descendants). Post-synthesis.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub total_area: IndexMap<String, Value>,
}
