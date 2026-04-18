//! Top-level `VerilogModule` type aggregating parsed interface elements.

use serde::{Deserialize, Serialize};

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
/// logical argument name. Mirrors Python's `_FIFO_INFIXES` in
/// `tapa/verilog/xilinx/module_ops/ports.py`.
pub const FIFO_INFIXES: &[&str] = &["_V", "_r", "_s", ""];

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
    /// Mirrors Python's `tapa.verilog.xilinx.module_ops.ports.get_port_of`:
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

    /// Extract submodule instantiations from the raw source.
    ///
    /// Returns a list of `(module_name, instance_name)` pairs for every
    /// `module_name #(...) instance_name (...);` occurrence in the
    /// module body. Used by cross-language grouped-Verilog parity tests
    /// so Python and Rust exports can be compared on the shape of their
    /// instance lists without a full syntactic roundtrip.
    ///
    /// Implementation: a minimal token scan that recognizes an
    /// instantiation as `ident [#(parens)] ident (parens);`. Comments,
    /// `module`/`endmodule`, parameter/port/signal declarations, and
    /// procedural blocks are skipped — only full instantiation
    /// statements at the module body level are returned.
    #[must_use]
    pub fn instance_names(&self) -> Vec<(String, String)> {
        parser::extract_instance_names(&self.source)
    }
}

/// Match `name[idx]` and return `(name, idx)`. Mirrors Python's
/// `match_array_name` in `tapa/verilog/util.py`.
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

/// Collapse `name[idx]` into `name_{idx}`. Mirrors Python's
/// `sanitize_array_name` in `tapa/verilog/util.py`.
#[must_use]
pub fn sanitize_array_name(name: &str) -> String {
    match_array_name(name).map_or_else(|| name.to_owned(), |(base, idx)| format!("{base}_{idx}"))
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
        assert_eq!(sanitize_array_name("foo"), "foo");
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
    fn instance_names_extracts_single_fifo() {
        let src = "
module top (input clk, input rst);
wire [32:0] d;
fifo #(.DATA_WIDTH(33), .DEPTH(2)) fifo_0 (
  .clk(clk), .reset(rst), .if_din(d), .if_dout()
);
endmodule
";
        let m = VerilogModule::parse(src).unwrap();
        let insts = m.instance_names();
        assert_eq!(insts, vec![("fifo".to_owned(), "fifo_0".to_owned())]);
    }

    #[test]
    fn instance_names_extracts_multiple() {
        let src = "
module top (input clk);
wire a, b;
fifo_a fifo_0 (.clk(clk), .din(a), .dout(b));
fifo_b #(.X(1)) fifo_1 (
  .clk(clk),
  .din(b)
);
// comment: fake_module fake_inst();
endmodule
";
        let m = VerilogModule::parse(src).unwrap();
        let insts = m.instance_names();
        assert_eq!(
            insts,
            vec![
                ("fifo_a".to_owned(), "fifo_0".to_owned()),
                ("fifo_b".to_owned(), "fifo_1".to_owned()),
            ]
        );
    }

    #[test]
    fn instance_names_skips_declarations() {
        let src = "
module top (input clk);
parameter P = 32;
wire [P-1:0] data;
assign data = 0;
endmodule
";
        let m = VerilogModule::parse(src).unwrap();
        let insts = m.instance_names();
        assert!(insts.is_empty(), "no instantiations, got {insts:?}");
    }
}
