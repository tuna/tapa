//! Verilog module parser for TAPA-generated module headers.
//!
//! All structure comes from a single `tree-sitter-systemverilog` parse
//! (headers, ports, parameters, signal declarations).

use crate::error::ParseError;
use crate::module::VerilogModule;

mod inspection;
mod tree_sitter;

pub use inspection::{original_module_text, unconditionally_zero_driven_names};

// ── Top-level parser ────────────────────────────────────────────────

/// Parse a TAPA-generated Verilog module, extracting all interface elements.
pub fn parse_module(source: &str) -> Result<VerilogModule, ParseError> {
    let or_unknown = |name: Option<String>| name.unwrap_or_else(|| "<unknown>".to_owned());
    let header_failed = |name| ParseError::ParseFailed {
        module: or_unknown(name),
        message: "module header: tree-sitter failed to extract header".to_string(),
    };

    let info = match tree_sitter::parse_module_info(source)? {
        tree_sitter::ModuleParse::Ok(info) => *info,
        tree_sitter::ModuleParse::Issue { issue, name } => {
            return Err(ParseError::ParseFailed {
                module: or_unknown(name),
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
        tree_sitter::ModuleParse::NoModule => return Err(ParseError::NoModuleFound),
        tree_sitter::ModuleParse::HeaderFailed { name } => return Err(header_failed(name)),
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
        assert_eq!(port(&m, "b").direction, Direction::Output);
        // Width is inherited across comma-separated ANSI ports.
        let c = port(&m, "c");
        assert_eq!(c.direction, Direction::Output);
        assert!(c.width.is_some(), "width inherited by c");
    }
}
