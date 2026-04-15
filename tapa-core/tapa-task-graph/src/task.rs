//! Task definition types.

use std::collections::HashMap;

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
    pub tasks: HashMap<String, Vec<TaskInstance>>,
    /// FIFO / interconnect definitions (upper tasks only).
    #[serde(default)]
    pub fifos: HashMap<String, InterconnectDefinition>,
}
