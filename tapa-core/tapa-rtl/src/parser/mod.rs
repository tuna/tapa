//! Constrained Verilog parser for TAPA-generated module headers.
//!
//! Uses `nom` parser combinators for signal declarations and procedural
//! block skipping, and `tree-sitter-systemverilog` for module headers,
//! ports, and parameters.

use nom::branch::alt;
use nom::bytes::complete::{tag, take_until, take_while1};
use nom::character::complete::{char, multispace0, space0};
use nom::combinator::{opt, value};
use nom::sequence::{delimited, pair, preceded, terminated};
use nom::IResult;
use nom::Parser;

use crate::error::ParseError;
use crate::expression::tokenize_expression;
use crate::module::VerilogModule;
use crate::port::Width;
use crate::pragma::Pragma;
use crate::signal::{Signal, SignalKind};

mod tree_sitter;

// ── Utility parsers ─────────────────────────────────────────────────

fn ws(input: &str) -> IResult<&str, &str> {
    multispace0(input)
}

fn identifier(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$')(input)
}

/// Parse a width specification: `[expr:expr]`.
/// Handles nested brackets and part-select operators (`+:`, `-:`).
fn width_spec(input: &str) -> IResult<&str, Width> {
    let (input, _) = char('[').parse(input)?;
    // Find the msb:lsb colon at bracket depth 0, skipping part-selects.
    let (colon_pos, close_pos) = find_width_split(input).ok_or_else(|| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Char))
    })?;
    let msb_str = &input[..colon_pos];
    let lsb_str = &input[colon_pos + 1..close_pos];
    let rest = &input[close_pos + 1..];
    Ok((
        rest,
        Width {
            msb: tokenize_expression(msb_str),
            lsb: tokenize_expression(lsb_str),
        },
    ))
}

/// Find the top-level `:` separator and closing `]` positions.
/// Skips colons inside nested brackets (e.g., `M[n*32 +: 32]`).
/// Returns `Some((colon_offset, close_bracket_offset))` or `None`.
fn find_width_split(input: &str) -> Option<(usize, usize)> {
    let mut depth: u32 = 0;
    let mut colon_pos = None;
    for (i, b) in input.bytes().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' if depth > 0 => depth -= 1,
            b']' if depth == 0 => return colon_pos.map(|cp| (cp, i)),
            b':' if depth == 0 && colon_pos.is_none() => colon_pos = Some(i),
            _ => {}
        }
    }
    None
}

// ── Pragma / attribute parsers ──────────────────────────────────────

/// Parse a single attribute: `(* key = "value" *)` or `(* key *)`.
fn attribute(input: &str) -> IResult<&str, Pragma> {
    let start = input;
    let (input, _) = tag("(*").parse(input)?;
    let (input, _) = ws(input)?;
    let (input, key) = identifier(input)?;
    let (input, _) = ws(input)?;
    let (input, value) = opt(preceded(
        pair(char('='), multispace0),
        delimited(char('"'), take_until("\""), char('"')),
    ))
    .parse(input)?;
    let (input, _) = ws(input)?;
    let (input, _) = tag("*)").parse(input)?;

    let consumed = &start[..start.len() - input.len()];
    Ok((
        input,
        Pragma {
            key: key.to_owned(),
            value: value.map(str::to_owned),
            raw_line: consumed.to_owned(),
        },
    ))
}

/// Fallback: capture raw `(* ... *)` when structured parsing fails.
fn raw_attribute(input: &str) -> IResult<&str, Pragma> {
    let start = input;
    let (input, _) = tag("(*").parse(input)?;
    let (input, _content) = take_until("*)").parse(input)?;
    let (input, _) = tag("*)").parse(input)?;
    let consumed = &start[..start.len() - input.len()];
    Ok((
        input,
        Pragma {
            key: String::new(),
            value: None,
            raw_line: consumed.to_owned(),
        },
    ))
}

