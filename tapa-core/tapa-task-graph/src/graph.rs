//! Root graph container.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ParseError;
use crate::task::TaskDefinition;

/// Root of the tapacc JSON output (`graph.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    /// C++ compiler flags.
    pub cflags: Vec<String>,
    /// Task definitions keyed by task name.
    pub tasks: BTreeMap<String, TaskDefinition>,
    /// Name of the top-level task.
    pub top: String,
}

impl Graph {
    /// Parse a `graph.json` payload with field-path error diagnostics.
    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        let de = &mut serde_json::Deserializer::from_str(json);
        serde_path_to_error::deserialize(de).map_err(|e| ParseError::Schema {
            path: e.path().to_string(),
            message: e.inner().to_string(),
        })
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}
