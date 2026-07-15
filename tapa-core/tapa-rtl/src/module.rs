//! Top-level `VerilogModule` type aggregating parsed interface elements.

use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::error::ParseError;
use crate::param::Parameter;
use crate::parser;
use crate::port::Port;
use crate::pragma::Pragma;
use crate::signal::Signal;

/// A parsed Verilog module interface.
///
/// Contains all interface elements extracted from a TAPA-generated
/// Verilog module: ports, parameters, signals, pragmas, and the
/// raw source text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerilogModule {
    /// Module name.
    pub name: String,
    /// Ports with direction and width.
    pub ports: Vec<Port>,
    /// Module parameters with defaults.
    pub parameters: Vec<Parameter>,
    /// Signal declarations (wire/reg).
    pub signals: Vec<Signal>,
    /// Pragmas extracted from attributes.
    pub pragmas: Vec<Pragma>,
    /// Raw Verilog source text, preserved verbatim.
    pub source: String,
}

/// Vitis-generated RTL infixes tried when looking up a FIFO port by its
/// logical argument name.
pub const FIFO_INFIXES: &[&str] = &["_V", "_r", "_s", ""];

static ARRAY_NAME_WITH_SUFFIX_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^([a-zA-Z_]\w*)\[(\d+)\]([a-zA-Z_]\w*)?$").unwrap());

impl VerilogModule {
    /// Parse a Verilog module header from source text.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Err(ParseError::EmptyInput);
        }
        parser::parse_module(source)
    }

    /// Find a port by exact name.
    pub fn find_port(&self, name: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.name == name)
    }

    /// Find a port by prefix and suffix.
    pub fn find_port_by_affixes(&self, prefix: &str, suffix: &str) -> Option<&Port> {
        self.ports
            .iter()
            .find(|p| p.name.starts_with(prefix) && p.name.ends_with(suffix))
    }

    /// Resolve a FIFO / stream port by its logical base name and a suffix
    /// like `_din`, `_dout`, `_full_n`, `_empty_n`, `_read`, `_write`.
    ///
    /// 1. Sanitize array-style names (`a[3]` → `a_3`).
    /// 2. Try each `FIFO_INFIXES` entry in order (`_V`, `_r`, `_s`, `""`)
    ///    to find a port named `{base}{infix}{suffix}`.
    /// 3. If the original name was `foo[0]`, also try `{foo}{infix}{suffix}`
    ///    as a singleton-array fallback.
    pub fn get_port_of(&self, fifo: &str, suffix: &str) -> Option<&Port> {
        let sanitized = sanitize_array_name(fifo);
        for infix in FIFO_INFIXES {
            let name = format!("{sanitized}{infix}{suffix}");
            if let Some(port) = self.find_port(&name) {
                return Some(port);
            }
        }
        if let Some((base, idx)) = match_array_name(fifo) {
            if idx == 0 {
                for infix in FIFO_INFIXES {
                    let name = format!("{base}{infix}{suffix}");
                    if let Some(port) = self.find_port(&name) {
                        return Some(port);
                    }
                }
            }
        }
        None
    }
}

/// Match `name[idx]` and return `(name, idx)`.
#[must_use]
pub fn match_array_name(name: &str) -> Option<(&str, u32)> {
    let lb = name.find('[')?;
    let rb = name.rfind(']')?;
    if rb <= lb + 1 || !name.ends_with(']') {
        return None;
    }
    let base = &name[..lb];
    let idx_str = &name[lb + 1..rb];
    if base.is_empty() || idx_str.is_empty() {
        return None;
    }
    if !base.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let idx: u32 = idx_str.parse().ok()?;
    Some((base, idx))
}

/// Collapse `name[idx]` into `name_{idx}`.
#[must_use]
pub fn sanitize_array_name(name: &str) -> String {
    ARRAY_NAME_WITH_SUFFIX_RE.captures(name).map_or_else(
        || name.to_owned(),
        |caps| {
            format!(
                "{}_{}{}",
                &caps[1],
                &caps[2],
                caps.get(3).map_or("", |m| m.as_str())
            )
        },
    )
}

