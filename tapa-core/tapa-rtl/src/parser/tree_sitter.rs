//! Tree-sitter helpers for Verilog parsing.
//!
//! Provides CST-based extraction for module headers, port declarations,
//! parameter declarations, and submodule instantiations.  Keeps the
//! traversal style consistent with `tapa-slotting/src/cpp_surgery.rs`.

use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use crate::expression::tokenize_expression;
use crate::param::Parameter;
use crate::port::{Direction, Port, Width};
use crate::pragma::Pragma;

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

static Q_MODULE: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&sv_language(), "(module_declaration) @mod").expect("Q_MODULE parses")
});

static Q_PORTS: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&sv_language(), "(port_declaration) @port").expect("Q_PORTS parses")
});

static Q_PARAMS: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&sv_language(), "(parameter_declaration) @param").expect("Q_PARAMS parses")
});

static Q_INSTANCES: LazyLock<Query> = LazyLock::new(|| {
    Query::new(&sv_language(), "(module_instantiation) @inst").expect("Q_INSTANCES parses")
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
        *last_width = width.clone();
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
        "input" => Direction::Input,
        "output" => Direction::Output,
        "inout" => Direction::Inout,
        _ => Direction::Input,
    }
}

fn extract_identifiers(node: Node, src: &[u8]) -> Vec<String> {
    let mut ids = Vec::new();
    fn walk(node: Node, src: &[u8], ids: &mut Vec<String>) {
        if node.kind() == "simple_identifier" || node.kind() == "escaped_identifier" {
            ids.push(text_of(node, src).to_owned());
        }
        for i in 0..node.child_count() {
            walk(node.child(i).unwrap(), src, ids);
        }
    }
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
                            "constant_param_expression" | "constant_mintypmax_expression"
                            | "constant_expression" | "constant_primary" | "primary_literal"
                            | "integral_number" | "decimal_number" | "octal_number"
                            | "binary_number" | "hex_number" | "real_number"
                            | "unbased_unsized_literal" => {
                                if default_str.is_none() {
                                    default_str = Some(text_of(d, src).trim().to_owned());
                                }
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

// ── Instance extraction ─────────────────────────────────────────────

fn extract_instances_from_node(node: Node, src: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut module_name = None;
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
            "simple_identifier" | "hierarchical_identifier" => {
                if module_name.is_none() {
                    module_name = Some(text_of(child, src).to_owned());
                }
            }
            "hierarchical_instance" => {
                for j in 0..child.child_count() {
                    let c = child.child(j).unwrap();
                    if c.kind() == "name_of_instance" {
                        for k in 0..c.child_count() {
                            let d = c.child(k).unwrap();
                            if d.kind() == "simple_identifier"
                                || d.kind() == "hierarchical_identifier"
                            {
                                let inst_name = text_of(d, src).to_owned();
                                if let Some(ref mn) = module_name {
                                    out.push((mn.clone(), inst_name));
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
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
/// declarations the old nom parser would have validated.
///
/// Ignores ERROR nodes inside procedural blocks, assignments, and
/// instantiations — those are skipped by the body loop anyway.
pub fn first_parse_error(source: &str) -> Option<ParseIssue> {
    let mut parser = sv_parser();
    let tree = parser.parse(source.as_bytes(), None)?;
    let root = tree.root_node();
    let src = source.as_bytes();

    fn walk(node: Node, src: &[u8]) -> Option<ParseIssue> {
        if node.kind() == "ERROR" || node.is_error() {
            let text = text_of(node, src);
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
            // ignored because the old parser skipped those constructs.
            return None;
        }
        for i in 0..node.child_count() {
            if let Some(found) = walk(node.child(i).unwrap(), src) {
                return Some(found);
            }
        }
        None
    }

    walk(root, src)
}

// ── Public API ──────────────────────────────────────────────────────

pub struct ModuleInfo {
    pub name: String,
    pub header_end: usize,
    pub params: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub port_names: Vec<String>,
}

/// Parse a Verilog module with tree-sitter and extract header info, ports,
/// and parameters.  Returns `None` if no `module_declaration` is found.
pub fn parse_module_info(source: &str) -> Option<ModuleInfo> {
    let mut parser = sv_parser();
    let tree = parser.parse(source.as_bytes(), None)?;
    let root = tree.root_node();
    let src = source.as_bytes();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&Q_MODULE, root, src);
    let m = matches.next()?;
    let mod_node = m.captures.first()?.node;

    let header = mod_node
        .child_by_field_name("header")
        .or_else(|| {
            (0..mod_node.child_count())
                .map(|i| mod_node.child(i).unwrap())
                .find(|n| {
                    n.kind() == "module_nonansi_header" || n.kind() == "module_ansi_header"
                })
        })?;

    let name = (0..header.child_count())
        .map(|i| header.child(i).unwrap())
        .find(|n| n.kind() == "simple_identifier")
        .map(|n| text_of(n, src).to_owned())?;

    let header_end = header.end_byte();

    // Parameters (header + body)
    let mut params = Vec::new();
    let mut param_cursor = QueryCursor::new();
    let mut param_matches = param_cursor.matches(&Q_PARAMS, mod_node, src);
    while let Some(m) = param_matches.next() {
        let param_node = m.captures.first().unwrap().node;
        params.extend(extract_parameters_from_node(param_node, src));
    }

    // Ports
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
                        let mut port_list = extract_ansi_port(c, src, &mut last_dir, &mut last_width);
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

    Some(ModuleInfo {
        name,
        header_end,
        params,
        ports,
        port_names,
    })
}

/// Extract `(module_name, instance_name)` pairs using tree-sitter.
pub fn extract_instance_names(source: &str) -> Vec<(String, String)> {
    let mut parser = sv_parser();
    let Some(tree) = parser.parse(source.as_bytes(), None) else {
        return Vec::new();
    };
    let root = tree.root_node();
    let src = source.as_bytes();

    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&Q_INSTANCES, root, src);
    while let Some(m) = matches.next() {
        let inst_node = m.captures.first().unwrap().node;
        out.extend(extract_instances_from_node(inst_node, src));
    }
    out
}
