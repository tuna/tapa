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
}