/// Convert frontend names into plain Verilog identifiers.
///
/// Keeps compatible array-name handling (`foo[3]` -> `foo_3`) before
/// replacing characters that cannot appear in unescaped Verilog identifiers.
#[must_use]
pub fn sanitize_identifier_name(name: &str) -> String {
    let name = sanitize_array_name(name);
    let mut out = String::with_capacity(name.len());
    for (idx, ch) in name.chars().enumerate() {
        let valid = ch.is_ascii_alphanumeric() || ch == '_' || ch == '$';
        if idx == 0 && ch.is_ascii_digit() {
            out.push('_');
        }
        out.push(if valid { ch } else { '_' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, dir: &str) -> Port {
        use crate::port::Direction;
        Port {
            name: name.to_owned(),
            direction: match dir {
                "output" => Direction::Output,
                "inout" => Direction::Inout,
                _ => Direction::Input,
            },
            width: None,
            pragma: None,
        }
    }

    fn module(ports: Vec<Port>) -> VerilogModule {
        VerilogModule {
            name: "m".into(),
            ports,
            parameters: Vec::new(),
            signals: Vec::new(),
            pragmas: Vec::new(),
            source: String::new(),
        }
    }

    #[test]
    fn match_array_name_basic() {
        assert_eq!(match_array_name("foo[3]"), Some(("foo", 3)));
        assert_eq!(match_array_name("foo[0]"), Some(("foo", 0)));
        assert_eq!(match_array_name("foo"), None);
        assert_eq!(match_array_name("foo[bar]"), None);
    }

    #[test]
    fn sanitize_array_name_collapses_brackets() {
        assert_eq!(sanitize_array_name("foo[3]"), "foo_3");
        assert_eq!(sanitize_array_name("foo[3]_bar"), "foo_3_bar");
        assert_eq!(sanitize_array_name("foo"), "foo");
    }

    #[test]
    fn sanitize_identifier_name_replaces_invalid_chars() {
        assert_eq!(sanitize_identifier_name("Module1Func#1"), "Module1Func_1");
        assert_eq!(sanitize_identifier_name("foo[3]"), "foo_3");
        assert_eq!(sanitize_identifier_name("1foo"), "_1foo");
    }

    #[test]
    fn get_port_of_suffixed_array_name() {
        let m = module(vec![port("qs_24_Network_s_dout", "output")]);
        assert_eq!(
            m.get_port_of("qs[24]_Network", "_dout")
                .map(|p| p.name.as_str()),
            Some("qs_24_Network_s_dout"),
        );
    }

    #[test]
    fn get_port_of_s_infix_istream() {
        let m = module(vec![port("a_q_VecAdd_s_dout", "output")]);
        assert_eq!(
            m.get_port_of("a_q_VecAdd", "_dout")
                .map(|p| p.name.as_str()),
            Some("a_q_VecAdd_s_dout"),
        );
    }

    #[test]
    fn get_port_of_s_infix_ostream() {
        let m = module(vec![port("a_q_VecAdd_s_din", "output")]);
        assert_eq!(
            m.get_port_of("a_q_VecAdd", "_din").map(|p| p.name.as_str()),
            Some("a_q_VecAdd_s_din"),
        );
    }

    #[test]
    fn get_port_of_v_infix_preferred() {
        // All infixes present → `_V` wins (it's first in FIFO_INFIXES).
        let m = module(vec![
            port("x_V_dout", "input"),
            port("x_r_dout", "input"),
            port("x_s_dout", "input"),
            port("x_dout", "input"),
        ]);
        assert_eq!(
            m.get_port_of("x", "_dout").map(|p| p.name.as_str()),
            Some("x_V_dout"),
        );
    }

    #[test]
    fn get_port_of_empty_infix_fallback() {
        let m = module(vec![port("x_dout", "input")]);
        assert_eq!(
            m.get_port_of("x", "_dout").map(|p| p.name.as_str()),
            Some("x_dout"),
        );
    }

    #[test]
    fn get_port_of_singleton_array_fallback() {
        let m = module(vec![port("x_s_dout", "output")]);
        // `x[0]` sanitized → `x_0`; direct lookup fails, singleton
        // fallback `{base}{infix}{suffix}` = `x_s_dout` succeeds.
        assert_eq!(
            m.get_port_of("x[0]", "_dout").map(|p| p.name.as_str()),
            Some("x_s_dout"),
        );
    }

    #[test]
    fn get_port_of_singleton_array_nonzero_idx_rejected() {
        let m = module(vec![port("x_s_dout", "output")]);
        // Non-zero index has no singleton fallback.
        assert_eq!(m.get_port_of("x[1]", "_dout"), None);
    }

    #[test]
    fn get_port_of_no_match() {
        let m = module(vec![port("other", "input")]);
        assert_eq!(m.get_port_of("missing", "_dout"), None);
    }
}
