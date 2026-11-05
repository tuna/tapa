//! Top-level `VerilogModule` type aggregating parsed interface elements.

use crate::error::ParseError;
use crate::param::Parameter;
use crate::parser;
use crate::port::Port;
use crate::signal::Signal;

/// A parsed Verilog module interface.
///
/// Contains all interface elements extracted from a TAPA-generated
/// Verilog module: ports, parameters, signals, and the raw source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerilogModule {
    /// Module name.
    pub name: String,
    /// Ports with direction and width.
    pub ports: Vec<Port>,
    /// Module parameters with defaults.
    pub parameters: Vec<Parameter>,
    /// Signal declarations (wire/reg).
    pub signals: Vec<Signal>,
    /// Raw Verilog source text, preserved verbatim.
    pub source: String,
}

/// Vitis-generated RTL infixes tried when looking up a FIFO port by its
/// logical argument name.
const FIFO_INFIXES: &[&str] = &["_V", "_r", "_s", ""];

/// The port-name convention belongs to the schema, so that `tapa-codegen`,
/// `tapa pack` and `frt-cosim` cannot drift apart on it. Re-exported here
/// because RTL identifiers are where it gets applied.
pub use tapa_ir::port::{sanitize_array_name, sanitize_identifier_name};

impl VerilogModule {
    /// Parse a Verilog module header from source text.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Err(ParseError::EmptyInput);
        }
        parser::parse_module(source)
    }

    /// Names driven by plain module-scope zero assignments in the original
    /// source. Conditional, procedural, delayed, or ambiguous drivers and
    /// malformed bodies are not proof. This performs one transient CST parse;
    /// the module's serialized interface remains unchanged.
    pub fn unconditionally_zero_driven_names(&self) -> std::collections::BTreeSet<String> {
        parser::unconditionally_zero_driven_names(&self.source)
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
        let find_with_base = |base: &str| {
            FIFO_INFIXES
                .iter()
                .find_map(|infix| self.find_port(&format!("{base}{infix}{suffix}")))
        };
        if let Some(port) = find_with_base(&sanitize_array_name(fifo)) {
            return Some(port);
        }
        match match_array_name(fifo) {
            Some((base, 0)) => find_with_base(base),
            _ => None,
        }
    }
}

/// Match `name[idx]` and return `(name, idx)`.
#[must_use]
fn match_array_name(name: &str) -> Option<(&str, u32)> {
    let lb = name.find('[')?;
    let rb = name.rfind(']')?;
    if rb <= lb + 1 || !name.ends_with(']') {
        return None;
    }
    let base = &name[..lb];
    let idx_str = &name[lb + 1..rb];
    if base.is_empty() {
        return None;
    }
    if !base.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let idx: u32 = idx_str.parse().ok()?;
    Some((base, idx))
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
        }
    }

    fn module(ports: Vec<Port>) -> VerilogModule {
        VerilogModule {
            name: "m".into(),
            ports,
            parameters: Vec::new(),
            signals: Vec::new(),
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
    fn sanitize_array_name_is_reexported_from_the_schema() {
        // Behavior is pinned in `tapa_ir::port`; this only guards the
        // re-export that keeps this crate's callers working.
        assert_eq!(sanitize_array_name("foo[3]"), "foo_3");
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

    #[test]
    fn zero_driver_query_requires_literal_full_width_module_assignments() {
        let source = "module Drivers(output [1:0] plain, sliced, ambiguous, delayed, connected);\n\
                      assign plain = (2'b0_0);\n\
                      assign sliced[0] = 1'b0;\n\
                      assign ambiguous = '0;\n\
                      assign ambiguous[0] = live;\n\
                      assign #1 delayed = 2'b0;\n\
                      assign connected = 2'b0;\n\
                      child inst(.activity(connected));\n\
                      endmodule";
        let module = VerilogModule::parse(source).unwrap();
        assert_eq!(
            module.unconditionally_zero_driven_names(),
            std::iter::once("plain".to_owned()).collect(),
            "partial writes and competing or delayed drivers are not proof"
        );
    }
}
