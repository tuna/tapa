//! Root project container.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blackbox::BlackBox;
use crate::error::ParseError;
use crate::module::definition::AnyModuleDefinition;

/// Collection of module definitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Modules {
    /// Namespace name (typically `"$root"`).
    pub name: String,
    /// Module definitions sorted by name.
    pub module_definitions: Vec<AnyModuleDefinition>,
    /// Name of the top-level module.
    #[serde(default)]
    pub top_name: Option<String>,
}

/// Root of the `GraphIR` JSON (`graphir.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    /// FPGA part number.
    #[serde(default)]
    pub part_num: Option<String>,
    /// Module definitions.
    pub modules: Modules,
    /// External blackbox files.
    #[serde(default)]
    pub blackboxes: Vec<BlackBox>,
    /// Interface definitions per module.
    #[serde(default)]
    pub ifaces: Option<HashMap<String, Vec<crate::interface::AnyInterface>>>,
    /// RS pragma annotations per module.
    #[serde(default)]
    pub module_to_rtl_pragmas: Option<HashMap<String, Vec<String>>>,
    /// Old-style RS pragmas (nested dicts).
    #[serde(default)]
    pub module_to_old_rtl_pragmas: Option<HashMap<String, Value>>,
    /// Floorplan island → pblock range mapping.
    #[serde(default)]
    pub island_to_pblock_range: Option<HashMap<String, Vec<String>>>,
    /// Route paths.
    #[serde(default)]
    pub routes: Option<Vec<Value>>,
    /// Resource usage upper bounds.
    #[serde(default)]
    pub resource_to_max_local_usage: Option<HashMap<String, f64>>,
    /// Cut crossing counts.
    #[serde(default)]
    pub cut_to_crossing_count: Option<HashMap<String, f64>>,
    /// Forward-compatibility: preserve any unknown top-level fields.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Project {
    /// Parse a `graphir.json` payload with field-path error diagnostics.
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
