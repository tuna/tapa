pub fn escape_verilog_identifier(name: &str) -> String {
    if is_bare_verilog_identifier(name) {
        return name.to_owned();
    }
    format!("\\{name} ")
}

pub fn verilator_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push_str(&format!("__{:03x}", ch as u32));
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        out.insert(0, '_');
    }
    out
}

pub fn cpp_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if ch == '[' {
            out.push('_');
        } else if ch == ']' {
            continue;
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        out.insert(0, '_');
    }
    out
}

pub fn escaped_verilog_signal(prefix: &str, name: &str, suffix: &str) -> String {
    escape_verilog_identifier(&format!("{prefix}{name}{suffix}"))
}

pub fn verilator_signal(prefix: &str, name: &str, suffix: &str) -> String {
    verilator_identifier(&format!("{prefix}{name}{suffix}"))
}

pub fn cpp_signal(prefix: &str, name: &str, suffix: &str) -> String {
    cpp_identifier(&format!("{prefix}{name}{suffix}"))
}

pub fn infer_peek_name(stream_name: &str) -> Option<String> {
    if let Some(base) = stream_name.strip_suffix("_s") {
        return Some(format!("{base}_peek"));
    }
    let mut iter = stream_name.rsplitn(2, '_');
    if let (Some(suffix), Some(base)) = (iter.next(), iter.next()) {
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("{base}_peek_{suffix}"));
        }
    }
    Some(format!("{stream_name}_peek"))
}

pub fn stream_peek_ports_exist(
    verilog_files: &[std::path::PathBuf],
    top_name: &str,
    peek_name: &str,
) -> bool {
    let dout_port = format!("{peek_name}_dout");
    let empty_n_port = format!("{peek_name}_empty_n");
    let module_decl = format!("module {top_name}");
    verilog_files.iter().any(|file| {
        std::fs::read_to_string(file)
            .map(|text| {
                if !text.contains(&module_decl) {
                    return false;
                }
                let has_dout = text.lines().any(|line| {
                    let t = line.trim();
                    (t.starts_with("input") || t.starts_with("output")) && t.contains(&dout_port)
                });
                let has_empty_n = text.lines().any(|line| {
                    let t = line.trim();
                    (t.starts_with("input") || t.starts_with("output"))
                        && t.contains(&empty_n_port)
                });
                has_dout && has_empty_n
            })
            .unwrap_or(false)
    })
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
        assert_eq!(infer_peek_name("i_next").as_deref(), Some("i_next_peek"));
        assert_eq!(infer_peek_name("a").as_deref(), Some("a_peek"));
    }

    #[test]
    fn infer_peek_name_preserves_special_suffix_forms() {
        assert_eq!(infer_peek_name("stream_s").as_deref(), Some("stream_peek"));
        assert_eq!(
            infer_peek_name("stream_3").as_deref(),
            Some("stream_peek_3")
        );
    }
}
