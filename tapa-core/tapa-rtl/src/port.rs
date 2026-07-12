//! Port types for Verilog module interfaces.

use serde::{Deserialize, Serialize};

use crate::expression::Expression;
use crate::pragma::Pragma;

/// Direction of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Input,
    Output,
    Inout,
}

/// Width of a port or signal, represented as `[msb:lsb]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Width {
    /// Most-significant bit expression.
    pub msb: Expression,
    /// Least-significant bit expression.
    pub lsb: Expression,
}

impl Width {
    /// Number of bits spanned, when both endpoints are integer
    /// literals; `None` for symbolic widths.
    #[must_use]
    pub fn bit_count(&self) -> Option<u32> {
        let msb = crate::expression::expression_as_u32(&self.msb)?;
        let lsb = crate::expression::expression_as_u32(&self.lsb)?;
        Some(msb.abs_diff(lsb) + 1)
    }
}

/// A port extracted from a Verilog module declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    /// Port name.
    pub name: String,
    /// Port direction.
    pub direction: Direction,
    /// Optional width `[msb:lsb]`; `None` means 1-bit.
    pub width: Option<Width>,
    /// Optional pragma attached to this port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pragma: Option<Pragma>,
}

impl Port {
    /// Bit width of the port: 1 when no width is declared, the literal
    /// span when both endpoints are integers, `None` when symbolic.
    #[must_use]
    pub fn bit_width(&self) -> Option<u32> {
        match self.width.as_ref() {
            None => Some(1),
            Some(w) => w.bit_count(),
        }
    }
}
