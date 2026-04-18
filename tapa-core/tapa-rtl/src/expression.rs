//! Token-level expression types compatible with `GraphIR`.

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

/// Classify a string as an identifier token or a literal token.
fn classify_token(s: &str) -> Token {
    let kind = if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') {
        TokenKind::Identifier
    } else {
        TokenKind::Literal
    };
    Token {
        kind,
        repr: s.to_owned(),
    }
}

/// Parse a width expression string like `"31:0"` or `"WIDTH-1:0"` into tokens.
///
/// Splits on operators and punctuation, classifying each piece as
/// identifier or literal.
pub fn tokenize_expression(input: &str) -> Expression {
    let input = input.trim();
    if input.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if matches!(ch, ':' | '+' | '-' | '*' | '/' | '(' | ')' | '[' | ']' | ' ' | '\t') {
            if !current.is_empty() {
                tokens.push(classify_token(&current));
                current.clear();
            }
            if !ch.is_ascii_whitespace() {
                tokens.push(Token {
                    kind: TokenKind::Literal,
                    repr: ch.to_string(),
                });
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(classify_token(&current));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_range() {
        let tokens = tokenize_expression("31:0");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].repr, "31");
        assert_eq!(tokens[0].kind, TokenKind::Literal);
        assert_eq!(tokens[1].repr, ":");
        assert_eq!(tokens[2].repr, "0");
    }

    #[test]
    fn identifier_in_range() {
        let tokens = tokenize_expression("WIDTH-1:0");
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].repr, "WIDTH");
        assert_eq!(tokens[0].kind, TokenKind::Identifier);
        assert_eq!(tokens[1].repr, "-");
        assert_eq!(tokens[2].repr, "1");
        assert_eq!(tokens[3].repr, ":");
        assert_eq!(tokens[4].repr, "0");
    }

    #[test]
    fn empty_string() {
        assert!(tokenize_expression("").is_empty());
    }
}
