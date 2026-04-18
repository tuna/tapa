//! Constrained Verilog parser for TAPA-generated module headers.
//!
//! Uses `nom` parser combinators to extract interface elements from
//! non-ANSI Verilog module declarations (HLS tool output format).

use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::{tag, take_until, take_while1};
use nom::character::complete::{char, multispace0, multispace1, space0};
use nom::combinator::{opt, value};
use nom::sequence::{delimited, pair, preceded, terminated};
use nom::IResult;

use crate::error::ParseError;
use crate::expression::tokenize_expression;
use crate::module::VerilogModule;
use crate::param::Parameter;
use crate::port::{Direction, Port, Width};
use crate::pragma::Pragma;
use crate::signal::{Signal, SignalKind};

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

/// Extract balanced parenthesized content: `(content)`.
/// Handles nested parens and comments containing parens.
fn balanced_parens(input: &str) -> IResult<&str, &str> {
    let (input, _) = char('(').parse(input)?;
    let start = input;
    let mut depth: u32 = 1;
    let mut i = 0;
    let bytes = input.as_bytes();
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let content = &input[..i];
                    let rest = &input[i + 1..];
                    return Ok((rest, content));
                }
            }
            // Skip single-line comments.
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            // Skip block comments.
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            // Skip string literals.
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    // Unbalanced — fail.
    Err(nom::Err::Error(nom::error::Error::new(
        start,
        nom::error::ErrorKind::Char,
    )))
}

// ── Module header parser ────────────────────────────────────────────

/// Parse port direction keyword.
fn direction(input: &str) -> IResult<&str, Direction> {
    alt((
        value(Direction::Input, tag("input")),
        value(Direction::Output, tag("output")),
        value(Direction::Inout, tag("inout")),
    ))
    .parse(input)
}

/// Parse signal kind keyword.
fn signal_kind(input: &str) -> IResult<&str, SignalKind> {
    alt((
        value(SignalKind::Wire, tag("wire")),
        value(SignalKind::Reg, tag("reg")),
    ))
    .parse(input)
}

