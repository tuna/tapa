//! Shared wire format for simulation setup and live DPI attachment.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct BufferEntry {
    #[serde(default)]
    pub base_addr: u64,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamEntry {
    /// Payload bytes including the EoT/TLAST byte.
    pub dpi_width_bytes: usize,
    pub path: String,
}

/// Ordered maps and fields preserve the setup writer's JSON ordering.
#[derive(Debug, Serialize, Deserialize)]
pub struct DpiConfig {
    pub buffers: BTreeMap<String, BufferEntry>,
    pub streams: BTreeMap<String, StreamEntry>,
}
