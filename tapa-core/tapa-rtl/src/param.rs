//! Parameter types for Verilog module declarations.

use serde::{Deserialize, Serialize};

use crate::expression::Expression;
use crate::port::Width;

/// A parameter extracted from a Verilog module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter name.
    pub name: String,
    /// Default value as a token expression.
    pub default: Expression,
    /// Optional width `[msb:lsb]`.
    pub width: Option<Width>,
}
