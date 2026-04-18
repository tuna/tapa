//! Port naming, arg table, and connection helpers.

use tapa_graphir::{Expression, HierarchicalName, ModuleConnection, ModuleNet, ModulePort, Range};

/// Stream port suffixes for istream (consumer side).
pub const ISTREAM_SUFFIXES: &[&str] = &["_dout", "_empty_n", "_read"];

/// Stream port suffixes for ostream (producer side).
pub const OSTREAM_SUFFIXES: &[&str] = &["_din", "_full_n", "_write"];

/// M-AXI port name prefix.
pub const M_AXI_PREFIX: &str = "m_axi_";

/// M-AXI read channel suffixes.
///
/// Matches Python's `tapa.protocol.M_AXI_SUFFIXES` on the read side —
/// notably does NOT include `_ARREGION`, which the Vitis top RTL
/// declares on scalar/mmap ports but Python's grouped-module lowering
/// never emits.
pub const M_AXI_READ_SUFFIXES: &[&str] = &[
    "_ARVALID", "_ARREADY", "_ARADDR", "_ARID", "_ARLEN", "_ARSIZE", "_ARBURST", "_ARLOCK",
    "_ARCACHE", "_ARPROT", "_ARQOS", "_RVALID", "_RREADY", "_RDATA", "_RLAST",
    "_RID", "_RRESP",
];

/// M-AXI write channel suffixes.
///
/// Matches Python's `tapa.protocol.M_AXI_SUFFIXES` on the write side —
/// notably does NOT include `_AWREGION`.
pub const M_AXI_WRITE_SUFFIXES: &[&str] = &[
    "_AWVALID", "_AWREADY", "_AWADDR", "_AWID", "_AWLEN", "_AWSIZE", "_AWBURST", "_AWLOCK",
    "_AWCACHE", "_AWPROT", "_AWQOS", "_WVALID", "_WREADY", "_WDATA", "_WSTRB",
    "_WLAST", "_BVALID", "_BREADY", "_BID", "_BRESP",
];

