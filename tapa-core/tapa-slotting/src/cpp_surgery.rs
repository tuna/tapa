//! Tree-sitter C++ function body replacement.
//!
//! Provides `replace_function` for precise C++ function body replacement
//! using tree-sitter CST queries and byte-offset splicing.

use regex::Regex;
use std::sync::LazyLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use crate::error::SlottingError;

fn cpp_language() -> Language {
    tree_sitter_cpp::LANGUAGE.into()
}

// ── Tree-sitter queries ──────────────────────────────────────────────

static QUERY_FUNC: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &cpp_language(),
        "
        (function_definition
          declarator: (function_declarator
            declarator: (identifier) @name)
          body: (compound_statement) @body)
        ",
    )
    .expect("QUERY_FUNC should parse")
});

static QUERY_EXTERN_FUNC: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &cpp_language(),
        "
        (linkage_specification
          body: (function_definition
            declarator: (function_declarator
              declarator: (identifier) @name)
            body: (compound_statement) @body))
        ",
    )
    .expect("QUERY_EXTERN_FUNC should parse")
});

static QUERY_EXTERN_LINKAGE_BRACED_DEF: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &cpp_language(),
        "
        (linkage_specification
          (declaration_list
            (function_definition
              declarator: (function_declarator
                declarator: (identifier) @name)))) @linkage
        ",
    )
    .expect("QUERY_EXTERN_LINKAGE_BRACED_DEF should parse")
});

static QUERY_EXTERN_LINKAGE_BRACED_DECL: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &cpp_language(),
        "
        (linkage_specification
          (declaration_list
            (declaration
              declarator: (function_declarator
                declarator: (identifier) @name)))) @linkage
        ",
    )
    .expect("QUERY_EXTERN_LINKAGE_BRACED_DECL should parse")
});

static QUERY_EXTERN_LINKAGE_INLINE: LazyLock<Query> = LazyLock::new(|| {
    Query::new(
        &cpp_language(),
        "
        (linkage_specification
          body: (function_definition
            declarator: (function_declarator
              declarator: (identifier) @name))) @linkage
        ",
    )
    .expect("QUERY_EXTERN_LINKAGE_INLINE should parse")
});

static TRAILING_EXTERN_C_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*//\s*extern\s+"C""#).unwrap());

// ── Internal helpers ─────────────────────────────────────────────────

/// Find all `linkage_specification` nodes wrapping `func_name`.
fn find_extern_c_linkage_nodes<'a>(
    root: Node<'a>,
    source: &[u8],
    func_name: &str,
) -> Vec<Node<'a>> {
    let func_bytes = func_name.as_bytes();
    let mut nodes = Vec::new();

    for query in [
        &*QUERY_EXTERN_LINKAGE_BRACED_DEF,
        &*QUERY_EXTERN_LINKAGE_BRACED_DECL,
        &*QUERY_EXTERN_LINKAGE_INLINE,
    ] {
        let name_idx = query.capture_index_for_name("name").unwrap();
        let linkage_idx = query.capture_index_for_name("linkage").unwrap();

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            let name_node = m.captures.iter().find(|c| c.index == name_idx);
            let linkage_node = m.captures.iter().find(|c| c.index == linkage_idx);

            if let (Some(name_cap), Some(linkage_cap)) = (name_node, linkage_node) {
                let name_text = &source[name_cap.node.byte_range()];
                if name_text == func_bytes {
                    nodes.push(linkage_cap.node);
                }
            }
        }
    }
    nodes
}

/// Find the `compound_statement` body node for `func_name`.
fn find_function_body<'a>(root: Node<'a>, source: &[u8], func_name: &str) -> Option<Node<'a>> {
    let func_bytes = func_name.as_bytes();

    for query in [&*QUERY_FUNC, &*QUERY_EXTERN_FUNC] {
        let name_idx = query.capture_index_for_name("name").unwrap();
        let body_idx = query.capture_index_for_name("body").unwrap();

        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            let name_node = m.captures.iter().find(|c| c.index == name_idx);
            let body_node = m.captures.iter().find(|c| c.index == body_idx);

            if let (Some(name_cap), Some(body_cap)) = (name_node, body_node) {
                let name_text = &source[name_cap.node.byte_range()];
                if name_text == func_bytes {
                    return Some(body_cap.node);
                }
            }
        }
    }
    None
}

// ── Public API ───────────────────────────────────────────────────────

