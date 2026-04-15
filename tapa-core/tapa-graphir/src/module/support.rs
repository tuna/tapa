//! Supporting types: ports, parameters, nets, ranges.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::definition::HierarchicalName;
use crate::expression::Expression;

/// A range with expression-valued bounds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Range {
    pub left: Expression,
    pub right: Expression,
}

/// A module port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModulePort {
    pub name: String,
    #[serde(default)]
    pub hierarchical_name: HierarchicalName,
    /// Port type string (e.g. `"input wire"`, `"output reg"`).
    #[serde(rename = "type")]
    pub port_type: String,
    #[serde(default)]
    pub range: Option<Range>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A module parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleParameter {
    pub name: String,
    #[serde(default)]
    pub hierarchical_name: HierarchicalName,
    pub expr: Expression,
    #[serde(default)]
    pub range: Option<Range>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// An internal wire / net.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleNet {
    pub name: String,
    #[serde(default)]
    pub hierarchical_name: HierarchicalName,
    #[serde(default)]
    pub range: Option<Range>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
