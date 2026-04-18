//! Module instantiation types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::definition::HierarchicalName;
use crate::expression::Expression;

/// A port/parameter connection on a module instantiation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleConnection {
    pub name: String,
    #[serde(default, skip_serializing_if = "HierarchicalName::is_none")]
    pub hierarchical_name: HierarchicalName,
    pub expr: Expression,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    #[serde(default, skip_serializing_if = "HierarchicalName::is_none")]
    pub hierarchical_name: HierarchicalName,
    /// Reference to the module definition name.
    pub module: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<ModuleConnection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ModuleConnection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floorplan_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<InstanceArea>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pragmas: Vec<(String, String)>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ModuleInstantiation {
    /// Find a connection by port name.
    #[must_use]
    pub fn get_connection(&self, port_name: &str) -> Option<&ModuleConnection> {
        self.connections.iter().find(|c| c.name == port_name)
    }

    /// Find a parameter by name.
    #[must_use]
    pub fn get_parameter(&self, param_name: &str) -> Option<&ModuleConnection> {
        self.parameters.iter().find(|p| p.name == param_name)
    }
}
