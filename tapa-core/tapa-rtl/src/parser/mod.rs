//! Verilog module parser for TAPA-generated module headers.
//!
//! All structure comes from a single `tree-sitter-systemverilog` parse
//! (headers, ports, parameters, signal declarations, attributes). `nom`
//! survives only as the attribute leaf parser: `(* key = "value" *)`
//! attribute text is matched with nom combinators because it is a
//! loosely structured pragma format, not Verilog grammar.

use nom::bytes::complete::{tag, take_until, take_while1};
use nom::character::complete::{char, multispace0};
use nom::combinator::opt;
use nom::sequence::{delimited, pair, preceded};
use nom::IResult;
use nom::Parser;

use crate::error::ParseError;
use crate::module::VerilogModule;
use crate::pragma::Pragma;

mod tree_sitter;

// ── Utility parsers ─────────────────────────────────────────────────

fn ws(input: &str) -> IResult<&str, &str> {
    multispace0(input)
}

fn identifier(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$')(input)
}

// ── Pragma / attribute parsers ──────────────────────────────────────

/// Parse a single attribute: `(* key = "value" *)` or `(* key *)`.
fn attribute(input: &str) -> IResult<&str, Pragma> {
    let start = input;
    let (input, _) = tag("(*").parse(input)?;
    let (input, _) = ws(input)?;
    let (input, key) = identifier(input)?;
    let (input, _) = ws(input)?;
    let (input, value) = opt(preceded(
        pair(char('='), multispace0),
        delimited(char('"'), take_until("\""), char('"')),
    ))
    .parse(input)?;
    let (input, _) = ws(input)?;
    let (input, _) = tag("*)").parse(input)?;

    let consumed = &start[..start.len() - input.len()];
    Ok((
        input,
        Pragma {
            key: key.to_owned(),
            value: value.map(str::to_owned),
            raw_line: consumed.to_owned(),
        },
    ))
}

/// Fallback: capture raw `(* ... *)` when structured parsing fails.
fn raw_attribute(input: &str) -> IResult<&str, Pragma> {
    let start = input;
    let (input, _) = tag("(*").parse(input)?;
    let (input, _content) = take_until("*)").parse(input)?;
    let (input, _) = tag("*)").parse(input)?;
    let consumed = &start[..start.len() - input.len()];
    Ok((
        input,
        Pragma {
            key: String::new(),
            value: None,
            raw_line: consumed.to_owned(),
        },
    ))
}

// ── Error-context helpers ───────────────────────────────────────────

/// Byte offset of the `module` keyword when it is the first non-trivia
/// token, mirroring the module scan the old pre-parser used for error
/// classification. Returns `None` when any other token comes first
/// (the old scan rejected such input with `NoModuleFound`).
fn module_keyword_start(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace.
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Skip single-line comments.
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Skip block comments.
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Skip attributes: `(* ... *)`
        if i + 1 < bytes.len() && bytes[i] == b'(' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b')') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Skip compiler directives (backtick lines like `timescale).
        if bytes[i] == b'`' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Check for `module` keyword.
        if source[i..].starts_with("module") {
            let after = i + 6;
            if after >= bytes.len() || !bytes[after].is_ascii_alphanumeric() {
                return Some(i);
            }
        }
        return None;
    }
    None
}

/// Module name for error context, extracted without a full parse.
fn partial_module_name(source: &str) -> String {
    module_keyword_start(source)
        .and_then(|start| source[start..].strip_prefix("module"))
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("<unknown>")
        .trim_end_matches('(')
        .to_owned()
}

// ── Top-level parser ────────────────────────────────────────────────

