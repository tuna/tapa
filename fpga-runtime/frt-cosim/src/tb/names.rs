pub fn escape_verilog_identifier(name: &str) -> String {
    if is_bare_verilog_identifier(name) {
        return name.to_owned();
    }
    format!("\\{name} ")
}

pub fn verilator_identifier(name: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            let _ = write!(out, "__{:03x}", ch as u32);
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

pub fn cpp_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if ch == ']' {
            // skip closing bracket
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

pub fn infer_peek_name(stream_name: &str) -> String {
    if let Some(base) = stream_name.strip_suffix("_s") {
        return format!("{base}_peek");
    }
    let mut iter = stream_name.rsplitn(2, '_');
    if let (Some(suffix), Some(base)) = (iter.next(), iter.next()) {
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return format!("{base}_peek_{suffix}");
        }
    }
    format!("{stream_name}_peek")
}

pub fn stream_peek_ports_exist(
    verilog_contents: &[String],
    top_name: &str,
    peek_name: &str,
) -> bool {
    let top_file = top_module_contents(verilog_contents, top_name);
    crate::tb::port_declaration(top_file, &format!("{peek_name}_dout")).is_some()
        && crate::tb::port_declaration(top_file, &format!("{peek_name}_empty_n")).is_some()
}

/// HLS packages one module per file; probes must inspect the DUT's file.
pub(super) fn top_module_contents<'a>(
    verilog_contents: &'a [String],
    top_name: &str,
) -> &'a [String] {
    static MODULE_DECL: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(
            r"(?ms)^\s*(?:\(\*.*?\*\)\s*)*module\s+(\\[^\s]+|[^\s(#;]+)(?:\s|[#(;])",
        )
        .unwrap()
    });
    verilog_contents
        .iter()
        .find(|text| {
            let code = crate::tb::verilog_code(text);
            MODULE_DECL
                .captures_iter(&code)
                .any(|decl| decl[1].strip_prefix('\\').unwrap_or(&decl[1]) == top_name)
        })
        .map_or(&[], std::slice::from_ref)
}

fn is_bare_verilog_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_verilog_identifiers_using_backslash_syntax() {
        assert_eq!(escape_verilog_identifier("plain_name"), "plain_name");
        assert_eq!(escape_verilog_identifier("chan[0]"), "\\chan[0] ");
        assert_eq!(escape_verilog_identifier("a.b"), "\\a.b ");
        assert_eq!(escape_verilog_identifier("foo-bar"), "\\foo-bar ");
    }

    #[test]
    fn mangles_identifiers_like_verilator() {
        assert_eq!(verilator_identifier("plain_name"), "plain_name");
        assert_eq!(verilator_identifier("chan[0]"), "chan__05b0__05d");
        assert_eq!(verilator_identifier("a.b"), "a__02eb");
        assert_eq!(verilator_identifier("foo-bar"), "foo__02dbar");
    }

    #[test]
    fn creates_cpp_safe_identifiers_like_the_verilator_headers() {
        assert_eq!(cpp_identifier("plain_name"), "plain_name");
        assert_eq!(cpp_identifier("chan[0]"), "chan_0");
        assert_eq!(cpp_identifier("a.b"), "a_b");
        assert_eq!(cpp_identifier("foo-bar"), "foo_bar");
    }

    #[test]
    fn infer_peek_name_handles_plain_stream_names() {
        assert_eq!(infer_peek_name("i_next"), "i_next_peek");
        assert_eq!(infer_peek_name("a"), "a_peek");
    }

    #[test]
    fn infer_peek_name_preserves_special_suffix_forms() {
        assert_eq!(infer_peek_name("stream_s"), "stream_peek");
        assert_eq!(infer_peek_name("stream_3"), "stream_peek_3");
    }

    #[test]
    fn stream_probes_inspect_only_the_named_dut() {
        for module_name in ["Top", "\\Top"] {
            let rtl = vec![
                "module \\Top#Child (input [31:0] stream_dout, stream_peek_dout, input stream_peek_empty_n); endmodule".to_owned(),
                "module TopChild (input [31:0] stream_dout, stream_peek_dout, input stream_peek_empty_n); endmodule".to_owned(),
                format!("module {module_name} (input [7:0] stream_dout); endmodule"),
            ];
            assert_eq!(
                crate::tb::port_declaration(top_module_contents(&rtl, "Top"), "stream_dout"),
                Some(8),
                "DUT {module_name} must supply its own stream width"
            );
            assert!(
                !stream_peek_ports_exist(&rtl, "Top", "stream_peek"),
                "DUT {module_name} must not inherit another module's peek ports"
            );
        }
    }
}
