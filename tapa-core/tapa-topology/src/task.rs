//! Task type for the compilation model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tapa_task_graph::interconnect::EndpointRef;
use tapa_task_graph::port::ArgCategory;
use tapa_task_graph::task::TaskLevel;

use crate::instance::InstanceDesign;

/// A port in the topology model (same schema as graph.json ports).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDesign {
    /// Port category (typed enum matching Python semantics).
    pub cat: ArgCategory,
    /// Port name.
    pub name: String,
    /// C++ type string.
    #[serde(rename = "type")]
    pub ctype: String,
    /// Bit width.
    pub width: u32,
    /// Channel count for hierarchical memory ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chan_count: Option<u32>,
    /// Channel size for hierarchical memory ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chan_size: Option<u32>,
    /// Extra unknown port fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A FIFO / interconnect in the topology model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FifoDesign {
    /// FIFO depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Consumer endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_by: Option<EndpointRef>,
    /// Producer endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<EndpointRef>,
    /// Extra unknown fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// A single task definition in `design.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDesign {
    /// Task name (also present as the dict key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Task hierarchy level.
    pub level: TaskLevel,
    /// C++ source code.
    pub code: String,
    /// Synthesis target string.
    pub target: String,
    /// Whether this task is a floorplan slot.
    #[serde(default)]
    pub is_slot: bool,
    /// External ports.
    #[serde(default)]
    pub ports: Vec<PortDesign>,
    /// Child task instantiations (upper tasks only).
    ///
    /// Uses [`BTreeMap`] so iteration is alphabetical by task name,
    /// matching Python's `dict(sorted(tasks.items()))` behavior in
    /// `tapa/task.py::Task.__init__`. Slot-parameter aggregation and
    /// other iteration-order-sensitive traversals depend on this.
    #[serde(default)]
    pub tasks: BTreeMap<String, Vec<InstanceDesign>>,
    /// FIFO interconnects (upper tasks only).
    #[serde(default)]
    pub fifos: BTreeMap<String, FifoDesign>,
    /// RTL-enriched annotations (`self_area`, `total_area`, `clock_period`, etc.).
    /// Stored as a flat map for forward compatibility.
    #[serde(flatten)]
    pub annotations: BTreeMap<String, serde_json::Value>,
}