/// Parse: `direction [wire|reg] [signed] [width] name1, name2, ... ;`
/// Returns multiple ports for comma-separated declarations.
type PortDecl = (Direction, Option<Width>, String);
fn port_declarations(input: &str) -> IResult<&str, Vec<PortDecl>> {
    let (input, dir) = direction(input)?;
    let (input, _) = multispace0(input)?;
    // Skip optional wire/reg qualifier.
    let input = if input.starts_with("wire") || input.starts_with("reg") {
        let (rest, _) = identifier(input)?;
        let (rest, _) = multispace0(rest)?;
        rest
    } else {
        input
    };
    let input = skip_signedness(input);
    let (input, w) = opt(terminated(width_spec, multispace0)).parse(input)?;

    let mut ports = Vec::new();
    let mut cursor = input;
    loop {
        let (rest, _) = multispace0(cursor)?;
        let (rest, name) = identifier(rest)?;
        ports.push((dir, w.clone(), name.to_owned()));
        let (rest, _) = space0(rest)?;
        if let Some(after_comma) = rest.strip_prefix(',') {
            cursor = after_comma;
        } else {
            let (rest, _) = char(';').parse(rest)?;
            return Ok((rest, ports));
        }
    }
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

/// Parse: `parameter [width] name = value ;`
fn parameter_declaration(input: &str) -> IResult<&str, Parameter> {
    let (input, _) = tag("parameter").parse(input)?;
    let (input, _) = multispace1(input)?;
    let (input, w) = opt(terminated(width_spec, multispace0)).parse(input)?;
    let (input, name) = identifier(input)?;
    let (input, _) = (space0, char('='), space0).parse(input)?;
    // Take everything until the semicolon as the default value.
    let (input, val_str) = take_until(";").parse(input)?;
    let (input, _) = char(';').parse(input)?;
    Ok((
        input,
        Parameter {
            name: name.to_owned(),
            default: tokenize_expression(val_str.trim()),
            width: w,
        },
    ))
}

/// Parsed module header result.
struct ModuleHeader {
    name: String,
    /// Port names from the header (non-ANSI) or empty (ANSI/parameterized).
    port_names: Vec<String>,
    /// Parameters from `#(parameter ...)` block.
    params: Vec<Parameter>,
    /// Ports parsed from ANSI port list (if any).
    ansi_ports: Vec<Port>,
}

/// Parse module header: supports non-ANSI, parameterized, and ANSI forms.
///
/// Forms accepted:
/// - `module Name (id, id, ...);`
/// - `module Name #(parameter ...) (id, id, ...);`
/// - `module Name #(parameter ...) (input wire [w:0] p, ...);`
fn module_header(input: &str) -> IResult<&str, ModuleHeader> {
    let (input, _) = tag("module").parse(input)?;
    let (input, _) = multispace0(input)?;
    let (input, name) = identifier(input)?;
    let (input, _) = ws(input)?;

    // Optional parameter block: #(...)
    let (input, params) = if input.starts_with('#') {
        let (input, _) = char('#').parse(input)?;
        let (input, _) = multispace0(input)?;
        let (input, param_text) = balanced_parens(input)?;
        let params = parse_parameter_block(param_text);
        let (input, _) = ws(input)?;
        (input, params)
    } else {
        (input, Vec::new())
    };

    // Port list: (...) or empty (module Name;).
    if input.starts_with(';') {
        // No port list — `module Name;`
        let (input, _) = char(';').parse(input)?;
        return Ok((input, ModuleHeader {
            name: name.to_owned(),
            port_names: Vec::new(),
            params,
            ansi_ports: Vec::new(),
        }));
    }

    let (input, port_text) = balanced_parens(input)?;
    let (input, _) = (multispace0, char(';')).parse(input)?;

    // Detect ANSI vs non-ANSI by checking if the port list contains direction keywords.
    let trimmed_ports = port_text.trim();
    let is_ansi = detect_ansi_ports(trimmed_ports);

    if is_ansi {
        let ansi_ports = parse_ansi_port_list(trimmed_ports);
        Ok((input, ModuleHeader {
            name: name.to_owned(),
            port_names: Vec::new(),
            params,
            ansi_ports,
        }))
    } else {
        // Non-ANSI: comma-separated identifiers.
        let port_names = trimmed_ports
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        Ok((input, ModuleHeader {
            name: name.to_owned(),
            port_names,
            params,
            ansi_ports: Vec::new(),
        }))
    }
}

/// Strip `//` line comments from text, preserving structure.
fn strip_line_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.find("//").map_or(line, |pos| &line[..pos]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip `(* ... *)` attributes from a string, returning the first pragma (if any)
/// and the cleaned text.
fn strip_inline_attributes(text: &str) -> (Option<Pragma>, String) {
    let mut result = String::with_capacity(text.len());
    let mut pragma = None;
    let mut cursor = text;
    while let Some(start) = cursor.find("(*") {
        result.push_str(&cursor[..start]);
        let after_open = &cursor[start + 2..];
        if let Some(end) = after_open.find("*)") {
            let attr_content = after_open[..end].trim();
            if pragma.is_none() {
                let raw = &cursor[start..start + 2 + end + 2];
                // Try to parse key/value from the attribute content.
                let (key, value) = if let Some(eq) = attr_content.find('=') {
                    let k = attr_content[..eq].trim();
                    let v = attr_content[eq + 1..].trim().trim_matches('"');
                    (k.to_owned(), Some(v.to_owned()))
                } else {
                    (attr_content.to_owned(), None)
                };
                pragma = Some(Pragma {
                    key,
                    value,
                    raw_line: raw.to_owned(),
                });
            }
            cursor = &after_open[end + 2..];
        } else {
            // Unterminated attribute — keep as-is.
            result.push_str(&cursor[start..]);
            cursor = "";
            break;
        }
    }
    result.push_str(cursor);
    (pragma, result)
}

/// Detect if a port list is ANSI-style (has direction keywords).
fn detect_ansi_ports(text: &str) -> bool {
    // Skip comments, whitespace, and attributes to find the first real token.
    let mut s = text;
    loop {
        s = s.trim_start();
        if s.starts_with("//") {
            s = s.find('\n').map_or("", |i| &s[i + 1..]);
        } else if s.starts_with("/*") {
            s = s.find("*/").map_or("", |i| &s[i + 2..]);
        } else if s.starts_with("(*") {
            s = s.find("*)").map_or("", |i| &s[i + 2..]);
        } else {
            break;
        }
    }
    s.starts_with("input") || s.starts_with("output") || s.starts_with("inout")
}

/// Split on commas at brace/bracket depth 0 (respects concatenations).
fn split_balanced_commas(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut depth: u32 = 0;
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        match b {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                segments.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    segments.push(&text[start..]);
    segments
}

/// Parse a `#(parameter ...)` block into a list of Parameters.
fn parse_parameter_block(text: &str) -> Vec<Parameter> {
    let mut params = Vec::new();
    for segment in split_balanced_commas(text) {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }
        // Strip leading `parameter` keyword if present.
        let seg = seg.strip_prefix("parameter").unwrap_or(seg).trim();
        // Find `name = value` pattern.
        if let Some(eq_pos) = seg.find('=') {
            let before_eq = seg[..eq_pos].trim();
            let after_eq = seg[eq_pos + 1..].trim();
            // `before_eq` may be `[width] name` or just `name` or `type name`.
            let name = before_eq.split_whitespace().last().unwrap_or(before_eq);
            // Extract width if present: `[msb:lsb] name`.
            let width = if before_eq.contains('[') && before_eq.contains(']') {
                let open = before_eq.find('[').unwrap_or(0);
                let close = before_eq.find(']').unwrap_or(0);
                let width_str = &before_eq[open + 1..close];
                width_str.find(':').map(|colon| Width {
                    msb: tokenize_expression(width_str[..colon].trim()),
                    lsb: tokenize_expression(width_str[colon + 1..].trim()),
                })
            } else {
                None
            };
            params.push(Parameter {
                name: name.to_owned(),
                default: tokenize_expression(after_eq),
                width,
            });
        }
    }
    params
}

/// Parse ANSI port list: `input wire [w:0] name, output reg name2, ...`
/// Inherits direction and width across comma-separated ports per Verilog spec.
fn parse_ansi_port_list(text: &str) -> Vec<Port> {
    let mut ports = Vec::new();
    let cleaned = strip_line_comments(text);
    // Track inherited direction and width.
    let mut last_dir = Direction::Input;
    let mut last_width: Option<Width> = None;

    for segment in cleaned.split(',') {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }
        // Strip any (* ... *) attributes from the segment.
        let (seg_pragma, seg_clean) = strip_inline_attributes(seg);
        let seg = seg_clean.trim();
        if seg.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = seg.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        let mut idx = 0;
        let mut has_explicit_dir = false;
        let dir = match tokens.get(idx).copied() {
            Some("input") => { idx += 1; has_explicit_dir = true; Direction::Input }
            Some("output") => { idx += 1; has_explicit_dir = true; Direction::Output }
            Some("inout") => { idx += 1; has_explicit_dir = true; Direction::Inout }
            _ => last_dir,
        };
        // Skip wire/reg.
        let has_wire_reg = matches!(tokens.get(idx).copied(), Some("wire" | "reg"));
        if has_wire_reg {
            idx += 1;
        }
        let has_signed = matches!(tokens.get(idx).copied(), Some("signed" | "unsigned"));
        if has_signed {
            idx += 1;
        }
        let has_type_spec = has_explicit_dir || has_wire_reg || has_signed;
        let remaining = tokens[idx..].join(" ");
        let remaining = remaining.trim();
        // Parse width if present.
        let (explicit_width, name) = if remaining.starts_with('[') {
            if let Some(close) = remaining.find(']') {
                let width_str = &remaining[1..close];
                let after = remaining[close + 1..].trim();
                let w = width_str.find(':').map(|colon| Width {
                    msb: tokenize_expression(width_str[..colon].trim()),
                    lsb: tokenize_expression(width_str[colon + 1..].trim()),
                });
                (w, after.to_owned())
            } else {
                (None, remaining.to_owned())
            }
        } else {
            (None, remaining.to_owned())
        };
        // Update inheritance state: explicit width or new type spec resets last_width.
        last_dir = dir;
        if explicit_width.is_some() || has_type_spec {
            last_width = explicit_width;
        }
        let width = last_width.clone();
        if !name.is_empty() {
            ports.push(Port {
                name,
                direction: dir,
                width,
                pragma: seg_pragma,
            });
        }
    }
    ports
}

// ── Top-level parser ────────────────────────────────────────────────

/// Parse a TAPA-generated Verilog module, extracting all interface elements.
#[allow(clippy::too_many_lines, reason = "main parser entrypoint; splitting would fragment the parse loop")]
pub fn parse_module(source: &str) -> Result<VerilogModule, ParseError> {
    let work = source.trim();

    // Extract leading attributes before the module keyword.
    let (module_start, leading_pragmas) = find_module_with_pragmas(work)?;
    let header_input = &work[module_start..];

    // Extract module name for error context even if full header parse fails.
    let partial_name = header_input
        .strip_prefix("module")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("<unknown>")
        .trim_end_matches('(')
        .to_owned();

    // Parse module header.
    let (remaining, header) =
        module_header(header_input).map_err(|e| ParseError::ParseFailed {
            module: partial_name,
            message: format!("module header: {e}"),
        })?;
    let name = header.name;
    let port_names = header.port_names;

    // Parse body declarations until `endmodule`.
    let mut ports: Vec<Port> = header.ansi_ports;
    let mut parameters: Vec<Parameter> = header.params;
    let mut signals: Vec<Signal> = Vec::new();
    let mut pragmas: Vec<Pragma> = leading_pragmas;

    // Tracks the most recently parsed attribute so it can be attached
    // to the immediately following port declaration.
    let mut pending_pragma: Option<Pragma> = None;

    let mut cursor = remaining;
    loop {
        cursor = cursor.trim_start();
        if cursor.is_empty() || cursor.starts_with("endmodule") {
            break;
        }

        // Try to parse attributes — hold the last one for port attachment.
        if cursor.starts_with("(*") {
            if let Ok((after_attrs, mut attrs)) = attributes(cursor) {
                if !attrs.is_empty() {
                    // Keep the last attribute for potential port attachment.
                    pending_pragma = Some(attrs.last().unwrap().clone());
                    pragmas.append(&mut attrs);
                    cursor = after_attrs;
                    continue;
                }
            }
        }

        // Try parameter declaration — malformed parameter is a fatal error.
        if cursor.starts_with("parameter") {
            let (rest, param) = parameter_declaration(cursor).map_err(|_| {
                let line = cursor.lines().next().unwrap_or(cursor);
                ParseError::ParseFailed {
                    module: name.clone(),
                    message: format!("malformed parameter: {line}"),
                }
            })?;
            parameters.push(param);
            pending_pragma = None;
            cursor = rest;
            continue;
        }

        // Try port declaration — malformed port is a fatal error.
        if cursor.starts_with("input")
            || cursor.starts_with("output")
            || cursor.starts_with("inout")
        {
            let (rest, decls) = port_declarations(cursor).map_err(|_| {
                let line = cursor.lines().next().unwrap_or(cursor);
                ParseError::ParseFailed {
                    module: name.clone(),
                    message: format!("malformed port declaration: {line}"),
                }
            })?;
            let pragma = pending_pragma.take();
            for (i, (dir, w, pname)) in decls.into_iter().enumerate() {
                ports.push(Port {
                    name: pname,
                    direction: dir,
                    width: w,
                    // Attach pragma only to the first port in the group.
                    pragma: if i == 0 { pragma.clone() } else { None },
                });
            }
            cursor = rest;
            continue;
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
            pending_pragma = None;
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
            cursor = cursor
                .find("*/")
                .map_or("", |i| &cursor[i + 2..]);
            continue;
        }

        // Skip procedural blocks (always, initial, generate, etc.) entirely.
        if cursor.starts_with("always")
            || cursor.starts_with("initial")
            || cursor.starts_with("generate")
            || cursor.starts_with("function")
            || cursor.starts_with("task")
        {
            pending_pragma = None;
            cursor = skip_procedural_block(cursor);
            continue;
        }

        // Skip non-interface statements (assign, instance, etc.).
        pending_pragma = None;
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

/// Extract `(module_name, instance_name)` pairs for every submodule
/// instantiation in a Verilog source string.
///
/// Scans the module body for `module_name [#(...)] instance_name (...);`
/// statements at top-level nesting, skipping comments, string literals,
/// declarations, and procedural blocks. Keyword-starting identifiers
/// like `parameter`, `wire`, `input`, `output`, `inout`, `reg`,
/// `assign`, `always`, `initial`, `generate`, `function`, `task` are
/// excluded to avoid false positives.
#[must_use]
pub fn extract_instance_names(source: &str) -> Vec<(String, String)> {
    const EXCLUDED_FIRST_TOKENS: &[&str] = &[
        "module", "endmodule", "parameter", "wire", "reg", "input", "output", "inout",
        "assign", "always", "initial", "generate", "endgenerate", "function", "endfunction",
        "task", "endtask", "if", "else", "for", "while", "begin", "end", "case", "endcase",
        "default", "return", "localparam", "genvar", "integer", "real", "string", "logic",
        "typedef", "struct", "union", "enum",
    ];

    let mut out: Vec<(String, String)> = Vec::new();
    // Strip comments and string literals to simplify scanning.
    let stripped = strip_comments_and_strings(source);
    let bytes = stripped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        i = skip_whitespace(bytes, i);
        if i >= bytes.len() {
            break;
        }

        // Try to read an identifier.
        let (tok_end, first_tok) = read_ident(bytes, &stripped, i);
        if tok_end == i {
            // Not an identifier char — advance one byte to avoid stalling.
            i += 1;
            continue;
        }
        i = tok_end;
        if EXCLUDED_FIRST_TOKENS.contains(&first_tok) {
            // Skip to matching `end` for procedural / compound constructs,
            // or advance past the next semicolon for declarations.
            i = match first_tok {
                "always" | "initial" | "generate" | "function" | "task" | "if" | "for"
                | "while" | "case" | "begin" => skip_nested_block(&stripped, i),
                _ => advance_past_semicolon(&stripped, i),
            };
            continue;
        }

        // First identifier is a candidate module name. Expect optional
        // `#(...)` parameter override, then another identifier (instance
        // name), then `(...)` port list, then `;`.
        let saved = i;
        i = skip_whitespace(bytes, i);
        // Optional `#(...)`.
        if i < bytes.len() && bytes[i] == b'#' {
            i = skip_whitespace(bytes, i + 1);
            if i < bytes.len() && bytes[i] == b'(' {
                i = skip_balanced_parens(&stripped, i);
            } else {
                // Malformed, bail out.
                i = advance_past_semicolon(&stripped, saved);
                continue;
            }
        }
        i = skip_whitespace(bytes, i);

        // Instance name.
        let (inst_end, inst_slice) = read_ident(bytes, &stripped, i);
        if inst_end == i {
            i = advance_past_semicolon(&stripped, saved);
            continue;
        }
        let inst_name = inst_slice.to_owned();
        i = skip_whitespace(bytes, inst_end);
        // Expect `(...)`.
        if i >= bytes.len() || bytes[i] != b'(' {
            i = advance_past_semicolon(&stripped, saved);
            continue;
        }
        let j = skip_whitespace(bytes, skip_balanced_parens(&stripped, i));
        if j >= bytes.len() || bytes[j] != b';' {
            i = advance_past_semicolon(&stripped, saved);
            continue;
        }
        out.push((first_tok.to_owned(), inst_name));
        i = j + 1;
    }
    out
}

/// Advance `i` past any ASCII whitespace bytes.
fn skip_whitespace(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Read a Verilog identifier starting at `start`. Returns `(end, slice)` where
/// `end == start` if no identifier character was present.
fn read_ident<'a>(bytes: &[u8], source: &'a str, start: usize) -> (usize, &'a str) {
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    (i, &source[start..i])
}

/// Remove `// ...`, `/* ... */` comments and string literals from source.
fn strip_comments_and_strings(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        if bytes[i] == b'"' {
            out.push(' ');
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn skip_balanced_parens(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    if start >= bytes.len() || bytes[start] != b'(' {
        return start;
    }
    let mut depth: u32 = 0;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

fn advance_past_semicolon(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b';' {
        i += 1;
    }
    if i < bytes.len() {
        i += 1;
    }
    i
}

fn skip_nested_block(source: &str, start: usize) -> usize {
    // Simple heuristic: advance past the next semicolon outside of
    // nested parens. Sufficient for extracting top-level instantiations
    // after skipping procedural constructs in our generated grouped
    // modules.
    let bytes = source.as_bytes();
    let mut i = start;
    let mut depth: u32 = 0;
    let mut begin_depth: u32 = 0;
    let mut entered_block = false;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            depth += 1;
        } else if bytes[i] == b')' {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && bytes[i].is_ascii_alphabetic() {
            let tok_start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let tok = &source[tok_start..i];
            if tok == "begin" {
                begin_depth += 1;
                entered_block = true;
            } else if tok == "end" {
                begin_depth = begin_depth.saturating_sub(1);
                if entered_block && begin_depth == 0 {
                    return i;
                }
            }
            continue;
        } else if depth == 0 && bytes[i] == b';' && !entered_block {
            return i + 1;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
