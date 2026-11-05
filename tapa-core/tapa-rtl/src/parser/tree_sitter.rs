//! Tree-sitter helpers for Verilog parsing.
//!
//! Provides CST-based extraction for module headers, port declarations,
//! and parameter declarations.

use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use crate::error::ParseError;
use crate::expression::tokenize_expression;
use crate::param::Parameter;
use crate::port::{Direction, Port, Width};
use crate::signal::{Signal, SignalKind};

pub(super) fn sv_language() -> Language {
    tree_sitter_systemverilog::LANGUAGE.into()
}

pub(super) fn sv_parser() -> Parser {
    let mut p = Parser::new();
    p.set_language(&sv_language())
        .expect("systemverilog grammar loads");
    p
}

// ── Queries ─────────────────────────────────────────────────────────

static Q_PORTS: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&sv_language(), "(port_declaration) @port").expect("Q_PORTS parses")
});

static Q_PARAMS: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&sv_language(), "(parameter_declaration) @param").expect("Q_PARAMS parses")
});

// ── Text helpers ────────────────────────────────────────────────────

pub(super) fn text_of<'a>(node: Node<'a>, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

/// A node's children, without the cursor tree-sitter's `children` needs.
pub(super) fn children_of(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    (0..node.child_count()).map(move |i| node.child(i).unwrap())
}

// ── Width extraction ────────────────────────────────────────────────

fn extract_width(node: Node, src: &[u8]) -> Option<Width> {
    fn find_packed_dimension(node: Node) -> Option<Node> {
        if node.kind() == "packed_dimension" {
            return Some(node);
        }
        for child in children_of(node) {
            if let Some(found) = find_packed_dimension(child) {
                return Some(found);
            }
        }
        None
    }

    let pd = find_packed_dimension(node)?;
    for child in children_of(pd) {
        if child.kind() == "constant_range" {
            return parse_constant_range(child, src);
        }
    }
    None
}

fn parse_constant_range(node: Node, src: &[u8]) -> Option<Width> {
    let mut msb_str = None;
    let mut lsb_str = None;
    for child in children_of(node) {
        let text = text_of(child, src).trim();
        if text == ":" {
            continue;
        }
        if msb_str.is_none() {
            msb_str = Some(text);
        } else {
            lsb_str = Some(text);
        }
    }
    Some(Width {
        msb: tokenize_expression(msb_str?),
        lsb: tokenize_expression(lsb_str?),
    })
}

// ── Port extraction ─────────────────────────────────────────────────

/// Extract ports from a `port_declaration` node (body port, non-ANSI).
fn extract_ports_from_node(node: Node, src: &[u8]) -> Vec<Port> {
    let mut direction = Direction::Input;
    let mut width: Option<Width> = None;
    let mut names: Vec<String> = Vec::new();

    for child in children_of(node) {
        if matches!(
            child.kind(),
            "input_declaration" | "output_declaration" | "inout_declaration"
        ) {
            direction = match child.kind() {
                "input_declaration" => Direction::Input,
                "output_declaration" => Direction::Output,
                _ => Direction::Inout,
            };
            for c in children_of(child) {
                if c.kind() == "net_port_type" || c.kind() == "data_type_or_implicit" {
                    width = extract_width(c, src);
                }
                if c.kind() == "list_of_port_identifiers" {
                    names.extend(extract_identifiers(c, src));
                }
            }
        }
    }

    names
        .into_iter()
        .map(|name| Port {
            name,
            direction,
            width: width.clone(),
        })
        .collect()
}

/// Extract ports from an `ansi_port_declaration` node (header port, ANSI).
/// Inherits `direction` and `width` from the previous port when the current
/// port has no explicit type specification, matching Verilog semantics.
fn extract_ansi_port(
    node: Node,
    src: &[u8],
    last_dir: &mut Direction,
    last_width: &mut Option<Width>,
) -> Option<Port> {
    let mut direction = *last_dir;
    let mut width: Option<Width> = None;
    let mut name: Option<String> = None;
    let mut has_type_spec = false;

    for child in children_of(node) {
        match child.kind() {
            "net_port_header" | "variable_port_header" => {
                has_type_spec = true;
                for c in children_of(child) {
                    if c.kind() == "port_direction" {
                        direction = parse_direction(text_of(c, src).trim());
                    }
                    if c.kind() == "net_port_type" || c.kind() == "variable_port_type" {
                        width = extract_width(c, src);
                    }
                }
            }
            "port_direction" => {
                has_type_spec = true;
                direction = parse_direction(text_of(child, src).trim());
            }
            "port_identifier" | "simple_identifier" => {
                name = Some(text_of(child, src).to_owned());
            }
            _ => {}
        }
    }

    if has_type_spec {
        *last_dir = direction;
        *last_width = width;
    }
    let final_width = last_width.clone();

    name.map(|name| Port {
        name,
        direction,
        width: final_width,
    })
}

