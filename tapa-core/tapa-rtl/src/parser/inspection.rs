//! Conservative inspection of original RTL through a transient CST.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Query, QueryCursor};

use super::tree_sitter::{
    children_of, find_first_module, has_error_node, module_header, signal_kind_of, sv_language,
    sv_parser, text_of,
};

static Q_DRIVER_USES: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &sv_language(),
        "[(net_lvalue) (variable_lvalue) (nonrange_variable_lvalue)] @lhs
         [(module_instantiation) (interface_instantiation) (program_instantiation)
          (checker_instantiation) (gate_instantiation) (udp_instantiation)
          (subroutine_call) (function_subroutine_call)
          (net_decl_assignment) (variable_decl_assignment)] @ambiguous",
    )
    .expect("Q_DRIVER_USES parses")
});

/// Plain zero literals, with optional parentheses; no constant folding.
fn is_zero_expression(node: Node, src: &[u8]) -> bool {
    match node.kind() {
        "unbased_unsized_literal" => text_of(node, src) == "'0",
        "unsigned_number" => text_of(node, src).chars().all(|ch| ch == '0' || ch == '_'),
        "decimal_number" | "binary_number" | "octal_number" | "hex_number" => {
            node.child_by_field_name("value").map_or_else(
                || {
                    node.named_child(0)
                        .is_some_and(|child| is_zero_expression(child, src))
                },
                |value| text_of(value, src).chars().all(|ch| ch == '0' || ch == '_'),
            )
        }
        "expression"
        | "primary"
        | "primary_literal"
        | "integral_number"
        | "mintypmax_expression" => {
            let mut cursor = node.walk();
            let mut children = node
                .named_children(&mut cursor)
                .filter(|child| !matches!(child.kind(), "one_line_comment" | "block_comment"));
            let Some(child) = children.next() else {
                return false;
            };
            children.next().is_none() && is_zero_expression(child, src)
        }
        _ => false,
    }
}

/// Only assignments directly in the module, without delay or drive strength,
/// establish a constant driver. Generate and procedural scopes stay unknown.
fn is_module_zero_assignment(lhs: Node, module: Node, src: &[u8]) -> bool {
    let Some(assignment) = lhs.parent().filter(|node| node.kind() == "net_assignment") else {
        return false;
    };
    let Some(rhs) = assignment.named_child(1) else {
        return false;
    };
    if !is_zero_expression(rhs, src) {
        return false;
    }
    let mut node = assignment;
    while let Some(parent) = node.parent() {
        if parent.id() == module.id() {
            return true;
        }
        if !matches!(
            parent.kind(),
            "list_of_net_assignments" | "continuous_assign" | "module_item"
        ) || (parent.kind() == "continuous_assign" && parent.named_child_count() != 1)
        {
            return false;
        }
        node = parent;
    }
    false
}

fn record_identifiers(
    node: Node,
    src: &[u8],
    proven_name: Option<&str>,
    names: &mut BTreeMap<String, bool>,
) {
    if node.kind() == "simple_identifier" {
        let name = text_of(node, src);
        let proven = proven_name == Some(name);
        names
            .entry(name.to_owned())
            .and_modify(|previous| *previous &= proven)
            .or_insert(proven);
    } else {
        for child in children_of(node) {
            record_identifiers(child, src, proven_name, names);
        }
    }
}

/// Collect names with only plain module-scope zero assignments. Other lvalues,
/// initializers, instance connections and subroutine arguments stay ambiguous.
pub fn unconditionally_zero_driven_names(source: &str) -> BTreeSet<String> {
    let src = source.as_bytes();
    let mut parser = sv_parser();
    let Some(tree) = parser.parse(src, None) else {
        return BTreeSet::new();
    };
    let Some(module) = find_first_module(tree.root_node()) else {
        return BTreeSet::new();
    };
    if has_error_node(module) {
        return BTreeSet::new();
    }

    let mut names = BTreeMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&Q_DRIVER_USES, module, src);
    while let Some(found) = matches.next() {
        let lhs = found.captures()[0].node;
        if matches!(
            lhs.kind(),
            "net_decl_assignment" | "variable_decl_assignment"
        ) && !children_of(lhs).any(|child| child.kind() == "=")
        {
            continue;
        }
        let proven_name = (found.captures()[0].index == 0
            && is_module_zero_assignment(lhs, module, src))
        .then(|| text_of(lhs, src));
        record_identifiers(lhs, src, proven_name, &mut names);
    }
    names
        .into_iter()
        .filter_map(|(name, proven)| proven.then_some(name))
        .collect()
}

/// Declarations reconstructed by the mutable interface, plus their attributes
/// and surrounding comments. Everything after this contiguous prefix remains
/// original source text, including nested declarations and instance closures.
/// A `localparam` is not reconstructed, so the prefix ends at one.
fn is_declaration_prefix(node: Node, src: &[u8]) -> bool {
    match node.kind() {
        "port_declaration"
        | "parameter_declaration"
        | "attribute_instance"
        | "one_line_comment"
        | "block_comment" => true,
        "net_declaration" | "data_declaration" => signal_kind_of(node, src).is_some(),
        "module_item" => {
            let mut cursor = node.walk();
            let is_prefix = node
                .named_children(&mut cursor)
                .all(|child| is_declaration_prefix(child, src));
            is_prefix
        }
        _ => false,
    }
}

/// Locate the original body and trailing source with the same CST grammar
/// used for the interface: the body runs from the end of the declaration
/// prefix to the module's `endmodule`, and the tail holds everything after
/// the first `module_declaration` — later modules and trailing text, except
/// the terminator's own same-line comment, which emission rewrites.
/// No body nodes or rewritten source are retained between inspections.
pub fn original_module_text(source: &str) -> (&str, &str) {
    let empty = ("", "");
    let src = source.as_bytes();
    let mut parser = sv_parser();
    let Some(tree) = parser.parse(src, None) else {
        return empty;
    };
    let root = tree.root_node();
    let Some(module) = find_first_module(root) else {
        return empty;
    };
    let Some(header) = module_header(module) else {
        return empty;
    };
    let body_end = children_of(module)
        .find(|node| node.kind() == "endmodule")
        .map_or_else(|| module.end_byte(), |node| node.start_byte());
    let mut body_start = header.end_byte();
    for child in children_of(module) {
        if child.end_byte() <= header.end_byte() {
            continue;
        }
        if child.kind() == "endmodule" || !is_declaration_prefix(child, src) {
            break;
        }
        if !matches!(
            child.kind(),
            "attribute_instance" | "one_line_comment" | "block_comment"
        ) {
            body_start = child.end_byte();
        }
    }
    // The comment on the terminator's own line is replaced by the emitted
    // `endmodule //name`; every later node starts the preserved tail.
    let tail_start = children_of(root)
        .filter(|node| node.start_byte() >= module.end_byte())
        .find(|node| {
            !(node.start_position().row == module.end_position().row
                && matches!(node.kind(), "one_line_comment" | "block_comment"))
        })
        .map_or_else(|| source.len(), |node| node.start_byte());
    (&source[body_start..body_end], &source[tail_start..])
}
