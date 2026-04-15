//! Expression types — a sequence of tokens.

use serde::{Deserialize, Serialize};

/// A single token in an expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Token {
    /// `"id"` for identifier, `"lit"` for literal.
    #[serde(rename = "type")]
    pub kind: TokenKind,
    /// Text representation.
    pub repr: String,
}

/// Token kind discriminator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TokenKind {
    #[serde(rename = "id")]
    Identifier,
    #[serde(rename = "lit")]
    Literal,
}

/// An expression is a sequence of tokens.
pub type Expression = Vec<Token>;
