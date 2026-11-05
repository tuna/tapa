//! Token-level expression types for Verilog parameter/range expressions.

/// A single token in an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Text representation.
    pub repr: String,
}

/// An expression is a sequence of tokens.
pub type Expression = Vec<Token>;

/// Concatenate an expression's token representations back into source
/// text (no separators; tokenization preserved operator characters).
#[must_use]
pub fn expression_source(expr: &Expression) -> String {
    expr.iter().map(|t| t.repr.as_str()).collect()
}

/// Evaluate an expression as a plain unsigned integer, when it is one.
/// Returns `None` for anything symbolic (`WIDTH-1`, parameters, ...).
#[must_use]
pub fn expression_as_u32(expr: &Expression) -> Option<u32> {
    expression_source(expr).parse().ok()
}

/// Parse a width expression string like `"31:0"` or `"WIDTH-1:0"` into tokens.
///
/// Splits on operators and punctuation.
pub fn tokenize_expression(input: &str) -> Expression {
    let input = input.trim();
    if input.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if matches!(
            ch,
            ':' | '+' | '-' | '*' | '/' | '(' | ')' | '[' | ']' | ' ' | '\t'
        ) {
            if !current.is_empty() {
                tokens.push(Token {
                    repr: std::mem::take(&mut current),
                });
            }
            if !ch.is_ascii_whitespace() {
                tokens.push(Token {
                    repr: ch.to_string(),
                });
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(Token { repr: current });
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
        assert_eq!(tokens[1].repr, ":");
        assert_eq!(tokens[2].repr, "0");
    }

    #[test]
    fn identifier_in_range() {
        let tokens = tokenize_expression("WIDTH-1:0");
        assert_eq!(tokens.len(), 5);
        assert_eq!(tokens[0].repr, "WIDTH");
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
