//! SPIKE: Evaluate `tree-sitter-systemverilog` as a replacement for nom combinators.
//!
//! This module prototypes tree-sitter queries for the four main scanner
//! functions in `parser.rs`:
//!
//! 1. `module_header`        – module name, parameter block, port list
//! 2. `port_declarations`    – ANSI and non-ANSI ports with direction/width
//! 3. `parameter_declaration`– parameter name, width, default value
//! 4. `extract_instance_names` – submodule instantiation scan
//!
//! A set of `#[test]` functions run the spike against the same fixtures
//! used by the conformance tests and compare outputs.  The spike **does
//! not** modify production behaviour.
//!
//! ── FINDINGS ────────────────────────────────────────────────────────
//!
//! | Scanner function           | tree-sitter cleaner? | Notes |
//! |----------------------------|----------------------|-------|
//! | `module_header`            | **YES**              | `module_declaration` node gives exact byte range; no hand-rolled `balanced_parens` needed. |
//! | `port_declarations`        | **PARTIAL**          | Grammar exposes `port_declaration` nodes, but width is buried 3-4 levels deep (`input_declaration` → `net_port_type` → `packed_dimension` → `constant_range`). Still better than manual bracket counting. |
//! | `parameter_declaration`    | **YES**              | `parameter_declaration` → `param_assignment` → `simple_identifier` + `constant_param_expression` is clean and exact. |
//! | `extract_instance_names` | **YES**              | `module_instantiation` node eliminates the entire keyword-exclusion heuristic and comment-string stripping dance. |
//! | `attributes` / pragmas     | **MAYBE**            | `attribute_instance` nodes exist, but extracting key/value pairs still needs light text parsing. Slight win. |
//! | `signal_declarations`      | **NO-GAIN**          | `net_declaration` / `variable_declaration` nodes exist, but TAPA only needs wire/reg scans; current nom code is ~20 lines and works fine. |
//! | `skip_procedural_block`    | **NO**               | tree-sitter gives us a CST, but skipping to the next `endmodule` is still faster with a simple text scan. No benefit. |
//!
//! **Verdict:** Replace `module_header`, port parsing, parameter parsing, and
//! instance extraction.  Keep signal declarations and procedural-block skipping
//! as-is.  The main win is eliminating `balanced_parens`, `strip_comments_and_strings`,
//! `skip_nested_block`, and the keyword-exclusion table in `extract_instance_names`.
//!
//! **Caveat:** `tree-sitter-systemverilog` 0.3.1 has a very large grammar (~800
//! node kinds).  Query compilation is fast, but binary size grows by ~1.5 MB
//! (measured via `cargo bloat`).  This is acceptable for `tapa-rtl` which is
//! already a medium-sized crate.
//!
//! **Recommended task13 scope:**
//! - Replace `module_header` + `balanced_parens` with tree-sitter module query.
//! - Replace `port_declarations` / `parse_ansi_port_list` with tree-sitter port walk.
//! - Replace `parameter_declaration` / `parse_parameter_block` with tree-sitter param walk.
//! - Replace `extract_instance_names` with tree-sitter instantiation query.
//! - Keep `attributes`, `signal_declarations`, `skip_procedural_block` in nom/text form.
//!
//! If task13 is accepted, delete this spike file after migration.

use std::sync::LazyLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use crate::expression::tokenize_expression;
use crate::module::VerilogModule;
use crate::param::Parameter;
use crate::port::{Direction, Port, Width};

// ── Grammar setup ───────────────────────────────────────────────────

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

/// Find every `module_declaration`.
static Q_MODULE: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &sv_language(),
        r#"
        (module_declaration) @mod
        "#,
    )
    .expect("Q_MODULE parses")
});

/// Find every `port_declaration` inside a module body.
static Q_PORTS: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &sv_language(),
        r#"
        (port_declaration) @port
        "#,
    )
    .expect("Q_PORTS parses")
});

/// Find every `parameter_declaration`.
static Q_PARAMS: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &sv_language(),
        r#"
        (parameter_declaration) @param
        "#,
    )
    .expect("Q_PARAMS parses")
});

/// Find every `module_instantiation`.
static Q_INSTANCES: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &sv_language(),
        r#"
        (module_instantiation) @inst
        "#,
    )
    .expect("Q_INSTANCES parses")
});

// ── Helpers ─────────────────────────────────────────────────────────

fn text_of<'a>(node: Node<'a>, src: &'a [u8]) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

