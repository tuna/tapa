//! Port naming, arg table, and connection helpers.

use std::collections::BTreeSet;

use tapa_graphir::{
    AnyModuleDefinition, BaseFields, Expression, GroupedFields, HierarchicalName, ModuleConnection,
    ModuleNet, ModulePort, Range,
};

use crate::instantiation_builder::ArgTable;

pub use tapa_protocol::{
    ISTREAM_SUFFIXES, M_AXI_PREFIX, M_AXI_READ_SUFFIXES, M_AXI_WRITE_SUFFIXES, OSTREAM_SUFFIXES,
};

/// Build a `ModulePort` with the given type string.
///
/// The `hierarchical_name` defaults to `HierarchicalName::get_name(name)`,
/// matching `HierarchicalName.get_name(port.name)` used
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

/// Build a `ModuleNet` (internal wire). The `hierarchical_name`
/// defaults to `HierarchicalName::get_name(name)`, the convention used
/// throughout the pipeline.
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
/// matching `HierarchicalName.get_name(conn.name)` used when
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
/// (otherwise). Mirrors `Expression.from_str_to_tokens`, so
/// `"C_S_AXI_ADDR_WIDTH - 1"` becomes the three-token stream
/// `[id("C_S_AXI_ADDR_WIDTH"), lit("-"), lit("1")]` — the
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
            if t.chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
            {
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
    let base = tapa_rtl::module::sanitize_array_name(base);
    format!("{base}{suffix}")
}

