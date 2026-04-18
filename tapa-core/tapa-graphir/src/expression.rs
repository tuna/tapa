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

impl Token {
    /// Create an identifier token.
    #[must_use]
    pub fn new_id(name: &str) -> Self {
        Self {
            kind: TokenKind::Identifier,
            repr: name.to_owned(),
        }
    }

    /// Create a literal token.
    #[must_use]
    pub fn new_lit(value: &str) -> Self {
        Self {
            kind: TokenKind::Literal,
            repr: value.to_owned(),
        }
    }

    /// Returns `true` if this is a non-empty identifier token.
    #[must_use]
    pub fn is_id(&self) -> bool {
        self.kind == TokenKind::Identifier && !self.repr.is_empty()
    }

    /// Returns `true` if this is a non-empty literal token.
    #[must_use]
    pub fn is_lit(&self) -> bool {
        self.kind == TokenKind::Literal && !self.repr.is_empty()
    }
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Expression(pub Vec<Token>);

impl Expression {
    /// Create an empty expression.
    #[must_use]
    pub fn new_empty() -> Self {
        Self(Vec::new())
    }

    /// Create a single-token identifier expression.
    ///
    /// # Panics
    ///
    /// Panics if `name` is empty.
    #[must_use]
    pub fn new_id(name: &str) -> Self {
        assert!(!name.is_empty(), "identifier name must not be empty");
        Self(vec![Token::new_id(name)])
    }

    /// Create a single-token literal expression.
    #[must_use]
    pub fn new_lit(value: &str) -> Self {
        Self(vec![Token::new_lit(value)])
    }

    /// Returns `true` if this is a single identifier token.
    #[must_use]
    pub fn is_identifier(&self) -> bool {
        self.0.len() == 1 && self.0[0].kind == TokenKind::Identifier
    }

    /// Iterate over identifiers used in this expression.
    pub fn get_used_identifiers(&self) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .filter(|t| t.kind == TokenKind::Identifier)
            .map(|t| t.repr.as_str())
    }

    /// Returns `true` if the expression has no tokens.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the inner token slice.
    #[must_use]
    pub fn tokens(&self) -> &[Token] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_new_id() {
        let t = Token::new_id("foo");
        assert!(t.is_id());
        assert!(!t.is_lit());
        assert_eq!(t.repr, "foo");
    }

    #[test]
    fn token_new_lit() {
        let t = Token::new_lit("32'd0");
        assert!(t.is_lit());
        assert!(!t.is_id());
        assert_eq!(t.repr, "32'd0");
    }

    #[test]
    fn token_empty_repr_not_valid() {
        let t = Token::new_id("");
        assert!(!t.is_id());
        let t = Token::new_lit("");
        assert!(!t.is_lit());
    }

    #[test]
    fn expression_new_id() {
        let e = Expression::new_id("foo");
        assert!(e.is_identifier());
        assert_eq!(e.0.len(), 1);
        assert_eq!(e.0[0].repr, "foo");
    }

    #[test]
    #[should_panic(expected = "identifier name must not be empty")]
    fn expression_new_id_rejects_empty() {
        let _ = Expression::new_id("");
    }

    #[test]
    fn expression_new_lit() {
        let e = Expression::new_lit("32'd0");
        assert!(!e.is_identifier());
        assert_eq!(e.0.len(), 1);
    }

    #[test]
    fn expression_empty() {
        let e = Expression::new_empty();
        assert!(e.is_empty());
        assert!(!e.is_identifier());
    }

    #[test]
    fn expression_get_used_identifiers() {
        let e = Expression(vec![
            Token::new_id("a"),
            Token::new_lit("+"),
            Token::new_id("b"),
        ]);
        let ids: Vec<&str> = e.get_used_identifiers().collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn expression_serde_round_trip() {
        let e = Expression::new_id("signal_a");
        let json = serde_json::to_string(&e).unwrap();
        let e2: Expression = serde_json::from_str(&json).unwrap();
        assert_eq!(e, e2);
    }
}
