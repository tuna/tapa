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
use crate::pragma::Pragma;
use crate::signal::{Signal, SignalKind};

fn sv_language() -> Language {
    tree_sitter_systemverilog::LANGUAGE.into()
}

fn sv_parser() -> Parser {
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

fn text_of<'a>(node: Node<'a>, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

// ── Width extraction ────────────────────────────────────────────────

fn extract_width(node: Node, src: &[u8]) -> Option<Width> {
    fn find_packed_dimension(node: Node) -> Option<Node> {
        if node.kind() == "packed_dimension" {
            return Some(node);
        }
        for i in 0..node.child_count() {
            if let Some(found) = find_packed_dimension(node.child(i).unwrap()) {
                return Some(found);
            }
        }
        None
    }

    let pd = find_packed_dimension(node)?;
    for i in 0..pd.child_count() {
        let child = pd.child(i).unwrap();
        if child.kind() == "constant_range" {
            return parse_constant_range(child, src);
        }
    }
    None
}

fn parse_constant_range(node: Node, src: &[u8]) -> Option<Width> {
    let mut msb_str = None;
    let mut lsb_str = None;
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
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
    let mut pragmas = Vec::new();
    let mut direction = Direction::Input;
    let mut width: Option<Width> = None;
    let mut names: Vec<String> = Vec::new();

    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
            "attribute_instance" => {
                if let Some(p) = parse_attribute_text(text_of(child, src)) {
                    pragmas.push(p);
                }
            }
            "input_declaration" => {
                direction = Direction::Input;
                for j in 0..child.child_count() {
                    let c = child.child(j).unwrap();
                    if c.kind() == "net_port_type" || c.kind() == "data_type_or_implicit" {
                        width = extract_width(c, src);
                    }
                    if c.kind() == "list_of_port_identifiers" {
                        names.extend(extract_identifiers(c, src));
                    }
                }
            }
            "output_declaration" => {
                direction = Direction::Output;
                for j in 0..child.child_count() {
                    let c = child.child(j).unwrap();
                    if c.kind() == "net_port_type" || c.kind() == "data_type_or_implicit" {
                        width = extract_width(c, src);
                    }
                    if c.kind() == "list_of_port_identifiers" {
                        names.extend(extract_identifiers(c, src));
                    }
                }
            }
            "inout_declaration" => {
                direction = Direction::Inout;
                for j in 0..child.child_count() {
                    let c = child.child(j).unwrap();
                    if c.kind() == "net_port_type" || c.kind() == "data_type_or_implicit" {
                        width = extract_width(c, src);
                    }
                    if c.kind() == "list_of_port_identifiers" {
                        names.extend(extract_identifiers(c, src));
                    }
                }
            }
            _ => {}
        }
    }

    let pragma = pragmas.into_iter().last();
    names
        .into_iter()
        .map(|name| Port {
            name,
            direction,
            width: width.clone(),
            pragma: pragma.clone(),
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
) -> Vec<Port> {
    let mut direction = *last_dir;
    let mut width: Option<Width> = None;
    let mut name: Option<String> = None;
    let mut has_type_spec = false;

    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
            "net_port_header" => {
                has_type_spec = true;
                for j in 0..child.child_count() {
                    let c = child.child(j).unwrap();
                    if c.kind() == "port_direction" {
                        direction = parse_direction(text_of(c, src).trim());
                    }
                    if c.kind() == "net_port_type" {
                        width = extract_width(c, src);
                    }
                }
            }
            "variable_port_header" => {
                has_type_spec = true;
                for j in 0..child.child_count() {
                    let c = child.child(j).unwrap();
                    if c.kind() == "port_direction" {
                        direction = parse_direction(text_of(c, src).trim());
                    }
                    if c.kind() == "variable_port_type" {
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

    if has_type_spec || width.is_some() {
        *last_dir = direction;
        *last_width = width;
    }
    let final_width = last_width.clone();

    if let Some(name) = name {
        vec![Port {
            name,
            direction,
            width: final_width,
            pragma: None, // pragma handled by caller
        }]
    } else {
        Vec::new()
    }
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
        for i in 0..node.child_count() {
            walk(node.child(i).unwrap(), src, ids);
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

    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "data_type_or_implicit" || child.kind() == "data_type" {
            width = extract_width(child, src).or(width);
        }
        if child.kind() == "list_of_param_assignments" {
            for j in 0..child.child_count() {
                let c = child.child(j).unwrap();
                if c.kind() == "param_assignment" {
                    let mut name = None;
                    let mut default_str = None;
                    for k in 0..c.child_count() {
                        let d = c.child(k).unwrap();
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

// ── Attribute parsing ───────────────────────────────────────────────

fn parse_attribute_text(text: &str) -> Option<Pragma> {
    if let Ok((_, pragma)) = super::attribute(text) {
        Some(pragma)
    } else if let Ok((_, pragma)) = super::raw_attribute(text) {
        Some(pragma)
    } else {
        None
    }
}

// ── Error detection ────────────────────────────────────────────────

pub enum ParseIssue {
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
    for i in 0..root.child_count() {
        if let Some(found) = first_issue(root.child(i).unwrap(), src) {
            return Some(found);
        }
    }
    None
}

// ── Signal extraction ───────────────────────────────────────────────

/// Whether a module-level declaration node is a wire/reg signal
/// declaration. Mirrors the old text-level `starts_with` gate: only
/// declarations whose text begins with `wire` or `reg` are signals;
/// `logic`/`integer`/procedural-local declarations are not.
fn signal_kind_of(node: Node, src: &[u8]) -> Option<SignalKind> {
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
fn has_error_node(node: Node) -> bool {
    node.kind() == "ERROR"
        || node.is_missing()
        || (0..node.child_count()).any(|i| has_error_node(node.child(i).unwrap()))
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
    for i in 0..node.child_count() {
        let list = node.child(i).unwrap();
        if list.kind() != list_kind {
            continue;
        }
        for j in 0..list.child_count() {
            let assignment = list.child(j).unwrap();
            if !matches!(
                assignment.kind(),
                "net_decl_assignment" | "variable_decl_assignment"
            ) {
                continue;
            }
            // The declared name is the first identifier child; later
            // identifiers belong to unpacked dimensions or initializers.
            for k in 0..assignment.child_count() {
                let child = assignment.child(k).unwrap();
                if matches!(child.kind(), "simple_identifier" | "escaped_identifier") {
                    names.push(text_of(child, src).to_owned());
                    break;
                }
            }
        }
    }
    names
}

/// Extract signals from a wire/reg declaration, rejecting malformed
/// declarations the same way the old nom parser did.
fn extract_signals(node: Node, src: &[u8], module: &str) -> Result<Vec<Signal>, ParseError> {
    let Some(kind) = signal_kind_of(node, src) else {
        return Ok(Vec::new());
    };
    let malformed = || {
        let line = text_of(node, src).lines().next().unwrap_or("");
        ParseError::ParseFailed {
            module: module.to_owned(),
            message: format!("malformed signal declaration: {line}"),
        }
    };
    if has_error_node(node) {
        return Err(malformed());
    }
    let names = declarator_names(node, src);
    if names.is_empty() {
        return Err(malformed());
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

/// Everything the body walk collects from one module, in source order.
struct BodyExtraction {
    signals: Vec<Signal>,
    pragmas: Vec<Pragma>,
}

/// Walk the direct children of the target `module_declaration`,
/// collecting body pragmas and top-level signal declarations.
///
/// Scope discipline: nested modules, generate regions, procedural
/// blocks, functions, and tasks are never descended into, so their
/// local declarations neither leak into `signals` nor into `pragmas`.
fn walk_module_body(
    mod_node: Node,
    header_node: Node,
    src: &[u8],
    module: &str,
) -> Result<BodyExtraction, ParseError> {
    fn visit(
        node: Node,
        src: &[u8],
        module: &str,
        out: &mut BodyExtraction,
    ) -> Result<(), ParseError> {
        match node.kind() {
            "attribute_instance" => {
                if let Some(p) = parse_attribute_text(text_of(node, src)) {
                    out.pragmas.push(p);
                }
            }
            // Unwrap one grouping level; port declarations carry their
            // leading attributes as direct children.
            "module_item" | "port_declaration" => {
                for i in 0..node.child_count() {
                    visit(node.child(i).unwrap(), src, module, out)?;
                }
            }
            "net_declaration" | "data_declaration" => {
                out.signals.append(&mut extract_signals(node, src, module)?);
            }
            // Malformed signals that broke the decl node entirely.
            "ERROR" => {
                let trimmed = text_of(node, src).trim_start();
                if trimmed.starts_with("wire") || trimmed.starts_with("reg") {
                    let line = trimmed.lines().next().unwrap_or("");
                    return Err(ParseError::ParseFailed {
                        module: module.to_owned(),
                        message: format!("malformed signal declaration: {line}"),
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut out = BodyExtraction {
        signals: Vec::new(),
        pragmas: Vec::new(),
    };
    for i in 0..mod_node.child_count() {
        let child = mod_node.child(i).unwrap();
        if child.id() == header_node.id() {
            continue;
        }
        visit(child, src, module, &mut out)?;
    }
    Ok(out)
}

// ── Module location & pragma preamble ───────────────────────────────

/// First `module_declaration` in document order, anywhere in the tree.
fn find_first_module(root: Node) -> Option<Node> {
    if root.kind() == "module_declaration" {
        return Some(root);
    }
    for i in 0..root.child_count() {
        if let Some(found) = find_first_module(root.child(i).unwrap()) {
            return Some(found);
        }
    }
    None
}

/// Whether a node sitting before the module is trivia the module scan
/// may skip: comments, attributes, and compiler directives. Mirrors the
/// old text scan, which refused the parse on any other leading token.
fn is_preamble_trivia(node: Node, src: &[u8]) -> bool {
    let text = text_of(node, src).trim_start();
    matches!(
        node.kind(),
        "one_line_comment" | "block_comment" | "attribute_instance"
    ) || text.starts_with('`')
        || (node.kind() == "ERROR" && text.starts_with("(*"))
}

/// Pragmas attached to the module itself: attribute instances that
/// precede the `module` keyword, whether they are attached at the top
/// level or folded into the module header by the grammar.
fn leading_pragmas(root: Node, mod_node: Node, header: Node, src: &[u8]) -> Vec<Pragma> {
    let mut pragmas = Vec::new();
    for i in 0..root.child_count() {
        let child = root.child(i).unwrap();
        if child.end_byte() > mod_node.start_byte() {
            break;
        }
        if child.kind() == "attribute_instance" {
            if let Some(p) = parse_attribute_text(text_of(child, src)) {
                pragmas.push(p);
            }
        } else if child.kind() == "ERROR" {
            // Raw `(* ... *)` the grammar could not structure at all.
            if let Some(p) = parse_attribute_text(text_of(child, src).trim_start()) {
                pragmas.push(p);
            }
        }
    }
    for i in 0..header.child_count() {
        let child = header.child(i).unwrap();
        if child.kind() == "module_keyword" {
            break;
        }
        if child.kind() == "attribute_instance" {
            if let Some(p) = parse_attribute_text(text_of(child, src)) {
                pragmas.push(p);
            }
        }
    }
    pragmas
}

/// Extract ANSI header ports and non-ANSI body ports.
fn extract_ports(header: Node, mod_node: Node, src: &[u8]) -> (Vec<Port>, Vec<String>) {
    let mut ports = Vec::new();
    let mut port_names = Vec::new();

    // ANSI header ports
    for i in 0..header.child_count() {
        let child = header.child(i).unwrap();
        if child.kind() == "list_of_port_declarations" {
            let mut pending_pragma: Option<Pragma> = None;
            let mut last_dir = Direction::Input;
            let mut last_width: Option<Width> = None;
            for j in 0..child.child_count() {
                let c = child.child(j).unwrap();
                match c.kind() {
                    "attribute_instance" => {
                        if let Some(p) = parse_attribute_text(text_of(c, src)) {
                            pending_pragma = Some(p);
                        }
                    }
                    "ansi_port_declaration" => {
                        let mut port_list =
                            extract_ansi_port(c, src, &mut last_dir, &mut last_width);
                        if let Some(ref p) = pending_pragma {
                            if let Some(first) = port_list.first_mut() {
                                first.pragma = Some(p.clone());
                            }
                        }
                        ports.extend(port_list);
                        pending_pragma = None;
                    }
                    _ => {}
                }
            }
        }
        if child.kind() == "list_of_ports" {
            for j in 0..child.child_count() {
                let c = child.child(j).unwrap();
                if c.kind() == "port" {
                    for k in 0..c.child_count() {
                        let d = c.child(k).unwrap();
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
        let port_node = m.captures.first().unwrap().node;
        ports.extend(extract_ports_from_node(port_node, src));
    }
    (ports, port_names)
}

// ── Public API ──────────────────────────────────────────────────────

pub struct ModuleInfo {
    pub name: String,
    pub params: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub port_names: Vec<String>,
    pub signals: Vec<Signal>,
    pub pragmas: Vec<Pragma>,
}

/// Outcome of extracting a module with tree-sitter; the caller maps the
/// failure variants onto `ParseError` with text-level context.
pub enum ModuleParse {
    /// No `module_declaration` found anywhere in the tree.
    NoModule,
    /// Module found, but the header or module name was not extractable.
    HeaderFailed,
    /// An `ERROR` node matches a malformed port/parameter declaration.
    Issue(ParseIssue),
    /// Everything extracted.
    Ok(Box<ModuleInfo>),
}

/// Parse a Verilog module with tree-sitter and extract the name, ports,
/// parameters, signals, and pragmas.
pub fn parse_module_info(source: &str) -> Result<ModuleParse, ParseError> {
    let src = source.as_bytes();
    let mut parser = sv_parser();
    let Some(tree) = parser.parse(src, None) else {
        return Ok(ModuleParse::NoModule);
    };
    let root = tree.root_node();

    // Locate the target module, rejecting non-trivia content that the
    // old text scan would have rejected with `NoModuleFound`.
    let Some(mod_node) = find_first_module(root) else {
        return Ok(ModuleParse::NoModule);
    };
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
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
        return Ok(ModuleParse::Issue(issue));
    }

    let Some(header) = mod_node.child_by_field_name("header").or_else(|| {
        (0..mod_node.child_count())
            .map(|i| mod_node.child(i).unwrap())
            .find(|n| n.kind() == "module_nonansi_header" || n.kind() == "module_ansi_header")
    }) else {
        return Ok(ModuleParse::HeaderFailed);
    };

    let Some(name) = (0..header.child_count())
        .map(|i| header.child(i).unwrap())
        .find(|n| n.kind() == "simple_identifier")
        .map(|n| text_of(n, src).to_owned())
    else {
        return Ok(ModuleParse::HeaderFailed);
    };

    // Parameters (header + body)
    let mut params = Vec::new();
    let mut param_cursor = QueryCursor::new();
    let mut param_matches = param_cursor.matches(&Q_PARAMS, mod_node, src);
    while let Some(m) = param_matches.next() {
        let param_node = m.captures.first().unwrap().node;
        params.extend(extract_parameters_from_node(param_node, src));
    }

    let (ports, port_names) = extract_ports(header, mod_node, src);

    // Body signals and pragmas (module scope only).
    let body = walk_module_body(mod_node, header, src, &name)?;

    let mut pragmas = leading_pragmas(root, mod_node, header, src);
    pragmas.extend(body.pragmas);

    Ok(ModuleParse::Ok(Box::new(ModuleInfo {
        name,
        params,
        ports,
        port_names,
        signals: body.signals,
        pragmas,
    })))
}