/// Get M-AXI port name: `m_axi_{base}{suffix}`.
#[must_use]
pub fn m_axi_port_name(base: &str, suffix: &str) -> String {
    let base = tapa_rtl::module::sanitize_array_name(base);
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
    let name = tapa_rtl::module::sanitize_array_name(name);
    match cat {
        ArgCategory::Scalar => {
            vec![input_wire(
                &name,
                if width > 1 {
                    Some(range_msb(width - 1))
                } else {
                    None
                },
            )]
        }
        ArgCategory::Istream | ArgCategory::Istreams => {
            vec![
                input_wire(
                    &format!("{name}_dout"),
                    Some(range_msb(width.saturating_sub(1))),
                ),
                input_wire(&format!("{name}_empty_n"), None),
                output_wire(&format!("{name}_read"), None),
            ]
        }
        ArgCategory::Ostream | ArgCategory::Ostreams => {
            vec![
                output_wire(
                    &format!("{name}_din"),
                    Some(range_msb(width.saturating_sub(1))),
                ),
                input_wire(&format!("{name}_full_n"), None),
                output_wire(&format!("{name}_write"), None),
            ]
        }
        ArgCategory::Mmap | ArgCategory::AsyncMmap | ArgCategory::Immap | ArgCategory::Ommap => {
            let mut ports = vec![input_wire(&format!("{name}_offset"), Some(range_msb(63)))];
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
            for suffix in M_AXI_READ_SUFFIXES
                .iter()
                .chain(M_AXI_WRITE_SUFFIXES.iter())
            {
                let port_name = m_axi_port_name(&name, suffix);
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
/// Width expressions keep tokenized shape: each RTL token
/// becomes a `GraphIR` [`Token`] classified as identifier (alphabetic
/// start or underscore) or literal (otherwise).
#[must_use]
pub fn rtl_port_to_graphir(port: &tapa_rtl::port::Port) -> ModulePort {
    let port_type = match port.direction {
        tapa_rtl::port::Direction::Input => "input wire",
        tapa_rtl::port::Direction::Output => "output wire",
        tapa_rtl::port::Direction::Inout => "inout wire",
    };
    rtl_port_to_graphir_with_type(port, port_type)
}

/// Convert a `tapa_rtl::Port` to a `tapa_graphir::ModulePort` with an
/// explicit Verilog port type string.
#[must_use]
pub fn rtl_port_to_graphir_with_type(port: &tapa_rtl::port::Port, port_type: &str) -> ModulePort {
    let range = port.width.as_ref().map(|w| Range {
        left: tokens_to_expression(&w.msb),
        right: tokens_to_expression(&w.lsb),
    });
    make_port(&port.name, port_type, range)
}

/// Convert a `tapa_rtl::MutableModule` to a `tapa_graphir::AnyModuleDefinition::Verilog`.
///
/// Translates ports **and** parameters from the parsed RTL module into
/// `GraphIR` structures.
#[must_use]
pub fn mutable_module_to_verilog_def(
    mm: &tapa_rtl::mutation::MutableModule,
) -> tapa_graphir::AnyModuleDefinition {
    let ports: Vec<ModulePort> = mm.inner.ports.iter().map(rtl_port_to_graphir).collect();
    let parameters: Vec<tapa_graphir::ModuleParameter> = mm
        .inner
        .parameters
        .iter()
        .map(rtl_parameter_to_graphir)
        .collect();
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
/// Performs the same classification as
/// `Expression.from_str_to_tokens` (identifier vs literal). If the
/// token stream is a pure arithmetic expression on integer literals —
/// with optional surrounding parentheses — the result is collapsed to a
/// single literal token, matching the frontend range folding that
/// inherits when parsing declarations like
/// `parameter X = (32 / 8);` into a literal `4`.
fn tokens_to_expression(tokens: &[tapa_rtl::expression::Token]) -> Expression {
    let graphir_tokens: Vec<tapa_graphir::Token> = tokens
        .iter()
        .map(|t| {
            if t.repr
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
            {
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
/// literals. Delegates to `evalexpr` for safe evaluation.
fn try_evaluate_literal_expr(tokens: &[tapa_graphir::Token]) -> Option<i64> {
    let expr = tokens
        .iter()
        .map(|t| t.repr.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    match evalexpr::eval(&expr) {
        Ok(evalexpr::Value::Int(n)) => Some(n),
        _ => None,
    }
}

pub(crate) fn resolve_instance_in_task(
    parent: &tapa_topology::task::TaskDesign,
    inst_name: &str,
) -> Option<(String, String)> {
    for (task_name, instances) in &parent.tasks {
        for (idx, inst) in instances.iter().enumerate() {
            if instance_matches_name(task_name, idx, inst, inst_name) {
                let canonical = crate::instantiation_builder::instance_name(task_name, idx, inst);
                return Some((task_name.clone(), canonical));
            }
        }
    }
    None
}

pub(crate) fn instance_matches_name(
    task_name: &str,
    idx: usize,
    inst: &tapa_topology::instance::InstanceDesign,
    inst_name: &str,
) -> bool {
    if inst.name.is_some() {
        crate::instantiation_builder::instance_name(task_name, idx, inst) == inst_name
    } else {
        format!("{task_name}_{idx}") == inst_name
    }
}

/// Find the parent-visible arg name for a child port in a given task.
pub(crate) fn find_arg_name_in_task(
    parent: &tapa_topology::task::TaskDesign,
    task_name: &str,
    inst_name: &str,
    port_name: &str,
) -> Option<String> {
    let instances = parent.tasks.get(task_name)?;
    for (idx, inst) in instances.iter().enumerate() {
        if instance_matches_name(task_name, idx, inst, inst_name) {
            for (child_port, arg) in &inst.args {
                if child_port == port_name {
                    return Some(arg.arg.clone());
                }
            }
        }
    }
    None
}

/// Find the grouped module named `name` and return mutable references to its
/// base + grouped fields. Centralizes the `AnyModuleDefinition::Grouped`
/// pattern used by several post-pass rewrites in `build_project_from_state`.
pub(crate) fn find_grouped_mut<'a>(
    defs: &'a mut [AnyModuleDefinition],
    name: &str,
) -> Option<(&'a mut BaseFields, &'a mut GroupedFields)> {
    for module in defs {
        if let AnyModuleDefinition::Grouped { base, grouped, .. } = module {
            if base.name == name {
                return Some((base, grouped));
            }
        }
    }
    None
}

pub(crate) fn attach_grouped_assigns(
    module: &mut AnyModuleDefinition,
    assigns: Vec<(String, String)>,
) {
    if assigns.is_empty() {
        return;
    }
    let value = serde_json::Value::Array(
        assigns
            .into_iter()
            .map(|(lhs, rhs)| serde_json::json!({ "lhs": lhs, "rhs": rhs }))
            .collect(),
    );
    if let AnyModuleDefinition::Grouped { extra, .. } = module {
        extra.insert("assigns".to_owned(), value);
    }
}

pub(crate) fn build_arg_pipeline_assigns(
    upper_task: &tapa_topology::task::TaskDesign,
    fsm_def: &AnyModuleDefinition,
    arg_table: &ArgTable,
) -> Vec<(String, String)> {
    let fsm_ports: BTreeSet<&str> = fsm_def.ports().iter().map(|p| p.name.as_str()).collect();
    let mut assigns = Vec::new();

    for (child_task_name, insts) in &upper_task.tasks {
        for (idx, inst) in insts.iter().enumerate() {
            let inst_name = crate::instantiation_builder::instance_name(child_task_name, idx, inst);
            let Some(inst_arg_table) = arg_table.get(&inst_name) else {
                continue;
            };

            for (port_name, arg) in &inst.args {
                let (fsm_in, fsm_out, source) = match arg.cat {
                    tapa_task_graph::port::ArgCategory::Scalar => (
                        format!("{inst_name}__{port_name}_in"),
                        format!("{inst_name}__{port_name}"),
                        scalar_or_literal_source(&arg.arg),
                    ),
                    tapa_task_graph::port::ArgCategory::Mmap
                    | tapa_task_graph::port::ArgCategory::AsyncMmap
                    | tapa_task_graph::port::ArgCategory::Immap
                    | tapa_task_graph::port::ArgCategory::Ommap => (
                        format!("{inst_name}__{port_name}_offset_in"),
                        format!("{inst_name}__{port_name}_offset"),
                        mmap_offset_source(upper_task, &arg.arg),
                    ),
                    tapa_task_graph::port::ArgCategory::Istream
                    | tapa_task_graph::port::ArgCategory::Ostream
                    | tapa_task_graph::port::ArgCategory::Istreams
                    | tapa_task_graph::port::ArgCategory::Ostreams => continue,
                };

                if !fsm_ports.contains(fsm_in.as_str()) || !fsm_ports.contains(fsm_out.as_str()) {
                    continue;
                }
                let Some(queue_tail) = inst_arg_table.get(&arg.arg) else {
                    continue;
                };
                assigns.push((fsm_in, source));
                assigns.push((queue_tail.clone(), fsm_out));
            }
        }
    }

    assigns
}

pub(crate) fn scalar_or_literal_source(name: &str) -> String {
    if crate::instantiation_builder::is_literal_arg(name) {
        name.to_owned()
    } else {
        tapa_rtl::module::sanitize_array_name(name)
    }
}

pub(crate) fn mmap_offset_source(
    upper_task: &tapa_topology::task::TaskDesign,
    arg_name: &str,
) -> String {
    let sanitized = tapa_rtl::module::sanitize_array_name(arg_name);
    let chan_count = upper_task
        .ports
        .iter()
        .find(|p| p.name == arg_name)
        .and_then(|p| p.chan_count);
    if matches!(chan_count, Some(count) if count > 1) {
        format!("{sanitized}_0_offset")
    } else {
        format!("{sanitized}_offset")
    }
}

pub(crate) fn fifo_wire_range(suffix: &str, data_range: Option<&Range>) -> Option<Range> {
    if matches!(suffix, "_din" | "_dout") {
        data_range.cloned()
    } else {
        None
    }
}

/// Parse `taskname_idx` into `(task_name, idx)`.
pub(crate) fn parse_instance_name(name: &str) -> (String, u32) {
    if let Some(last_underscore) = name.rfind('_') {
        if let Ok(idx) = name[last_underscore + 1..].parse::<u32>() {
            return (name[..last_underscore].to_owned(), idx);
        }
    }
    (name.to_owned(), 0)
}

/// Build a port range `[width-1:0]` if width > 1.
pub(crate) fn port_range(width: u32) -> Option<tapa_graphir::Range> {
    if width > 1 {
        Some(range_msb(width - 1))
    } else {
        None
    }
}

/// Declare the six FIFO data/handshake wires for `fifo_name` unless an
/// identically named port or wire already exists (the exporter's DRC
/// requires every referenced identifier to be declared). Data suffixes
/// get the FIFO data range; control suffixes are 1-bit.
pub(crate) fn declare_fifo_wires(
    wires: &mut Vec<ModuleNet>,
    ports: &[ModulePort],
    fifo_name: &str,
    data_range: Option<&Range>,
) {
    for suffix in ["_din", "_dout", "_empty_n", "_full_n", "_read", "_write"] {
        let wire_name = format!("{fifo_name}{suffix}");
        if !ports.iter().any(|p| p.name == wire_name) && !wires.iter().any(|w| w.name == wire_name)
        {
            wires.push(make_wire(&wire_name, fifo_wire_range(suffix, data_range)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_port_name_istream() {
        assert_eq!(stream_port_name("data", "_dout"), "data_dout");
    }

    #[test]
    fn stream_port_name_sanitizes_indexed_names() {
        assert_eq!(stream_port_name("a[0]_Cannon", "_din"), "a_0_Cannon_din");
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
        assert!(
            is_m_axi_master_output("_AWVALID"),
            "AWVALID is master output"
        );
        assert!(
            !is_m_axi_master_output("_AWREADY"),
            "AWREADY is master input"
        );
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
        assert!(
            is_m_axi_master_output("_ARVALID"),
            "ARVALID is master output"
        );
        assert!(
            !is_m_axi_master_output("_ARREADY"),
            "ARREADY is master input"
        );
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
        assert_eq!(
            by_name.get("m_axi_b_RVALID"),
            Some(&false),
            "RVALID must be input"
        );
        // B channel VALID must be input.
        assert_eq!(
            by_name.get("m_axi_b_BVALID"),
            Some(&false),
            "BVALID must be input"
        );
        // R channel READY must be output.
        assert_eq!(
            by_name.get("m_axi_b_RREADY"),
            Some(&true),
            "RREADY must be output"
        );
        // B channel READY must be output.
        assert_eq!(
            by_name.get("m_axi_b_BREADY"),
            Some(&true),
            "BREADY must be output"
        );
        // AW channel VALID must be output.
        assert_eq!(
            by_name.get("m_axi_b_AWVALID"),
            Some(&true),
            "AWVALID must be output"
        );
        // AW channel READY must be input.
        assert_eq!(
            by_name.get("m_axi_b_AWREADY"),
            Some(&false),
            "AWREADY must be input"
        );
    }
}
