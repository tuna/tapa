//! Signal types (wire and reg declarations).

use serde::{Deserialize, Serialize};

use crate::port::Width;

/// Kind of signal declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalKind {
    Wire,
    Reg,
}

/// A signal (wire or reg) extracted from a Verilog module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    /// Signal name.
    pub name: String,
    /// Wire or reg.
    pub kind: SignalKind,
    /// Optional width `[msb:lsb]`; `None` means 1-bit.
    pub width: Option<Width>,
}