/// Parse zero or more attributes before a declaration.
fn attributes(input: &str) -> IResult<&str, Vec<Pragma>> {
    let mut pragmas = Vec::new();
    let mut remaining = input;
    loop {
        let (next, _) = ws(remaining)?;
        if !next.starts_with("(*") {
            remaining = next;
            break;
        }
        // Try structured parse first, then raw fallback.
        if let Ok((after, pragma)) = attribute(next) {
            pragmas.push(pragma);
            remaining = after;
        } else if let Ok((after, raw_pragma)) = raw_attribute(next) {
            pragmas.push(raw_pragma);
            remaining = after;
        } else {
            remaining = next;
            break;
        }
    }
    Ok((remaining, pragmas))
}

// ── Module header helpers ───────────────────────────────────────────

/// Parse signal kind keyword.
fn signal_kind(input: &str) -> IResult<&str, SignalKind> {
    alt((
        value(SignalKind::Wire, tag("wire")),
        value(SignalKind::Reg, tag("reg")),
    ))
    .parse(input)
}

/// Parse: `wire|reg [signed] [width] name1 [dims] [= expr], name2 [dims] [= expr], ... ;`
/// Returns multiple signals for comma-separated declarations.
fn signal_declarations(input: &str) -> IResult<&str, Vec<Signal>> {
    let (input, kind) = signal_kind(input)?;
    let (input, _) = multispace0(input)?;
    let input = skip_signedness(input);
    let (input, w) = opt(terminated(width_spec, multispace0)).parse(input)?;

    let mut sigs = Vec::new();
    let mut cursor = input;
    loop {
        let (rest, _) = multispace0(cursor)?;
        let (rest, name) = identifier(rest)?;
        let (rest, _) = space0(rest)?;
        let rest = skip_bracketed_dimensions(rest);
        // Skip optional `= expr`.
        let (rest, _) = space0(rest)?;
        let rest = if rest.starts_with('=') {
            // Skip the assignment expression, respecting nested braces.
            skip_to_comma_or_semi_balanced(rest)
        } else {
            rest
        };
        sigs.push(Signal {
            name: name.to_owned(),
            kind,
            width: w.clone(),
            attribute: None,
        });
        let (rest, _) = space0(rest)?;
        if let Some(after_comma) = rest.strip_prefix(',') {
            cursor = after_comma;
        } else {
            let (rest, _) = char(';').parse(rest)?;
            return Ok((rest, sigs));
        }
    }
}

/// Skip an assignment expression until `,` or `;` at brace/bracket depth 0.
fn skip_to_comma_or_semi_balanced(input: &str) -> &str {
    let mut depth: u32 = 0;
    for (i, b) in input.bytes().enumerate() {
        match b {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' if depth > 0 => depth -= 1,
            b',' | b';' if depth == 0 => return &input[i..],
            _ => {}
        }
    }
    input
}

/// Skip zero or more bracketed dimensions like `[0:N-1]` after a signal name.
fn skip_bracketed_dimensions(mut input: &str) -> &str {
    while input.starts_with('[') {
        if let Some(close) = input.find(']') {
            input = input[close + 1..].trim_start();
        } else {
            break;
        }
    }
    input
}

/// Skip `signed` or `unsigned` qualifier if present.
fn skip_signedness(input: &str) -> &str {
    for kw in &["signed", "unsigned"] {
        if let Some(rest) = input.strip_prefix(kw) {
            if rest.starts_with(|c: char| c.is_ascii_whitespace() || c == '[') {
                return rest.trim_start();
            }
        }
    }
    input
}

// ── Top-level parser ────────────────────────────────────────────────