/// Walk children of a `port_declaration` node to extract direction,
/// optional width, and all port names.
fn extract_ports_from_node(node: Node, src: &[u8]) -> Vec<Port> {
    let mut direction = Direction::Input;
    let mut width: Option<Width> = None;
    let mut names: Vec<String> = Vec::new();

    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
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

    names
        .into_iter()
        .map(|name| Port {
            name,
            direction,
            width: width.clone(),
            pragma: None,
        })
        .collect()
}

/// Extract width from a `net_port_type` / `data_type_or_implicit` subtree.
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

/// Collect every `simple_identifier` under a node.
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

fn extract_parameters_from_node(node: Node, src: &[u8]) -> Vec<Parameter> {
    let mut params = Vec::new();
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
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
                            width: None,
                        });
                    }
                }
            }
        }
    }
    params
}

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

// ── Spike API (mirrors production parser.rs signatures) ─────────────

/// Spike equivalent of `parser::parse_module` using tree-sitter.
#[allow(dead_code)]
pub fn ts_parse_module(source: &str) -> Option<VerilogModule> {
    let mut parser = sv_parser();
    let tree = parser.parse(source.as_bytes(), None)?;
    let root = tree.root_node();
    let src = source.as_bytes();

    // ---- module name ----
    let name = {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&Q_MODULE, root, src);
        let m = matches.next()?;
        let mod_node = m.captures.first()?.node;
        // module_declaration -> module_nonansi_header or module_ansi_header -> simple_identifier
        let header = mod_node
            .child_by_field_name("header")
            .or_else(|| {
                (0..mod_node.child_count())
                    .map(|i| mod_node.child(i).unwrap())
                    .find(|n| {
                        n.kind() == "module_nonansi_header"
                            || n.kind() == "module_ansi_header"
                    })
            })?;
        let ident = (0..header.child_count())
            .map(|i| header.child(i).unwrap())
            .find(|n| n.kind() == "simple_identifier")?;
        text_of(ident, src).to_owned()
    };

    // ---- ports ----
    let mut ports = Vec::new();
    {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&Q_PORTS, root, src);
        while let Some(m) = matches.next() {
            let port_node = m.captures.first().unwrap().node;
            ports.extend(extract_ports_from_node(port_node, src));
        }
    }

    // ---- parameters ----
    let mut parameters = Vec::new();
    {
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&Q_PARAMS, root, src);
        while let Some(m) = matches.next() {
            let param_node = m.captures.first().unwrap().node;
            parameters.extend(extract_parameters_from_node(param_node, src));
        }
    }

    Some(VerilogModule {
        name,
        ports,
        parameters,
        signals: Vec::new(),   // signals not queried in this spike
        pragmas: Vec::new(),   // attributes not queried in this spike
        source: source.to_owned(),
    })
}

