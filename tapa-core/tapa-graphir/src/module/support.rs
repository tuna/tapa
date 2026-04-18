//! Supporting types: ports, parameters, nets, ranges.

use std::collections::BTreeMap;

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
    #[serde(default, skip_serializing_if = "HierarchicalName::is_none")]
    pub hierarchical_name: HierarchicalName,
    /// Port type string (e.g. `"input wire"`, `"output reg"`).
    #[serde(rename = "type")]
    pub port_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A module parameter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleParameter {
    pub name: String,
    #[serde(default, skip_serializing_if = "HierarchicalName::is_none")]
    pub hierarchical_name: HierarchicalName,
    pub expr: Expression,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ModulePort {
    /// Returns `true` if this port is an input (wire, reg, or plain).
    #[must_use]
    pub fn is_input(&self) -> bool {
        self.port_type.starts_with("input")
    }

    /// Returns `true` if this port is an output (wire, reg, or plain).
    #[must_use]
    pub fn is_output(&self) -> bool {
        self.port_type.starts_with("output")
    }

    /// Returns the width expression from the range, if present.
    /// For a range `[left:right]`, returns the `left` expression.
    #[must_use]
    pub fn get_width_expr(&self) -> Option<&Expression> {
        self.range.as_ref().map(|r| &r.left)
    }
}

/// An internal wire / net.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModuleNet {
    pub name: String,
    #[serde(default, skip_serializing_if = "HierarchicalName::is_none")]
    pub hierarchical_name: HierarchicalName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