/// Parse a TAPA-generated Verilog module, extracting all interface elements.
#[allow(
    clippy::too_many_lines,
    reason = "main parser entrypoint; splitting would fragment the parse loop"
)]
pub fn parse_module(source: &str) -> Result<VerilogModule, ParseError> {
    let (module_start, leading_pragmas) = find_module_with_pragmas(source)?;
    let header_input = &source[module_start..];

    // Extract module name for error context even if full header parse fails.
    let partial_name = header_input
        .strip_prefix("module")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("<unknown>")
        .trim_end_matches('(')
        .to_owned();

    // Reject sources that contain tree-sitter ERROR nodes (malformed
    // declarations that error-recovery would otherwise swallow).
    if let Some(issue) = tree_sitter::first_parse_error(source) {
        return Err(ParseError::ParseFailed {
            module: partial_name,
            message: match issue {
                tree_sitter::ParseIssue::MalformedPort => "malformed port declaration".to_string(),
                tree_sitter::ParseIssue::MalformedParameter => "malformed parameter".to_string(),
            },
        });
    }

    // Use tree-sitter to extract module name, ports, and parameters.
    let info = tree_sitter::parse_module_info(source).ok_or_else(|| ParseError::ParseFailed {
        module: partial_name,
        message: "module header: tree-sitter failed to extract header".to_string(),
    })?;

    let name = info.name;
    let port_names = info.port_names;
    let ports = info.ports;
    let parameters = info.params;
    let mut signals: Vec<Signal> = Vec::new();
    let mut pragmas = leading_pragmas;

    // Scan body for signal declarations and any stray pragmas.
    let mut cursor = &source[info.header_end..];
    loop {
        cursor = cursor.trim_start();
        if cursor.is_empty() || cursor.starts_with("endmodule") {
            break;
        }

        // Collect body pragmas.
        if cursor.starts_with("(*") {
            if let Ok((after_attrs, mut attrs)) = attributes(cursor) {
                if !attrs.is_empty() {
                    pragmas.append(&mut attrs);
                    cursor = after_attrs;
                    continue;
                }
            }
        }

        // Try signal declaration — malformed signal is a fatal error.
        if cursor.starts_with("wire") || cursor.starts_with("reg") {
            let (rest, mut sigs) = signal_declarations(cursor).map_err(|_| {
                let line = cursor.lines().next().unwrap_or(cursor);
                ParseError::ParseFailed {
                    module: name.clone(),
                    message: format!("malformed signal declaration: {line}"),
                }
            })?;
            signals.append(&mut sigs);
            cursor = rest;
            continue;
        }

        // Skip single-line comments (// ...) without consuming the next line.
        if cursor.starts_with("//") {
            cursor = cursor.find('\n').map_or("", |i| &cursor[i + 1..]);
            continue;
        }

        // Skip block comments (/* ... */).
        if cursor.starts_with("/*") {
            cursor = cursor.find("*/").map_or("", |i| &cursor[i + 2..]);
            continue;
        }

        // Skip procedural blocks (always, initial, generate, etc.) entirely.
        if cursor.starts_with("always")
            || cursor.starts_with("initial")
            || cursor.starts_with("generate")
            || cursor.starts_with("function")
            || cursor.starts_with("task")
        {
            cursor = skip_procedural_block(cursor);
            continue;
        }

        // Skip non-interface statements (assign, instance, parameter, port decl, etc.).
        cursor = skip_line(cursor);
    }

    // Verify all header-listed ports have declarations.
    let declared_names: std::collections::HashSet<&str> =
        ports.iter().map(|p| p.name.as_str()).collect();
    for pname in &port_names {
        if !declared_names.contains(pname.as_str()) {
            return Err(ParseError::ParseFailed {
                module: name.clone(),
                message: format!("port `{pname}` listed in header but has no declaration"),
            });
        }
    }

    Ok(VerilogModule {
        name,
        ports,
        parameters,
        signals,
        pragmas,
        source: source.to_owned(),
    })
}

/// Skip a procedural block (always, initial, etc.) including nested begin/end.
fn skip_procedural_block(input: &str) -> &str {
    // Count begin/end nesting depth; also handle single-statement blocks.
    let mut cursor = input;
    let mut depth: u32 = 0;
    let mut found_begin = false;

    loop {
        cursor = cursor.trim_start();
        if cursor.is_empty() || cursor.starts_with("endmodule") {
            return cursor;
        }

        // Check for block keywords.
        if starts_with_keyword(cursor, "begin") {
            depth += 1;
            found_begin = true;
            cursor = &cursor[5..];
            continue;
        }
        if starts_with_keyword(cursor, "end") && !cursor.starts_with("endmodule") {
            depth = depth.saturating_sub(1);
            cursor = &cursor[3..];
            if depth == 0 && found_begin {
                return cursor;
            }
            continue;
        }

        // If we haven't entered a begin/end block and we hit a semicolon,
        // the procedural statement is complete (single-line always @ (...) stmt;).
        if let Some(semi) = cursor.find(';') {
            cursor = &cursor[semi + 1..];
            if !found_begin || depth == 0 {
                return cursor;
            }
        } else {
            return cursor;
        }
    }
}