fn parse_direction(text: &str) -> Direction {
    match text {
        "output" => Direction::Output,
        "inout" => Direction::Inout,
        _ => Direction::Input,
    }
}

fn extract_identifiers(node: Node, src: &[u8]) -> Vec<String> {
    fn walk(node: Node, src: &[u8], ids: &mut Vec<String>) {
        if node.kind() == "simple_identifier" || node.kind() == "escaped_identifier" {
            ids.push(text_of(node, src).to_owned());
        }
        for child in children_of(node) {
            walk(child, src, ids);
        }
    }
    let mut ids = Vec::new();
    walk(node, src, &mut ids);
    ids
}

// ── Parameter extraction ────────────────────────────────────────────

fn extract_parameters_from_node(node: Node, src: &[u8]) -> Vec<Parameter> {
    let mut width: Option<Width> = None;
    let mut params = Vec::new();

    for child in children_of(node) {
        if child.kind() == "data_type_or_implicit" || child.kind() == "data_type" {
            width = extract_width(child, src).or(width);
        }
        if child.kind() == "list_of_param_assignments" {
            for c in children_of(child) {
                if c.kind() == "param_assignment" {
                    let mut name = None;
                    let mut default_str = None;
                    for d in children_of(c) {
                        match d.kind() {
                            "simple_identifier" => name = Some(text_of(d, src).to_owned()),
                            "constant_param_expression"
                            | "constant_mintypmax_expression"
                            | "constant_expression"
                            | "constant_primary"
                            | "primary_literal"
                            | "integral_number"
                            | "decimal_number"
                            | "octal_number"
                            | "binary_number"
                            | "hex_number"
                            | "real_number"
                            | "unbased_unsized_literal"
                                if default_str.is_none() =>
                            {
                                default_str = Some(text_of(d, src).trim().to_owned());
                            }
                            _ => {}
                        }
                    }
                    if let (Some(n), Some(d)) = (name, default_str) {
                        params.push(Parameter {
                            name: n,
                            default: tokenize_expression(&d),
                            width: width.clone(),
                        });
                    }
                }
            }
        }
    }
    params
}

// ── Error detection ────────────────────────────────────────────────

pub(super) enum ParseIssue {
    MalformedPort,
    MalformedParameter,
}

/// Scan the tree-sitter CST for `ERROR` nodes that correspond to
/// malformed port/parameter declarations.
///
/// Ignores ERROR nodes inside procedural blocks, assignments, and
/// instantiations — those are skipped by the body walk anyway.
fn first_issue(root: Node, src: &[u8]) -> Option<ParseIssue> {
    if root.kind() == "ERROR" || root.is_error() {
        let text = text_of(root, src);
        let trimmed = text.trim_start();
        if trimmed.starts_with("parameter") {
            return Some(ParseIssue::MalformedParameter);
        }
        if trimmed.starts_with("input")
            || trimmed.starts_with("output")
            || trimmed.starts_with("inout")
        {
            return Some(ParseIssue::MalformedPort);
        }
        // Other ERROR nodes (inside procedural blocks, etc.) are
        // ignored: only the module interface is validated here.
        return None;
    }
    for child in children_of(root) {
        if let Some(found) = first_issue(child, src) {
            return Some(found);
        }
    }
    None
}

// ── Signal extraction ───────────────────────────────────────────────

/// Whether a module-level declaration node is a wire/reg signal
/// declaration: only declarations whose text begins with `wire` or
/// `reg` are signals; `logic`/`integer`/procedural-local declarations
/// are not.
pub(super) fn signal_kind_of(node: Node, src: &[u8]) -> Option<SignalKind> {
    if !matches!(node.kind(), "net_declaration" | "data_declaration") {
        return None;
    }
    let text = text_of(node, src).trim_start();
    if text.starts_with("wire") {
        Some(SignalKind::Wire)
    } else if text.starts_with("reg") {
        Some(SignalKind::Reg)
    } else {
        None
    }
}

/// True when the node subtree contains an `ERROR` or `MISSING` node.
pub(super) fn has_error_node(node: Node) -> bool {
    node.kind() == "ERROR" || node.is_missing() || children_of(node).any(has_error_node)
}

