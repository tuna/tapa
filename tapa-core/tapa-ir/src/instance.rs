//! Task instantiation types.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::port::ArgCategory;

/// An integer constant bound to a child port, sized to that port's width.
///
/// The frontend reports the width and the value; how they spell out as a
/// Verilog literal is the RTL backend's decision, not the frontend's.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireValue {
    /// Bit width of the literal.
    pub width: u32,
    /// Unsigned value; a negative source constant arrives two's-complement.
    pub value: u64,
}

impl WireValue {
    /// Whether the value fits the width and the width is a real wire: a
    /// `0'd0` or `8'd300` renders invalid Verilog that only the vendor tool
    /// would reject, far from the frontend slip that produced it. Checked at
    /// the parse boundary (`TaskGraph::from_json`).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.width >= 1 && self.width <= 64 && (self.width == 64 || self.value < (1 << self.width))
    }
}

impl fmt::Display for WireValue {
    /// Render as a sized Verilog decimal literal, e.g. `64'd5`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}'d{}", self.width, self.value)
    }
}

/// What a child port is bound to in the parent's scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ArgSource {
    /// A parent-scope port or FIFO, by name.
    Name(String),
    /// A constant passed by value at the invoke site.
    Literal(WireValue),
}

impl ArgSource {
    /// The parent-scope name, or `None` for a constant.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            Self::Literal(_) => None,
        }
    }
}

/// A single argument connecting a parent port/FIFO to a child task port.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Arg {
    /// What the child port is bound to.
    pub arg: ArgSource,
    /// Category: matches one of the 10 valid wire strings.
    pub cat: ArgCategory,
}

impl Arg {
    /// Bind a child port to a parent-scope name.
    pub fn named(name: impl Into<String>, cat: ArgCategory) -> Self {
        Self {
            arg: ArgSource::Name(name.into()),
            cat,
        }
    }

    /// The parent-scope name, or `None` for a constant.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.arg.name()
    }
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
    /// Return the explicit instance name, or `{definition_name}_{index}`
    /// when the invoke was written without a name.
    #[must_use]
    pub fn canonical_name(&self, definition_name: &str, index: usize) -> Cow<'_, str> {
        self.name.as_deref().map_or_else(
            || Cow::Owned(format!("{definition_name}_{index}")),
            Cow::Borrowed,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend reports a constant's shape; only the backend spells it
    /// as Verilog. Keeping that split is the point of the typed value.
    #[test]
    fn a_constant_renders_as_a_sized_verilog_literal() {
        assert_eq!(
            WireValue {
                width: 64,
                value: 100
            }
            .to_string(),
            "64'd100"
        );
        assert_eq!(WireValue { width: 1, value: 0 }.to_string(), "1'd0");
    }

    #[test]
    fn a_binding_is_either_a_name_or_a_constant() {
        let named: Arg = serde_json::from_str(r#"{"arg": "q1", "cat": "istream"}"#).expect("parse");
        assert_eq!(named.name(), Some("q1"));

        let constant: Arg =
            serde_json::from_str(r#"{"arg": {"width": 64, "value": 5}, "cat": "scalar"}"#)
                .expect("parse");
        assert_eq!(constant.name(), None);
        assert_eq!(
            constant.arg,
            ArgSource::Literal(WireValue {
                width: 64,
                value: 5
            })
        );
    }

    #[test]
    fn both_bindings_survive_a_json_round_trip() {
        for json in [
            r#"{"arg":"q1","cat":"istream"}"#,
            r#"{"arg":{"width":64,"value":5},"cat":"scalar"}"#,
        ] {
            let arg: Arg = serde_json::from_str(json).expect("parse");
            assert_eq!(serde_json::to_string(&arg).expect("serialize"), json);
        }
    }
}
