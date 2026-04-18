//! Program type — top-level container for the compilation model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::task::TaskDesign;

/// Top-level program from `design.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Program {
    /// Name of the top-level task.
    pub top: String,
    /// Synthesis target (e.g., `"xilinx-hls"`).
    pub target: String,
    /// All task definitions, keyed by name.
    pub tasks: BTreeMap<String, TaskDesign>,
    /// Floorplanning region assignments for slot tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_task_name_to_fp_region: Option<BTreeMap<String, String>>,
    /// Extra unknown fields for forward compatibility.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