/// Replace a C++ function body using tree-sitter CST-based splicing.
///
/// **3-argument mode** (`new_def` is `None`):
/// Replaces only the `compound_statement` body of the named function.
///
/// **4-argument mode** (`new_def` is `Some`):
/// Removes all `extern "C"` linkage blocks for the function, then
/// appends new declaration and definition blocks.
pub fn replace_function(
    source: &str,
    func_name: &str,
    new_body_or_decl: &str,
    new_def: Option<&str>,
) -> Result<String, SlottingError> {
    if source.trim().is_empty() {
        return Err(SlottingError::EmptySource);
    }

    let mut parser = Parser::new();
    parser
        .set_language(&cpp_language())
        .map_err(|e| SlottingError::TreeSitter(e.to_string()))?;

    let source_bytes = source.as_bytes();
    let tree = parser
        .parse(source_bytes, None)
        .ok_or_else(|| SlottingError::TreeSitter("failed to parse C++ source".into()))?;

    if let Some(new_def) = new_def {
        // 4-argument mode: remove old extern "C" blocks, append new ones.
        let new_decl = new_body_or_decl;
        let mut linkage_nodes = find_extern_c_linkage_nodes(tree.root_node(), source_bytes, func_name);
        if linkage_nodes.is_empty() {
            return Err(SlottingError::FunctionNotFound(func_name.to_owned()));
        }
        linkage_nodes.sort_by_key(|n| std::cmp::Reverse(n.start_byte()));

        let mut bytes = source_bytes.to_vec();
        for node in linkage_nodes {
            let mut end = node.end_byte();
            let rest = &bytes[end..];
            if let Some(m) = TRAILING_EXTERN_C_COMMENT.find(std::str::from_utf8(rest).unwrap_or(""))
            {
                end += m.end();
            }
            bytes = [&bytes[..node.start_byte()], &bytes[end..]].concat();
        }

        let mut code = String::from_utf8(bytes)
            .map_err(|e| SlottingError::TreeSitter(e.to_string()))?;
        let trimmed_len = code.trim_end().len();
        code.truncate(trimmed_len);

        let decl_block = format!(
            "extern \"C\" {{\n{}\n}}  // extern \"C\"",
            new_decl.trim()
        );
        let def_block = format!(
            "extern \"C\" {{\n{}\n}}  // extern \"C\"",
            new_def.trim()
        );
        code.push_str("\n\n");
        code.push_str(&decl_block);
        code.push_str("\n\n");
        code.push_str(&def_block);
        code.push('\n');
        Ok(code)
    } else {
        // 3-argument mode: replace function body only.
        let body_node = find_function_body(tree.root_node(), source_bytes, func_name)
            .ok_or_else(|| SlottingError::FunctionNotFound(func_name.to_owned()))?;

        let before = &source_bytes[..body_node.start_byte()];
        let after = &source_bytes[body_node.end_byte()..];
        let mut result = String::from_utf8(before.to_vec())
            .map_err(|e| SlottingError::TreeSitter(e.to_string()))?;
        result.push_str("{\n    ");
        result.push_str(new_body_or_decl);
        result.push_str("\n}");
        let after_str = std::str::from_utf8(after)
            .map_err(|e| SlottingError::TreeSitter(e.to_string()))?;
        result.push_str(after_str);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_extern_c_function_body() {
        let source = r#"
extern "C" void my_func(int a, float b) {
    // original body
    int x = 1;
}
"#;
        let new_body = "#pragma HLS interface ap_none port = a register\n  { auto val = a; }";
        let result = replace_function(source, "my_func", new_body, None).unwrap();
        assert!(result.contains("pragma HLS"), "got: {result}");
        assert!(!result.contains("original body"), "got: {result}");
    }

    #[test]
    fn function_not_found_raises() {
        let source = r#"
extern "C" void other_func(int a) {
    int x = 1;
}
"#;
        let result = replace_function(source, "nonexistent_func", "{ }", None);
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[test]
    fn preserves_surrounding_code() {
        let source = r#"
#include <stdint.h>

extern "C" void slot_func(uint64_t x) {
    // old body
}

// some trailing code
"#;
        let result = replace_function(source, "slot_func", "// new body", None).unwrap();
        assert!(result.contains("#include <stdint.h>"), "got: {result}");
        assert!(result.contains("// some trailing code"), "got: {result}");
        assert!(result.contains("// new body"), "got: {result}");
        assert!(!result.contains("// old body"), "got: {result}");
    }

    #[test]
    fn four_arg_removes_old_blocks() {
        let source = r#"
extern "C" {
void my_func(int a);
}  // extern "C"

extern "C" {
void my_func(int a) {
    // original body
}
}  // extern "C"
"#;
        let result = replace_function(
            source,
            "my_func",
            "void my_func(int a);",
            Some("void my_func(int a) { /* new body */ }"),
        )
        .unwrap();
        assert!(result.contains("/* new body */"), "got: {result}");
        assert!(!result.contains("original body"), "got: {result}");
    }

    #[test]
    fn four_arg_with_includes() {
        let source = r#"
#include <stdint.h>

extern "C" {
void top(uint64_t x);
}  // extern "C"

extern "C" {
void top(uint64_t x) {
    // old impl
}
}  // extern "C"
"#;
        let result = replace_function(
            source,
            "top",
            "void top(uint64_t x);",
            Some("void top(uint64_t x) { /* new impl */ }"),
        )
        .unwrap();
        assert!(result.contains("#include <stdint.h>"), "got: {result}");
        assert!(result.contains("/* new impl */"), "got: {result}");
        assert!(!result.contains("old impl"), "got: {result}");
        // 2 new blocks, each has extern "C" open + comment = 4 total
        assert_eq!(result.matches(r#"extern "C""#).count(), 4, "got: {result}");
    }

    #[test]
    fn non_extern_function_body_replace() {
        let source = "
void simple_func(int x) {
    return x + 1;
}
";
        let result = replace_function(source, "simple_func", "return x * 2;", None).unwrap();
        assert!(result.contains("return x * 2;"), "got: {result}");
        assert!(!result.contains("return x + 1"), "got: {result}");
    }

    #[test]
    fn empty_source_rejected() {
        let err = replace_function("", "func", "body", None).unwrap_err();
        assert!(err.to_string().contains("empty"), "should reject empty source, got: {err}");
    }

    #[test]
    fn whitespace_only_source_rejected() {
        replace_function("   \n  ", "func", "body", None).unwrap_err();
    }

    #[test]
    fn four_arg_missing_function_rejected() {
        let source = "int helper() { return 1; }";
        let err = replace_function(source, "top", "void top();", Some("void top() {}")).unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "should reject missing function in 4-arg mode, got: {err}"
        );
    }
}
