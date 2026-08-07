//! Task definition types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::floorplan::Area;
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// This task's own area, as HLS reported it. Post-synthesis; `None`
    /// until a synthesis step annotates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_area: Option<Area>,
    /// This task's area including every instantiated descendant. Post-
    /// synthesis; `None` until out-of-context synthesis measures it, in
    /// which case consumers derive it from `self_area` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_area: Option<Area>,
}
