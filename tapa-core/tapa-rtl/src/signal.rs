//! Signal types (wire and reg declarations).

use crate::port::Width;

/// Kind of signal declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Wire,
    Reg,
}

/// A signal (wire or reg) extracted from a Verilog module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub name: String,
    pub kind: SignalKind,
    /// Optional width `[msb:lsb]`; `None` means 1-bit.
    pub width: Option<Width>,
    /// Optional synthesis attribute emitted before the declaration, e.g.
    /// `max_fanout = 256` renders as `(* max_fanout = 256 *) wire ...;`.
    /// Parsed modules never populate this (existing attributes are preserved
    /// verbatim elsewhere); it is set only on codegen-generated signals.
    pub attribute: Option<String>,
}
