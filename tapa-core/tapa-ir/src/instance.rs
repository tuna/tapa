//! Task instantiation types.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::port::ArgCategory;

/// A single argument connecting a parent port/FIFO to a child task port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Arg {
    /// Resolved name in parent scope (port/FIFO name or Verilog literal).
    pub arg: String,
    /// Category: matches one of the 10 valid wire strings.
    pub cat: ArgCategory,
}

/// A single instantiation of a child task within a parent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskInstance {
    /// Optional instance name emitted by tapacc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Arguments: maps child-port name → connection info.
    pub args: BTreeMap<String, Arg>,
    /// Bulk-synchronous step (can be negative for autorun tasks).
    #[serde(default)]
    pub step: i64,
}

impl TaskInstance {
    /// Return the explicit instance name, or the legacy
    /// `{definition_name}_{index}` name when no name was emitted.
    #[must_use]
    pub fn canonical_name(&self, definition_name: &str, index: usize) -> Cow<'_, str> {
        self.name.as_deref().map_or_else(
            || Cow::Owned(format!("{definition_name}_{index}")),
            Cow::Borrowed,
        )
    }
}