/// Declared identifier of each declarator in a wire/reg declaration,
/// e.g. `[a, b, c]` for `wire [3:0] a, b = f(), c [0:1];`.
fn declarator_names(node: Node, src: &[u8]) -> Vec<String> {
    let list_kind = if node.kind() == "net_declaration" {
        "list_of_net_decl_assignments"
    } else {
        "list_of_variable_decl_assignments"
    };
    let mut names = Vec::new();
    for list in children_of(node) {
        if list.kind() != list_kind {
            continue;
        }
        for assignment in children_of(list) {
            if !matches!(
                assignment.kind(),
                "net_decl_assignment" | "variable_decl_assignment"
            ) {
                continue;
            }
            // The declared name is the first identifier child; later
            // identifiers belong to unpacked dimensions or initializers.
            for child in children_of(assignment) {
                if matches!(child.kind(), "simple_identifier" | "escaped_identifier") {
                    names.push(text_of(child, src).to_owned());
                    break;
                }
            }
        }
    }
    names
}

/// `ParseFailed` for a malformed wire/reg declaration node.
fn malformed_signal(module: &str, node: Node, src: &[u8]) -> ParseError {
    let line = text_of(node, src).trim_start().lines().next().unwrap_or("");
    ParseError::ParseFailed {
        module: module.to_owned(),
        message: format!("malformed signal declaration: {line}"),
    }
}

/// Extract signals from a wire/reg declaration, rejecting malformed
/// declarations.
fn extract_signals(node: Node, src: &[u8], module: &str) -> Result<Vec<Signal>, ParseError> {
    let Some(kind) = signal_kind_of(node, src) else {
        return Ok(Vec::new());
    };
    if has_error_node(node) {
        return Err(malformed_signal(module, node, src));
    }
    let names = declarator_names(node, src);
    if names.is_empty() {
        return Err(malformed_signal(module, node, src));
    }
    let width = extract_width(node, src);
    Ok(names
        .into_iter()
        .map(|name| Signal {
            name,
            kind,
            width: width.clone(),
            attribute: None,
        })
        .collect())
}

// ── Module body walk ────────────────────────────────────────────────