/// Build a `ModulePort` with the given type string.
///
/// The `hierarchical_name` defaults to `HierarchicalName::get_name(name)`,
/// matching Python's `HierarchicalName.get_name(port.name)` used
/// throughout the pipeline.
#[must_use]
pub fn make_port(name: &str, port_type: &str, range: Option<Range>) -> ModulePort {
    ModulePort {
        name: name.to_owned(),
        hierarchical_name: HierarchicalName::get_name(name),
        port_type: port_type.to_owned(),
        range,
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build an input wire port.
#[must_use]
pub fn input_wire(name: &str, range: Option<Range>) -> ModulePort {
    make_port(name, "input wire", range)
}

/// Build an output wire port.
#[must_use]
pub fn output_wire(name: &str, range: Option<Range>) -> ModulePort {
    make_port(name, "output wire", range)
}

/// Build a `ModuleNet` (internal wire). The `hierarchical_name` defaults
/// to `HierarchicalName::get_name(name)`, matching Python's
/// `HierarchicalName.get_name(net.name)` used throughout the pipeline.
#[must_use]
pub fn make_wire(name: &str, range: Option<Range>) -> ModuleNet {
    ModuleNet {
        name: name.to_owned(),
        hierarchical_name: HierarchicalName::get_name(name),
        range,
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build a `ModuleConnection`.
///
/// The `hierarchical_name` defaults to `HierarchicalName::get_name(name)`,
/// matching Python's `HierarchicalName.get_name(conn.name)` used when
/// emitting `ModuleConnection` objects in the graphir conversion
/// pipeline.
#[must_use]
pub fn make_connection(name: &str, expr: Expression) -> ModuleConnection {
    ModuleConnection {
        name: name.to_owned(),
        hierarchical_name: HierarchicalName::get_name(name),
        expr,
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build a range `[msb:0]`.
#[must_use]
pub fn range_msb(msb: u32) -> Range {
    Range {
        left: Expression::new_lit(&msb.to_string()),
        right: Expression::new_lit("0"),
    }
}

/// Build a range from whitespace-separated token expressions.
///
/// Each `left` / `right` string is tokenized on whitespace — tokens are
/// classified as identifier (alphabetic start or underscore) or literal
/// (otherwise). Mirrors Python's `Expression.from_str_to_tokens`, so
/// `"C_S_AXI_ADDR_WIDTH - 1"` becomes the three-token stream
/// `[id("C_S_AXI_ADDR_WIDTH"), lit("-"), lit("1")]` — matching Python's
/// `GraphIR` expression shape for `ctrl_s_axi` ADDR/DATA/STRB ranges.
#[must_use]
pub fn range_expr(left: &str, right: &str) -> Range {
    Range {
        left: expression_from_str(left),
        right: expression_from_str(right),
    }
}

/// Tokenize a whitespace-separated expression string into a `GraphIR`
/// [`Expression`], classifying each token as identifier or literal via
/// the leading-character rule.
#[must_use]
pub fn expression_from_str(s: &str) -> Expression {
    let tokens: Vec<tapa_graphir::Token> = s
        .split_whitespace()
        .map(|t| {
            if t.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
                tapa_graphir::Token::new_id(t)
            } else {
                tapa_graphir::Token::new_lit(t)
            }
        })
        .collect();
    if tokens.is_empty() {
        Expression::new_lit("0")
    } else {
        Expression(tokens)
    }
}

/// Get stream port name: `{base}{suffix}`.
#[must_use]
pub fn stream_port_name(base: &str, suffix: &str) -> String {
    format!("{base}{suffix}")
}

/// Get M-AXI port name: `m_axi_{base}{suffix}`.
#[must_use]
pub fn m_axi_port_name(base: &str, suffix: &str) -> String {
    format!("{M_AXI_PREFIX}{base}{suffix}")
}

/// Return `true` if the given M-AXI suffix is master-driven (output from
/// the master's perspective). Master top modules expose these as outputs.
#[must_use]
pub fn is_m_axi_master_output(suffix: &str) -> bool {
    // Master outputs on AW / W / AR channels; master inputs on R / B channels.
    if suffix.starts_with("_AW") || suffix.starts_with("_AR") || suffix.starts_with("_W") {
        // *READY on these channels is a master input.
        !suffix.ends_with("READY")
    } else if suffix.starts_with("_R") || suffix.starts_with("_B") {
        // *READY on R / B is master output; everything else is master input.
        suffix.ends_with("READY")
    } else {
        false
    }
}

/// Expand a topology-level port into its RTL-level signal ports.
///
/// Scalars produce a single port. Streams produce `_dout/_empty_n/_read`
/// or `_din/_full_n/_write`. MMAP produces `_offset` + all M-AXI channels.
pub fn expand_port_to_signals(
    name: &str,
    cat: tapa_task_graph::port::ArgCategory,
    width: u32,
) -> Vec<ModulePort> {
    use tapa_task_graph::port::ArgCategory;
    match cat {
        ArgCategory::Scalar => {
            vec![input_wire(name, if width > 1 { Some(range_msb(width - 1)) } else { None })]
        }
        ArgCategory::Istream | ArgCategory::Istreams => {
            vec![
                input_wire(&format!("{name}_dout"), Some(range_msb(width.saturating_sub(1)))),
                input_wire(&format!("{name}_empty_n"), None),
                output_wire(&format!("{name}_read"), None),
            ]
        }
        ArgCategory::Ostream | ArgCategory::Ostreams => {
            vec![
                output_wire(&format!("{name}_din"), Some(range_msb(width.saturating_sub(1)))),
                input_wire(&format!("{name}_full_n"), None),
                output_wire(&format!("{name}_write"), None),
            ]
        }
        ArgCategory::Mmap | ArgCategory::AsyncMmap | ArgCategory::Immap | ArgCategory::Ommap => {
            let mut ports = vec![
                input_wire(&format!("{name}_offset"), Some(range_msb(63))),
            ];
            // Add M-AXI channel ports with correct directions per AXI protocol.
            //
            // AXI master port-direction rules (top-level has master-facing ports):
            //   AW / W / AR channels:
            //     *VALID     → master output
            //     *READY     → master input
            //     data/addr  → master output
            //   R / B channels:
            //     *VALID     → master input (slave sends valid)
            //     *READY     → master output (master sends ready)
            //     data/resp  → master input
            for suffix in M_AXI_READ_SUFFIXES.iter().chain(M_AXI_WRITE_SUFFIXES.iter()) {
                let port_name = m_axi_port_name(name, suffix);
                if is_m_axi_master_output(suffix) {
                    ports.push(output_wire(&port_name, None));
                } else {
                    ports.push(input_wire(&port_name, None));
                }
            }
            ports
        }
    }
}

/// Convert a `tapa_rtl::Port` to a `tapa_graphir::ModulePort`.
///
/// Width expressions keep Python's tokenized shape: each RTL token
/// becomes a `GraphIR` [`Token`] classified as identifier (alphabetic
/// start or underscore) or literal (otherwise). Mirrors Python's
/// `Expression.from_str_to_tokens` used by `get_task_graphir_ports` in
/// `tapa/graphir_conversion/utils.py`.
#[must_use]
pub fn rtl_port_to_graphir(port: &tapa_rtl::port::Port) -> ModulePort {
    let port_type = match port.direction {
        tapa_rtl::port::Direction::Input => "input wire",
        tapa_rtl::port::Direction::Output => "output wire",
        tapa_rtl::port::Direction::Inout => "inout wire",
    };
    let range = port.width.as_ref().map(|w| Range {
        left: tokens_to_expression(&w.msb),
        right: tokens_to_expression(&w.lsb),
    });
    make_port(&port.name, port_type, range)
}

/// Convert a `tapa_rtl::MutableModule` to a `tapa_graphir::AnyModuleDefinition::Verilog`.
///
/// Translates ports **and** parameters from the parsed RTL module into
/// `GraphIR` structures — matching Python's
/// `get_verilog_definition_from_tapa_module(...)`.
#[must_use]
pub fn mutable_module_to_verilog_def(
    mm: &tapa_rtl::mutation::MutableModule,
) -> tapa_graphir::AnyModuleDefinition {
    let ports: Vec<ModulePort> = mm.inner.ports.iter().map(rtl_port_to_graphir).collect();
    let parameters: Vec<tapa_graphir::ModuleParameter> =
        mm.inner.parameters.iter().map(rtl_parameter_to_graphir).collect();
    tapa_graphir::AnyModuleDefinition::Verilog {
        base: tapa_graphir::BaseFields {
            name: mm.inner.name.clone(),
            hierarchical_name: tapa_graphir::HierarchicalName::none(),
            parameters,
            ports,
            metadata: None,
        },
        verilog: tapa_graphir::VerilogFields {
            verilog: mm.emit(),
            submodules_module_names: Vec::new(),
        },
        extra: std::collections::BTreeMap::default(),
    }
}

/// Convert a parsed RTL parameter into a `GraphIR` `ModuleParameter`.
#[must_use]
pub fn rtl_parameter_to_graphir(
    param: &tapa_rtl::param::Parameter,
) -> tapa_graphir::ModuleParameter {
    let expr = tokens_to_expression(&param.default);
    let range = param.width.as_ref().map(|w| tapa_graphir::Range {
        left: tokens_to_expression(&w.msb),
        right: tokens_to_expression(&w.lsb),
    });
    tapa_graphir::ModuleParameter {
        name: param.name.clone(),
        hierarchical_name: tapa_graphir::HierarchicalName::get_name(&param.name),
        expr,
        range,
        extra: std::collections::BTreeMap::default(),
    }
}

/// Convert a sequence of RTL tokens into a `GraphIR` `Expression`.
///
/// Performs the same Python classification as
/// `Expression.from_str_to_tokens` (identifier vs literal). If the
/// token stream is a pure arithmetic expression on integer literals —
/// with optional surrounding parentheses — the result is collapsed to a
/// single literal token, mirroring pyverilog's constant folding that
/// Python inherits when parsing declarations like
/// `parameter X = (32 / 8);` into a literal `4`.
fn tokens_to_expression(
    tokens: &[tapa_rtl::expression::Token],
) -> Expression {
    let graphir_tokens: Vec<tapa_graphir::Token> = tokens
        .iter()
        .map(|t| {
            if t.repr.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
                tapa_graphir::Token::new_id(&t.repr)
            } else {
                tapa_graphir::Token::new_lit(&t.repr)
            }
        })
        .collect();
    if graphir_tokens.is_empty() {
        return Expression::new_lit("0");
    }
    if let Some(value) = try_evaluate_literal_expr(&graphir_tokens) {
        return Expression(vec![tapa_graphir::Token::new_lit(&value.to_string())]);
    }
    Expression(graphir_tokens)
}

/// Attempt to evaluate a simple arithmetic expression on integer
/// literals. Returns `None` if any token is non-literal or the
/// expression shape is anything other than a flat `lit OP lit [OP lit ...]`
/// (optionally wrapped in one pair of parens). Covers `+`, `-`, `*`, `/`.
fn try_evaluate_literal_expr(tokens: &[tapa_graphir::Token]) -> Option<i64> {
    // Strip a single outer paren pair if present.
    let slice = match (tokens.first(), tokens.last()) {
        (Some(a), Some(b)) if a.repr == "(" && b.repr == ")" => &tokens[1..tokens.len() - 1],
        _ => tokens,
    };
    if slice.is_empty() {
        return None;
    }
    // Must be alternating literal / operator / literal / ...
    let mut acc: Option<i64> = None;
    let mut pending_op: Option<&str> = None;
    for tok in slice {
        if tok.kind != tapa_graphir::TokenKind::Literal {
            return None;
        }
        if let Ok(n) = tok.repr.parse::<i64>() {
            acc = Some(match (acc, pending_op) {
                (None, None) => n,
                (Some(a), Some("+")) => a.checked_add(n)?,
                (Some(a), Some("-")) => a.checked_sub(n)?,
                (Some(a), Some("*")) => a.checked_mul(n)?,
                (Some(a), Some("/")) => {
                    if n == 0 {
                        return None;
                    }
                    a.checked_div(n)?
                }
                _ => return None,
            });
            pending_op = None;
        } else if matches!(tok.repr.as_str(), "+" | "-" | "*" | "/") {
            if pending_op.is_some() || acc.is_none() {
                return None;
            }
            pending_op = Some(tok.repr.as_str());
        } else {
            return None;
        }
    }
    if pending_op.is_some() {
        return None;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_port_name_istream() {
        assert_eq!(stream_port_name("data", "_dout"), "data_dout");
    }

    #[test]
    fn m_axi_port_name_format() {
        assert_eq!(m_axi_port_name("a", "_ARVALID"), "m_axi_a_ARVALID");
    }

    // -- AXI master direction regression tests --
    //
    // Master outputs on AW/W/AR: *VALID, *ADDR, *DATA, *STRB, data/id/len/size/burst/lock/cache/prot/qos/region.
    // Master inputs  on AW/W/AR: *READY.
    // Master inputs  on R/B:     *VALID, *DATA, *STRB, *ID, *LAST, *RESP.
    // Master outputs on R/B:     *READY.

    #[test]
    fn is_master_output_aw_channel() {
        assert!(is_m_axi_master_output("_AWVALID"), "AWVALID is master output");
        assert!(!is_m_axi_master_output("_AWREADY"), "AWREADY is master input");
        assert!(is_m_axi_master_output("_AWADDR"), "AWADDR is master output");
        assert!(is_m_axi_master_output("_AWLEN"), "AWLEN is master output");
    }

    #[test]
    fn is_master_output_w_channel() {
        assert!(is_m_axi_master_output("_WVALID"), "WVALID is master output");
        assert!(!is_m_axi_master_output("_WREADY"), "WREADY is master input");
        assert!(is_m_axi_master_output("_WDATA"), "WDATA is master output");
        assert!(is_m_axi_master_output("_WSTRB"), "WSTRB is master output");
        assert!(is_m_axi_master_output("_WLAST"), "WLAST is master output");
    }

    #[test]
    fn is_master_output_ar_channel() {
        assert!(is_m_axi_master_output("_ARVALID"), "ARVALID is master output");
        assert!(!is_m_axi_master_output("_ARREADY"), "ARREADY is master input");
        assert!(is_m_axi_master_output("_ARADDR"), "ARADDR is master output");
    }

    #[test]
    fn is_master_output_r_channel() {
        // The critical fix: R channel VALID/data are master INPUTS.
        assert!(!is_m_axi_master_output("_RVALID"), "RVALID is master input");
        assert!(is_m_axi_master_output("_RREADY"), "RREADY is master output");
        assert!(!is_m_axi_master_output("_RDATA"), "RDATA is master input");
        assert!(!is_m_axi_master_output("_RLAST"), "RLAST is master input");
        assert!(!is_m_axi_master_output("_RRESP"), "RRESP is master input");
        assert!(!is_m_axi_master_output("_RID"), "RID is master input");
    }

    #[test]
    fn is_master_output_b_channel() {
        // The critical fix: B channel VALID is a master INPUT.
        assert!(!is_m_axi_master_output("_BVALID"), "BVALID is master input");
        assert!(is_m_axi_master_output("_BREADY"), "BREADY is master output");
        assert!(!is_m_axi_master_output("_BRESP"), "BRESP is master input");
        assert!(!is_m_axi_master_output("_BID"), "BID is master input");
    }

    #[test]
    fn expand_port_mmap_directions() {
        // Regression: verify top-level mmap expansion produces the correct
        // AXI master direction on every channel. *VALID on R/B must be
        // emitted as input (master is the slave of those channels).
        use tapa_task_graph::port::ArgCategory;
        let ports = expand_port_to_signals("b", ArgCategory::Mmap, 32);
        let by_name: std::collections::HashMap<String, bool> = ports
            .iter()
            .map(|p| (p.name.clone(), p.port_type.starts_with("output")))
            .collect();
        // R channel VALID must be input.
        assert_eq!(by_name.get("m_axi_b_RVALID"), Some(&false), "RVALID must be input");
        // B channel VALID must be input.
        assert_eq!(by_name.get("m_axi_b_BVALID"), Some(&false), "BVALID must be input");
        // R channel READY must be output.
        assert_eq!(by_name.get("m_axi_b_RREADY"), Some(&true), "RREADY must be output");
        // B channel READY must be output.
        assert_eq!(by_name.get("m_axi_b_BREADY"), Some(&true), "BREADY must be output");
        // AW channel VALID must be output.
        assert_eq!(by_name.get("m_axi_b_AWVALID"), Some(&true), "AWVALID must be output");
        // AW channel READY must be input.
        assert_eq!(by_name.get("m_axi_b_AWREADY"), Some(&false), "AWREADY must be input");
    }
}