/// Check if input starts with a keyword (not a prefix of a longer identifier).
fn starts_with_keyword(input: &str, keyword: &str) -> bool {
    input.starts_with(keyword)
        && input[keyword.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
}

/// Find the module keyword and extract any leading pragmas/attributes.
fn find_module_with_pragmas(source: &str) -> Result<(usize, Vec<Pragma>), ParseError> {
    let mut i = 0;
    let bytes = source.as_bytes();
    let mut leading_pragmas = Vec::new();

    while i < bytes.len() {
        // Skip whitespace.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Skip single-line comments.
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Skip block comments.
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Parse and collect attributes: `(* ... *)`
        if i + 1 < bytes.len() && bytes[i] == b'(' && bytes[i + 1] == b'*' {
            // Try structured parse, then raw fallback.
            if let Ok((rest, pragma)) = attribute(&source[i..]) {
                let consumed = source[i..].len() - rest.len();
                leading_pragmas.push(pragma);
                i += consumed;
                continue;
            }
            if let Ok((rest, raw_pragma)) = raw_attribute(&source[i..]) {
                let consumed = source[i..].len() - rest.len();
                leading_pragmas.push(raw_pragma);
                i += consumed;
                continue;
            }
            // Cannot parse attribute at all — skip past `*)`
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b')') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Skip compiler directives (backtick lines like `timescale).
        if bytes[i] == b'`' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Check for `module` keyword.
        if source[i..].starts_with("module") {
            let after = i + 6;
            if after >= bytes.len() || !bytes[after].is_ascii_alphanumeric() {
                return Ok((i, leading_pragmas));
            }
        }
        return Err(ParseError::NoModuleFound);
    }
    Err(ParseError::NoModuleFound)
}

/// Skip to the next line (past the next semicolon or newline).
fn skip_line(input: &str) -> &str {
    if let Some(semi) = input.find(';') {
        &input[semi + 1..]
    } else if let Some(nl) = input.find('\n') {
        &input[nl + 1..]
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::Direction;

    #[test]
    fn parse_simple_module() {
        let src = "
module Simple (
  ap_clk,
  ap_rst_n,
  data_in,
  data_out
);

input   ap_clk;
input   ap_rst_n;
input  [31:0] data_in;
output [31:0] data_out;

wire [31:0] internal;
reg done;

endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "Simple");
        assert_eq!(m.ports.len(), 4);
        assert_eq!(m.signals.len(), 2);
        assert_eq!(m.parameters.len(), 0);

        let clk = m.ports.iter().find(|p| p.name == "ap_clk").unwrap();
        assert_eq!(clk.direction, Direction::Input);
        assert!(clk.width.is_none());

        let data_in = m.ports.iter().find(|p| p.name == "data_in").unwrap();
        assert_eq!(data_in.direction, Direction::Input);
        assert!(data_in.width.is_some());
    }

    #[test]
    fn parse_with_parameters() {
        let src = "
module WithParams (
  ap_clk
);

parameter ap_ST_fsm_state1 = 1'd1;
parameter [31:0] DATA_WIDTH = 32;

input ap_clk;

endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "WithParams");
        assert_eq!(m.parameters.len(), 2);
        assert_eq!(m.parameters[0].name, "ap_ST_fsm_state1");
        assert!(m.parameters[0].width.is_none());
        assert_eq!(m.parameters[1].name, "DATA_WIDTH");
        assert!(m.parameters[1].width.is_some());
    }

    #[test]
    fn parse_with_attributes() {
        let src = r#"
(* CORE_GENERATION_INFO="Test,hls_ip" *)
module AttrMod (
  ap_clk
);

(* RS_CLK *)
input ap_clk;

endmodule
"#;
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "AttrMod");
        assert!(!m.pragmas.is_empty(), "pragmas: {:?}", m.pragmas);
    }

    #[test]
    fn empty_input_rejected() {
        parse_module("").unwrap_err();
        parse_module("   ").unwrap_err();
    }

    #[test]
    fn non_verilog_rejected() {
        parse_module("hello world this is not verilog").unwrap_err();
    }

    #[test]
    fn parse_ansi_ports_with_direction_and_width() {
        let src = "
module AnsiMod (
  input wire [31:0] a,
  output [7:0] b,
  c
);
endmodule
";
        let m = parse_module(src).unwrap();
        assert_eq!(m.name, "AnsiMod");
        assert_eq!(m.ports.len(), 3);
        let a = m.ports.iter().find(|p| p.name == "a").unwrap();
        assert_eq!(a.direction, Direction::Input);
        assert!(a.width.is_some());
        let b = m.ports.iter().find(|p| p.name == "b").unwrap();
        assert_eq!(b.direction, Direction::Output);
        assert!(b.width.is_some());
        let c = m.ports.iter().find(|p| p.name == "c").unwrap();
        assert_eq!(c.direction, Direction::Output);
        // Width is inherited across comma-separated ANSI ports
        assert!(c.width.is_some());
    }

    #[test]
    fn parse_ansi_ports_with_pragma() {
        let src = "
module AnsiPragma (
  (* RS_CLK *) input ap_clk,
  output ap_done
);
endmodule
";
        let m = parse_module(src).unwrap();
        let clk = m.ports.iter().find(|p| p.name == "ap_clk").unwrap();
        assert!(clk.pragma.is_some());
    }

    #[test]
    fn parse_parameter_block_in_header() {
        let src = "
module ParamMod #(
  parameter WIDTH = 32,
  parameter [7:0] MASK = 8'hFF
) (
  input [WIDTH-1:0] data
);
endmodule
";
        let m = parse_module(src).unwrap();
        assert_eq!(m.parameters.len(), 2);
        assert_eq!(m.parameters[0].name, "WIDTH");
        assert_eq!(m.parameters[1].name, "MASK");
        let data = m.ports.iter().find(|p| p.name == "data").unwrap();
        assert!(data.width.is_some());
    }

    #[test]
    fn parse_module_with_comments() {
        let src = "
// Leading comment
module CommentMod (
  ap_clk,
  ap_rst_n
);
input ap_clk;
// this is a comment
input ap_rst_n;
endmodule
";
        let m = parse_module(src).unwrap();
        assert_eq!(m.name, "CommentMod");
        assert_eq!(m.ports.len(), 2);
    }

    #[test]
    fn parse_signal_with_balanced_assignment() {
        let src = "
module SigAssign;
  wire [31:0] w = {1'b1, 2'b10};
endmodule
";
        let m = parse_module(src).unwrap();
        assert_eq!(m.signals.len(), 1);
        assert_eq!(m.signals[0].name, "w");
    }

    #[test]
    fn parse_multiple_pragmas_in_body() {
        let src = r#"
module PragmaMod (
  ap_clk
);
input ap_clk;
(* DONT_TOUCH = "yes" *)
(* KEEP *)
wire net;
endmodule
"#;
        let m = parse_module(src).unwrap();
        assert!(m.pragmas.iter().any(|p| p.key == "DONT_TOUCH"));
        assert!(m.pragmas.iter().any(|p| p.key == "KEEP"));
    }

    #[test]
    fn parse_parameter_with_nested_parens() {
        let src = "
module NestParam #(
  parameter DEPTH = (1 + (2))
) (
  input a
);
endmodule
";
        let m = parse_module(src).unwrap();
        assert_eq!(m.parameters.len(), 1);
        assert_eq!(m.parameters[0].name, "DEPTH");
    }
}
