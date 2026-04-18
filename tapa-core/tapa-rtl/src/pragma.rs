//! Pragma types for Verilog module attributes.

use serde::{Deserialize, Serialize};

/// A structured pragma extracted from a Verilog attribute.
///
/// Pragmas appear as `(* key = "value" *)` or `(* key *)` in Verilog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pragma {
    /// Pragma key (e.g., `"RS_HS"`, `"RS_CLK"`).
    pub key: String,
    /// Optional value string (e.g., `"port_name.data"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Raw source line for verbatim preservation.
    pub raw_line: String,
}