/// Walk the direct children of the target `module_declaration`,
/// collecting top-level signal declarations.
///
/// Scope discipline: nested modules, generate regions, procedural
/// blocks, functions, and tasks are never descended into, so their
/// local declarations never leak into `signals`.
fn walk_module_body(
    mod_node: Node,
    header_node: Node,
    src: &[u8],
    module: &str,
) -> Result<Vec<Signal>, ParseError> {
    fn visit(
        node: Node,
        src: &[u8],
        module: &str,
        out: &mut Vec<Signal>,
    ) -> Result<(), ParseError> {
        match node.kind() {
            // Unwrap one grouping level.
            "module_item" => {
                for child in children_of(node) {
                    visit(child, src, module, out)?;
                }
            }
            "net_declaration" | "data_declaration" => {
                out.append(&mut extract_signals(node, src, module)?);
            }
            // Malformed signals that broke the decl node entirely.
            "ERROR" => {
                let trimmed = text_of(node, src).trim_start();
                if trimmed.starts_with("wire") || trimmed.starts_with("reg") {
                    return Err(malformed_signal(module, node, src));
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut signals = Vec::new();
    for child in children_of(mod_node) {
        if child.id() == header_node.id() {
            continue;
        }
        visit(child, src, module, &mut signals)?;
    }
    Ok(signals)
}

// ── Module location & preamble trivia ───────────────────────────────

/// First `module_declaration` in document order, anywhere in the tree.
pub(super) fn find_first_module(root: Node) -> Option<Node> {
    if root.kind() == "module_declaration" {
        return Some(root);
    }
    for child in children_of(root) {
        if let Some(found) = find_first_module(child) {
            return Some(found);
        }
    }
    None
}

pub(super) fn module_header(module: Node) -> Option<Node> {
    module.child_by_field_name("header").or_else(|| {
        children_of(module)
            .find(|node| matches!(node.kind(), "module_nonansi_header" | "module_ansi_header"))
    })
}

/// Whether a node sitting before the module is trivia the module scan
/// may skip: comments, attributes, and compiler directives. Any other
/// leading token rejects the parse.
fn is_preamble_trivia(node: Node, src: &[u8]) -> bool {
    let text = text_of(node, src).trim_start();
    matches!(
        node.kind(),
        "one_line_comment" | "block_comment" | "attribute_instance"
    ) || text.starts_with('`')
        || (node.kind() == "ERROR" && text.starts_with("(*"))
}

/// Name from a header too broken for tree-sitter to recover a
/// `module_declaration`: the first non-trivia node must begin with the
/// `module` keyword.
fn broken_header_name(root: Node, src: &[u8]) -> Option<String> {
    let first = children_of(root).find(|node| !is_preamble_trivia(*node, src))?;
    let rest = text_of(first, src).trim_start().strip_prefix("module")?;
    if rest
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    rest.split_whitespace()
        .next()
        .map(|name| name.trim_end_matches('(').to_owned())
}

/// Extract ANSI header ports and non-ANSI body ports.
fn extract_ports(header: Node, mod_node: Node, src: &[u8]) -> (Vec<Port>, Vec<String>) {
    let mut ports = Vec::new();
    let mut port_names = Vec::new();

    // ANSI header ports
    for child in children_of(header) {
        if child.kind() == "list_of_port_declarations" {
            let mut last_dir = Direction::Input;
            let mut last_width: Option<Width> = None;
            for c in children_of(child) {
                if c.kind() == "ansi_port_declaration" {
                    if let Some(port) = extract_ansi_port(c, src, &mut last_dir, &mut last_width) {
                        ports.push(port);
                    }
                }
            }
        }
        if child.kind() == "list_of_ports" {
            for c in children_of(child) {
                if c.kind() == "port" {
                    for d in children_of(c) {
                        if d.kind() == "simple_identifier" || d.kind() == "escaped_identifier" {
                            port_names.push(text_of(d, src).to_owned());
                        }
                    }
                }
            }
        }
    }

    // Body ports (non-ANSI style with direction in body)
    let mut port_cursor = QueryCursor::new();
    let mut port_matches = port_cursor.matches(&Q_PORTS, mod_node, src);
    while let Some(m) = port_matches.next() {
        let port_node = m.captures().first().unwrap().node;
        ports.extend(extract_ports_from_node(port_node, src));
    }
    (ports, port_names)
}

// ── Parser entry point ──────────────────────────────────────────────

pub(super) struct ModuleInfo {
    pub name: String,
    pub params: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub port_names: Vec<String>,
    pub signals: Vec<Signal>,
}

/// Module name recovered from a possibly-broken header, for error context.
/// A module without a header node (e.g. `module M(.*);`) carries its name
/// directly on the declaration.
fn recovered_name(mod_node: Node, src: &[u8]) -> Option<String> {
    children_of(module_header(mod_node).unwrap_or(mod_node))
        .find(|n| n.kind() == "simple_identifier")
        .map(|n| text_of(n, src).to_owned())
}

/// Outcome of extracting a module with tree-sitter; the caller maps the
/// failure variants onto `ParseError`.
pub(super) enum ModuleParse {
    /// No `module_declaration` found anywhere in the tree.
    NoModule,
    /// Module found, but the header or module name was not extractable.
    HeaderFailed { name: Option<String> },
    /// An `ERROR` node matches a malformed port/parameter declaration.
    Issue {
        issue: ParseIssue,
        name: Option<String>,
    },
    /// Everything extracted.
    Ok(Box<ModuleInfo>),
}

/// Parse a Verilog module with tree-sitter and extract the name, ports,
/// parameters, and signals.
pub(super) fn parse_module_info(source: &str) -> Result<ModuleParse, ParseError> {
    let src = source.as_bytes();
    let mut parser = sv_parser();
    let Some(tree) = parser.parse(src, None) else {
        return Ok(ModuleParse::NoModule);
    };
    let root = tree.root_node();

    // Locate the target module, rejecting non-trivia content before it.
    let Some(mod_node) = find_first_module(root) else {
        return Ok(match broken_header_name(root, src) {
            Some(name) => ModuleParse::HeaderFailed { name: Some(name) },
            None => ModuleParse::NoModule,
        });
    };
    for child in children_of(root) {
        if child.end_byte() <= mod_node.start_byte() {
            if !is_preamble_trivia(child, src) {
                return Ok(ModuleParse::NoModule);
            }
        } else {
            break;
        }
    }

    // Reject sources that contain tree-sitter ERROR nodes (malformed
    // declarations that error-recovery would otherwise swallow).
    if let Some(issue) = first_issue(root, src) {
        return Ok(ModuleParse::Issue {
            issue,
            name: recovered_name(mod_node, src),
        });
    }

    let Some(header) = module_header(mod_node) else {
        return Ok(ModuleParse::HeaderFailed {
            name: recovered_name(mod_node, src),
        });
    };

    let Some(name) = children_of(header)
        .find(|n| n.kind() == "simple_identifier")
        .map(|n| text_of(n, src).to_owned())
    else {
        return Ok(ModuleParse::HeaderFailed { name: None });
    };

    // Parameters (header + body)
    let mut params = Vec::new();
    let mut param_cursor = QueryCursor::new();
    let mut param_matches = param_cursor.matches(&Q_PARAMS, mod_node, src);
    while let Some(m) = param_matches.next() {
        let param_node = m.captures().first().unwrap().node;
        params.extend(extract_parameters_from_node(param_node, src));
    }

    let (ports, port_names) = extract_ports(header, mod_node, src);

    // Body signals (module scope only).
    let signals = walk_module_body(mod_node, header, src, &name)?;

    Ok(ModuleParse::Ok(Box::new(ModuleInfo {
        name,
        params,
        ports,
        port_names,
        signals,
    })))
}
