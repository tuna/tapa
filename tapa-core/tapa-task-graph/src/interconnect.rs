//! FIFO / interconnect definition types.

use serde::{Deserialize, Serialize};

/// Producer or consumer reference: `[task_name, instance_index]`.
///
/// In the JSON this is a two-element array `["TaskName", 0]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndpointRef(pub String, pub u32);

/// A FIFO / stream interconnect definition within an upper task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterconnectDefinition {
    /// FIFO depth (present for internal FIFOs).
    #[serde(default)]
    pub depth: Option<u32>,
    /// `[task_name, instance_idx]` of the consumer.
    #[serde(default)]
    pub consumed_by: Option<EndpointRef>,
    /// `[task_name, instance_idx]` of the producer.
    #[serde(default)]
    pub produced_by: Option<EndpointRef>,
}
