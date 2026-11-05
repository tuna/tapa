//! Token-level expression types for Verilog parameter/range expressions.

/// An expression is a sequence of tokens.
pub type Expression = Vec<String>;

/// Concatenate an expression's token representations back into source
/// text (no separators; tokenization preserved operator characters).
#[must_use]
pub fn expression_source(expr: &Expression) -> String {
    expr.concat()
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
                tokens.push(std::mem::take(&mut current));
            }
            if !ch.is_ascii_whitespace() {
                tokens.push(ch.to_string());
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_range() {
        let tokens = tokenize_expression("31:0");
        assert_eq!(tokens, ["31", ":", "0"]);
    }

    #[test]
    fn identifier_in_range() {
        let tokens = tokenize_expression("WIDTH-1:0");
        assert_eq!(tokens, ["WIDTH", "-", "1", ":", "0"]);
    }

    #[test]
    fn empty_string() {
        assert!(tokenize_expression("").is_empty());
    }
}