/// Parse a TAPA-generated Verilog module, extracting all interface elements.
pub fn parse_module(source: &str) -> Result<VerilogModule, ParseError> {
    let header_failed = || ParseError::ParseFailed {
        module: partial_module_name(source),
        message: "module header: tree-sitter failed to extract header".to_string(),
    };

    let info = match tree_sitter::parse_module_info(source)? {
        tree_sitter::ModuleParse::Ok(info) => *info,
        tree_sitter::ModuleParse::Issue(issue) => {
            return Err(ParseError::ParseFailed {
                module: partial_module_name(source),
                message: match issue {
                    tree_sitter::ParseIssue::MalformedPort => {
                        "malformed port declaration".to_string()
                    }
                    tree_sitter::ParseIssue::MalformedParameter => {
                        "malformed parameter".to_string()
                    }
                },
            });
        }
        tree_sitter::ModuleParse::NoModule => {
            return Err(if module_keyword_start(source).is_some() {
                header_failed()
            } else {
                ParseError::NoModuleFound
            });
        }
        tree_sitter::ModuleParse::HeaderFailed => return Err(header_failed()),
    };

    // Verify all header-listed ports have declarations.
    let declared_names: std::collections::HashSet<&str> =
        info.ports.iter().map(|p| p.name.as_str()).collect();
    for pname in &info.port_names {
        if !declared_names.contains(pname.as_str()) {
            return Err(ParseError::ParseFailed {
                module: info.name.clone(),
                message: format!("port `{pname}` listed in header but has no declaration"),
            });
        }
    }

    Ok(VerilogModule {
        name: info.name,
        ports: info.ports,
        parameters: info.params,
        signals: info.signals,
        pragmas: info.pragmas,
        source: source.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::Direction;
    use crate::signal::SignalKind;

    fn port<'m>(m: &'m VerilogModule, name: &str) -> &'m crate::port::Port {
        m.find_port(name).unwrap_or_else(|| panic!("port {name}"))
    }

    fn signal_names(m: &VerilogModule) -> Vec<&str> {
        m.signals.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn parse_simple_module() {
        let src = "\
// Leading comment
module Simple (
  ap_clk,
  data_out
);

input ap_clk;   // trailing comment
output [31:0] data_out;

wire [31:0] internal;
reg done;

endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "Simple");
        assert_eq!(m.ports.len(), 2, "ports");
        assert!(m.parameters.is_empty(), "no params");
        assert!(port(&m, "ap_clk").width.is_none());
        assert!(port(&m, "data_out").width.is_some());
        assert_eq!(port(&m, "data_out").direction, Direction::Output);
        assert_eq!(
            m.signals.iter().map(|s| s.kind).collect::<Vec<_>>(),
            [SignalKind::Wire, SignalKind::Reg]
        );
    }

    #[test]
    fn parse_signal_declaration_forms() {
        // Multiple declarators share the declared width; `signed` and
        // parameter-driven widths are accepted; unpacked dimensions and
        // initializers do not create extra signals.
        let src = "\
module SigForms;
parameter W = 32;
wire [W-1:0] a, b, c;
reg signed [2*(W+1)-1:0] s;
reg [3:0] mem [0:255];
wire [7:0] init = {1'b1, 2'b10};
endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(signal_names(&m), ["a", "b", "c", "s", "mem", "init"]);
        assert!(m.signals.iter().all(|s| s.width.is_some()), "all widths");
    }

    #[test]
    fn only_wire_and_reg_are_signals() {
        let src = "\
module NotSignals;
logic l;
integer i;
tri t;
reg actual;
endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(signal_names(&m), ["actual"]);
    }

    #[test]
    fn signal_scoping_excludes_nested_and_procedural_decls() {
        // Only the target module's own declarations are signals: nested
        // modules, generate regions, and procedural blocks are not
        // descended into.
        let src = "\
module Outer;
wire own;
module Inner;
wire inner;
endmodule
reg top;
always @(posedge clk) begin : blk
  reg local_r;
  top <= 1'b0;
end
generate
if (1) begin : g
  wire gen_w;
end
endgenerate
endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(signal_names(&m), ["own", "top"]);
    }

    #[test]
    fn err_on_malformed_signal() {
        let err = parse_module("module M;\nwire [31:0] ;\nendmodule\n").unwrap_err();
        assert!(err.to_string().contains("malformed signal"), "err: {err}");
        let err = parse_module("module M;\nreg [3:0] a, ;\nendmodule\n").unwrap_err();
        assert!(err.to_string().contains('M'), "err: {err}");
    }

    #[test]
    fn parse_parameters_body_and_header_block() {
        let src = "\
module WithParams #(
  parameter WIDTH = 32,
  parameter DEPTH = (1 + (2))
) (
  ap_clk
);
parameter ap_ST_fsm_state1 = 1'd1;
parameter [31:0] DATA_WIDTH = 32;
input ap_clk;
endmodule
";
        let m = parse_module(src).expect("parse");
        let names: Vec<&str> = m.parameters.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["WIDTH", "DEPTH", "ap_ST_fsm_state1", "DATA_WIDTH"]);
        assert!(m.parameters[3].width.is_some(), "typed param has width");
        assert!(m.parameters[2].width.is_none(), "untyped param has none");
    }

    #[test]
    fn parse_pragmas_leading_and_body_in_order() {
        let src = r#"
(* CORE_GENERATION_INFO="Test,hls_ip" *)
module AttrMod (
  ap_clk
);

(* RS_CLK *)
input ap_clk;

(* DONT_TOUCH = "yes" *)
(* KEEP *)
wire net;

endmodule
"#;
        let m = parse_module(src).expect("parse");
        let keys: Vec<&str> = m.pragmas.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(
            keys,
            ["CORE_GENERATION_INFO", "RS_CLK", "DONT_TOUCH", "KEEP"]
        );
        // The pragma above the declaration is recorded verbatim as a
        // module pragma; the parsed signal itself carries no attribute.
        assert_eq!(signal_names(&m), ["net"]);
        assert!(m.signals[0].attribute.is_none(), "no attribute on signal");
    }

    #[test]
    fn empty_and_non_verilog_rejected() {
        parse_module("").unwrap_err();
        parse_module("   ").unwrap_err();
        parse_module("hello world this is not verilog").unwrap_err();
        parse_module("logic stray;\nmodule M;\nendmodule\n").unwrap_err();
    }

    #[test]
    fn parse_ansi_ports() {
        let src = "\
module AnsiMod (
  (* RS_CLK *) input wire [31:0] a,
  output [7:0] b,
  c
);
endmodule
";
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "AnsiMod");
        assert_eq!(m.ports.len(), 3, "ports");
        assert_eq!(port(&m, "a").direction, Direction::Input);
        assert!(port(&m, "a").pragma.is_some(), "header pragma on port");
        assert_eq!(port(&m, "b").direction, Direction::Output);
        // Width is inherited across comma-separated ANSI ports.
        let c = port(&m, "c");
        assert_eq!(c.direction, Direction::Output);
        assert!(c.width.is_some(), "width inherited by c");
        // The header attribute attaches to the port only, not the module.
        assert!(m.pragmas.is_empty(), "no module pragmas");
    }
}
