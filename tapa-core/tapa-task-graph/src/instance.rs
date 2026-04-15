//! Task instantiation types.

use std::collections::HashMap;

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
    /// Arguments: maps child-port name → connection info.
    pub args: HashMap<String, Arg>,
    /// Bulk-synchronous step (can be negative for autorun tasks).
    #[serde(default)]
    pub step: i64,
}
