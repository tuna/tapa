//! Module instantiation types.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::definition::HierarchicalName;
use crate::expression::Expression;

/// A port/parameter connection on a module instantiation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleConnection {
    pub name: String,
    #[serde(default)]
    pub hierarchical_name: HierarchicalName,
    pub expr: Expression,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Resource area for an instance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceArea {
    pub ff: u64,
    pub lut: u64,
    pub dsp: u64,
    pub bram_18k: u64,
    pub uram: u64,
}

/// A module instantiation within a grouped module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleInstantiation {
    pub name: String,
    #[serde(default)]
    pub hierarchical_name: HierarchicalName,
    /// Reference to the module definition name.
    pub module: String,
    #[serde(default)]
    pub connections: Vec<ModuleConnection>,
    #[serde(default)]
    pub parameters: Vec<ModuleConnection>,
    #[serde(default)]
    pub floorplan_region: Option<String>,
    #[serde(default)]
    pub area: Option<InstanceArea>,
    #[serde(default)]
    pub pragmas: Vec<(String, String)>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