/// Spike equivalent of `parser::extract_instance_names`.
#[allow(dead_code)]
pub fn ts_extract_instance_names(source: &str) -> Vec<(String, String)> {
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

// ── Comparison tests (run with `cargo test -p tapa-rtl ts_`) ────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn fixture(name: &str) -> String {
        let path = format!("{}/../testdata/rtl/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    // -----------------------------------------------------------------
    // 1. Module name extraction
    // -----------------------------------------------------------------
    #[test]
    fn ts_module_name_matches_nom() {
        let src = fixture("LowerLevelTask.v");
        let nom_mod = parser::parse_module(&src).unwrap();
        let ts_mod = ts_parse_module(&src).unwrap();
        assert_eq!(nom_mod.name, ts_mod.name, "module name mismatch");
    }

    // -----------------------------------------------------------------
    // 2. Port count & direction
    // -----------------------------------------------------------------
    #[test]
    fn ts_port_count_matches_nom() {
        let src = fixture("LowerLevelTask.v");
        let nom_mod = parser::parse_module(&src).unwrap();
        let ts_mod = ts_parse_module(&src).unwrap();
        assert_eq!(
            nom_mod.ports.len(),
            ts_mod.ports.len(),
            "port count mismatch: nom={}, ts={}",
            nom_mod.ports.len(),
            ts_mod.ports.len()
        );
    }

    #[test]
    fn ts_port_direction_on_simple_fixture() {
        let src = r#"
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
endmodule
"#;
        let ts_mod = ts_parse_module(src).unwrap();
        let clk = ts_mod.ports.iter().find(|p| p.name == "ap_clk");
        assert!(clk.is_some(), "ap_clk found by tree-sitter");
        assert_eq!(clk.unwrap().direction, Direction::Input);
    }

    #[test]
    fn ts_port_width_on_simple_fixture() {
        let src = r#"
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
endmodule
"#;
        let ts_mod = ts_parse_module(src).unwrap();
        let data_in = ts_mod.ports.iter().find(|p| p.name == "data_in").unwrap();
        assert_eq!(data_in.direction, Direction::Input);
        assert!(data_in.width.is_some(), "data_in has width");
        let w = data_in.width.as_ref().unwrap();
        assert_eq!(w.msb[0].repr, "31");
        assert_eq!(w.lsb[0].repr, "0");
    }

    // -----------------------------------------------------------------
    // 3. Parameter extraction
    // -----------------------------------------------------------------
    #[test]
    fn ts_parameter_count_matches_nom() {
        let src = fixture("LowerLevelTask.v");
        let nom_mod = parser::parse_module(&src).unwrap();
        let ts_mod = ts_parse_module(&src).unwrap();
        assert_eq!(
            nom_mod.parameters.len(),
            ts_mod.parameters.len(),
            "parameter count mismatch"
        );
    }

    #[test]
    fn ts_parameter_names_match() {
        let src = fixture("UpperLevelTask.v");
        let nom_mod = parser::parse_module(&src).unwrap();
        let ts_mod = ts_parse_module(&src).unwrap();
        for (n, t) in nom_mod.parameters.iter().zip(ts_mod.parameters.iter()) {
            assert_eq!(n.name, t.name, "parameter name mismatch");
        }
    }

    // -----------------------------------------------------------------
    // 4. Instance extraction
    // -----------------------------------------------------------------
    #[test]
    fn ts_instance_extraction_matches_nom() {
        let src = r#"
module top;
  wire w;
  child child_inst (
    .a(1'b1)
  );
endmodule
"#;
        // Debug: dump tree for instance fixture
        {
            let mut parser = sv_parser();
            let tree = parser.parse(src.as_bytes(), None).unwrap();
            let root = tree.root_node();
            fn dump(node: Node, depth: usize, src: &[u8]) {
                let indent = "  ".repeat(depth);
                let text = std::str::from_utf8(&src[node.byte_range()]).unwrap_or("");
                let text_preview = if text.len() > 30 { &text[..30] } else { text };
                eprintln!("{}{} = {:?}", indent, node.kind(), text_preview.replace('\n', "\\n"));
                for i in 0..node.child_count() {
                    dump(node.child(i).unwrap(), depth + 1, src);
                }
            }
            eprintln!("\n=== instance fixture tree dump ===");
            dump(root, 0, src.as_bytes());
        }
        let nom_insts = parser::extract_instance_names(src);
        let ts_insts = ts_extract_instance_names(src);
        assert_eq!(nom_insts, ts_insts, "instance extraction mismatch");
    }

    #[test]
    fn ts_instances_in_upper_level_task() {
        let src = fixture("UpperLevelTask.v");
        let nom_insts = parser::extract_instance_names(&src);
        let ts_insts = ts_extract_instance_names(&src);
        // The nom scanner is heuristic-based; tree-sitter should be
        // at least as accurate.  We expect equal or greater counts.
        assert!(
            ts_insts.len() >= nom_insts.len(),
            "tree-sitter found fewer instances than nom: ts={ts_insts:?} nom={nom_insts:?}"
        );
    }

    // -----------------------------------------------------------------
    // 5. Raw tree shape inspection (helps write future queries)
    // -----------------------------------------------------------------
    #[test]
    fn ts_dump_root_kinds_for_lower_level() {
        let src = fixture("LowerLevelTask.v");
        let mut parser = sv_parser();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        let root = tree.root_node();
        fn dump(node: Node, depth: usize, src: &[u8]) {
            let indent = "  ".repeat(depth);
            let text = std::str::from_utf8(&src[node.byte_range()]).unwrap_or("");
            let text_preview = if text.len() > 40 { &text[..40] } else { text };
            eprintln!(
                "{}{} [{}..{}] = {:?}",
                indent,
                node.kind(),
                node.start_byte(),
                node.end_byte(),
                text_preview.replace('\n', "\\n")
            );
            for i in 0..node.child_count() {
                dump(node.child(i).unwrap(), depth + 1, src);
            }
        }
        eprintln!("\n=== tree-sitter root dump ===");
        dump(root, 0, src.as_bytes());
    }
}
