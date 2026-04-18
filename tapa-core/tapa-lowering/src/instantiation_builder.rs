//! Builds `ModuleInstantiation` for tasks, slots, FSMs, and FIFOs.

use tapa_graphir::{Expression, HierarchicalName, ModuleConnection, ModuleInstantiation};
use tapa_task_graph::port::ArgCategory;
use tapa_topology::instance::ArgDesign;

use crate::utils::{
    make_connection, m_axi_port_name, stream_port_name, ISTREAM_SUFFIXES, M_AXI_READ_SUFFIXES,
    M_AXI_WRITE_SUFFIXES, OSTREAM_SUFFIXES,
};

/// Arg-table entry mapping a parent-visible argument name to its
/// queue-tail wire name.
///
/// Mirrors Python's `get_task_arg_table(task)[inst_name][arg][-1].name`:
///   * scalar → `{inst_name}___{arg}__q0`
///   * mmap → `{inst_name}___{arg}_offset__q0`
///
/// Keyed on `arg.arg` (the parent-visible arg name) for both categories
/// so callers can look up queue-tail wires without needing to know the
/// child port name.
pub type ArgTable = std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;

/// Build an arg table for an upper task's instances.
///
/// For each instance's scalar/mmap arguments, produces the queue-tail
/// wire name Python's `get_task_arg_table(upper_task)` emits
/// (`Pipeline(instance.get_instance_arg(id_name))[-1].name`). The key
/// is the parent-visible arg name (`arg.arg`), matching Python's
/// `arg_table[inst_name][arg.name]` indexing in `wire_builder.py`.
#[must_use]
pub fn build_arg_table(
    top: &tapa_topology::task::TaskDesign,
) -> ArgTable {
    let mut table = ArgTable::new();
    for (task_name, instances) in &top.tasks {
        for (idx, inst) in instances.iter().enumerate() {
            let inst_name = format!("{task_name}_{idx}");
            let mut inst_table = std::collections::BTreeMap::new();
            for arg in inst.args.values() {
                // Scalar: {inst}___{arg}__q0; MMAP: {inst}___{arg}_offset__q0.
                // Streams have no arg-table entry.
                let wire = match arg.cat {
                    ArgCategory::Scalar => format!("{inst_name}___{}__q0", arg.arg),
                    ArgCategory::Mmap
                    | ArgCategory::AsyncMmap
                    | ArgCategory::Immap
                    | ArgCategory::Ommap => format!("{inst_name}___{}_offset__q0", arg.arg),
                    ArgCategory::Istream
                    | ArgCategory::Ostream
                    | ArgCategory::Istreams
                    | ArgCategory::Ostreams => continue,
                };
                inst_table.insert(arg.arg.clone(), wire);
            }
            table.insert(inst_name, inst_table);
        }
    }
    table
}

/// Collect all pipeline wire names from the arg table for wire generation.
#[must_use]
pub fn collect_arg_table_wires(arg_table: &ArgTable) -> Vec<String> {
    let mut wires = Vec::new();
    for inst_table in arg_table.values() {
        for signal in inst_table.values() {
            if !wires.contains(signal) {
                wires.push(signal.clone());
            }
        }
    }
    wires.sort();
    wires
}

/// Build connections for a child instance port based on its category.
///
/// When `arg_table_entry` is provided, scalar and mmap offset connections
/// use the queue-tail signal names from the arg table instead of raw arg
/// names. When `child_rtl` is provided, stream suffixes are resolved
/// against the child RTL via `get_port_of` (handles `_V` / `_r` / `_s` /
/// bare infix), and istream input suffixes also emit an extra `_peek`
/// variant when the RTL declares one — matching Python's
/// `_connect_istream` path in `instantiation_builder.py`.
///
/// When `child_rtl_ports` is provided, MMAP AXI channels are filtered to
/// only include suffixes that actually exist on the child's RTL module,
/// matching Python's `get_child_port_connection_mapping` behavior.
#[allow(clippy::implicit_hasher, reason = "Option<&HashSet> is simpler than generic S")]
pub fn build_port_connections(
    port_name: &str,
    arg: &ArgDesign,
    arg_table_entry: Option<&std::collections::BTreeMap<String, String>>,
    child_rtl_ports: Option<&std::collections::HashSet<String>>,
    child_rtl: Option<&tapa_rtl::VerilogModule>,
) -> Vec<ModuleConnection> {
    // Resolve a child RTL stream port name via Python-equivalent
    // get_port_of (with the `_FIFO_INFIXES` + singleton array fallback).
    // Fallback: the raw `{port}{suffix}` concat when no RTL is available.
    let resolve = |name: &str, suffix: &str| -> String {
        if let Some(module) = child_rtl {
            if let Some(p) = module.get_port_of(name, suffix) {
                return p.name.clone();
            }
        }
        stream_port_name(name, suffix)
    };
    // Try to resolve an istream peek port via `{port}_peek{suffix}`.
    let resolve_peek = |name: &str, suffix: &str| -> Option<String> {
        let module = child_rtl?;
        let peek_name = format!("{name}_peek");
        module.get_port_of(&peek_name, suffix).map(|p| p.name.clone())
    };
    match arg.cat {
        ArgCategory::Scalar => {
            let signal = arg_table_entry
                .and_then(|t| t.get(&arg.arg))
                .map_or_else(|| arg.arg.clone(), Clone::clone);
            vec![make_connection(port_name, Expression::new_id(&signal))]
        }
        ArgCategory::Istream | ArgCategory::Istreams => {
            // Python `_connect_istream` emits a base connection per
            // ISTREAM suffix and an extra `_peek{suffix}` connection
            // for every input-direction suffix when the RTL has it.
            let mut conns = Vec::new();
            for suffix in ISTREAM_SUFFIXES {
                let child_port = resolve(port_name, suffix);
                let wire = stream_port_name(&arg.arg, suffix);
                conns.push(make_connection(&child_port, Expression::new_id(&wire)));
                // `_dout` / `_empty_n` are input suffixes (child reads
                // from FIFO); emit peek variant if declared.
                if matches!(*suffix, "_dout" | "_empty_n") {
                    if let Some(peek_port) = resolve_peek(port_name, suffix) {
                        conns.push(make_connection(
                            &peek_port,
                            Expression::new_id(&wire),
                        ));
                    }
                }
            }
            conns
        }
        ArgCategory::Ostream | ArgCategory::Ostreams => {
            OSTREAM_SUFFIXES
                .iter()
                .map(|suffix| {
                    make_connection(
                        &resolve(port_name, suffix),
                        Expression::new_id(&stream_port_name(&arg.arg, suffix)),
                    )
                })
                .collect()
        }
        ArgCategory::Mmap | ArgCategory::AsyncMmap | ArgCategory::Immap | ArgCategory::Ommap => {
            let offset_port = format!("{port_name}_offset");
            let offset_signal = arg_table_entry
                .and_then(|t| t.get(&arg.arg))
                .map_or_else(|| format!("{}_offset", arg.arg), Clone::clone);
            let mut conns = vec![make_connection(
                &offset_port,
                Expression::new_id(&offset_signal),
            )];
            for suffix in M_AXI_READ_SUFFIXES
                .iter()
                .chain(M_AXI_WRITE_SUFFIXES.iter())
            {
                let child_port = m_axi_port_name(port_name, suffix);
                if let Some(known_ports) = child_rtl_ports {
                    if !known_ports.contains(&child_port) {
                        continue;
                    }
                }
                conns.push(make_connection(
                    &child_port,
                    Expression::new_id(&m_axi_port_name(&arg.arg, suffix)),
                ));
            }
            conns
        }
    }
}

/// Build a task instance with standard control connections.
#[must_use]
pub fn build_task_instance(
    inst_name: &str,
    module_name: &str,
    arg_connections: Vec<ModuleConnection>,
    region: Option<&str>,
) -> ModuleInstantiation {
    let mut connections = vec![
        make_connection("ap_clk", Expression::new_id("ap_clk")),
        make_connection("ap_rst_n", Expression::new_id("ap_rst_n")),
        make_connection("ap_start", Expression::new_id(&format!("{inst_name}__ap_start"))),
        make_connection("ap_done", Expression::new_id(&format!("{inst_name}__ap_done"))),
        make_connection("ap_idle", Expression::new_id(&format!("{inst_name}__ap_idle"))),
        make_connection("ap_ready", Expression::new_id(&format!("{inst_name}__ap_ready"))),
    ];
    connections.extend(arg_connections);

    ModuleInstantiation {
        hierarchical_name: HierarchicalName::get_name(inst_name),
        name: inst_name.to_owned(),
        module: module_name.to_owned(),
        connections,
        parameters: Vec::new(),
        floorplan_region: region.map(str::to_owned),
        area: None,
        pragmas: Vec::new(),
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build a FIFO instance with `DATA_WIDTH`, `ADDR_WIDTH`, `DEPTH` parameters.
///
/// Mirrors Python's `tapa.graphir_conversion.pipeline.fifo_builder::get_fifo_inst`.
/// `data_range` is the producer RTL's `_din` / `_dout` port range; the
/// `DATA_WIDTH` expression is `(left) - (right) + 1`, which Python
/// collapses to a single literal via `eval_verilog_const_no_exception`
/// when both endpoints are integer literals. `is_top` controls the
/// reset wiring — `rst` for top FIFOs, `~ap_rst_n` for slot-local
/// FIFOs — matching `_get_fifo_connections(is_top=...)`.
#[must_use]
pub fn build_fifo_instance(
    fifo_name: &str,
    data_range: Option<&tapa_graphir::Range>,
    depth: u32,
    region: Option<&str>,
    is_top: bool,
) -> ModuleInstantiation {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "addr_width is always a small positive integer"
    )]
    let addr_width = f64::from(depth).log2().ceil().max(1.0) as u32;

    let reset_expr = if is_top {
        Expression::new_id("rst")
    } else {
        Expression(vec![
            tapa_graphir::Token::new_lit("~"),
            tapa_graphir::Token::new_id("ap_rst_n"),
        ])
    };

    let connections = vec![
        make_connection("clk", Expression::new_id("ap_clk")),
        make_connection("reset", reset_expr),
        make_connection("if_dout", Expression::new_id(&format!("{fifo_name}_dout"))),
        make_connection("if_empty_n", Expression::new_id(&format!("{fifo_name}_empty_n"))),
        make_connection("if_read", Expression::new_id(&format!("{fifo_name}_read"))),
        make_connection("if_din", Expression::new_id(&format!("{fifo_name}_din"))),
        make_connection("if_full_n", Expression::new_id(&format!("{fifo_name}_full_n"))),
        make_connection("if_write", Expression::new_id(&format!("{fifo_name}_write"))),
        make_connection("if_read_ce", Expression::new_lit("1'b1")),
        make_connection("if_write_ce", Expression::new_lit("1'b1")),
    ];

    let data_width_expr = compute_data_width_expr(data_range);

    let parameters = vec![
        make_connection("DEPTH", Expression::new_lit(&depth.to_string())),
        make_connection("ADDR_WIDTH", Expression::new_lit(&addr_width.to_string())),
        make_connection("DATA_WIDTH", data_width_expr),
    ];

    ModuleInstantiation {
        hierarchical_name: HierarchicalName::get_name(fifo_name),
        name: fifo_name.to_owned(),
        module: "fifo".to_owned(),
        connections,
        parameters,
        floorplan_region: region.map(str::to_owned),
        area: None,
        pragmas: Vec::new(),
        extra: std::collections::BTreeMap::default(),
    }
}

/// Build the `DATA_WIDTH` expression for a FIFO from its data range.
///
/// Python emits `(left) - (right) + 1` tokens and folds to a literal when
/// both endpoints are integer literals via pyverilog evaluation. If the
/// range is missing we conservatively emit `32` (matches the legacy
/// topology-derived fallback).
fn compute_data_width_expr(data_range: Option<&tapa_graphir::Range>) -> Expression {
    let Some(range) = data_range else {
        return Expression::new_lit("32");
    };
    // Fast path: literal endpoints → literal result.
    if let (Some(l), Some(r)) = (
        expression_as_int(&range.left),
        expression_as_int(&range.right),
    ) {
        return Expression::new_lit(&(l - r + 1).to_string());
    }
    // Fallback: emit the full 9-token stream Python uses. If pyverilog
    // couldn't fold it either, Rust should emit it verbatim.
    let mut toks: Vec<tapa_graphir::Token> = Vec::new();
    toks.push(tapa_graphir::Token::new_lit("("));
    toks.extend(range.left.0.iter().cloned());
    toks.push(tapa_graphir::Token::new_lit(")"));
    toks.push(tapa_graphir::Token::new_lit("-"));
    toks.push(tapa_graphir::Token::new_lit("("));
    toks.extend(range.right.0.iter().cloned());
    toks.push(tapa_graphir::Token::new_lit(")"));
    toks.push(tapa_graphir::Token::new_lit("+"));
    toks.push(tapa_graphir::Token::new_lit("1"));
    Expression(toks)
}

fn expression_as_int(expr: &Expression) -> Option<i64> {
    if expr.0.len() != 1 {
        return None;
    }
    expr.0[0].repr.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_connection() {
        let arg = ArgDesign {
            arg: "n".into(),
            cat: ArgCategory::Scalar,
            extra: std::collections::BTreeMap::default(),
        };
        let conns = build_port_connections("count", &arg, None, None, None);
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].name, "count");
    }

    #[test]
    fn istream_connection() {
        let arg = ArgDesign {
            arg: "fifo_0".into(),
            cat: ArgCategory::Istream,
            extra: std::collections::BTreeMap::default(),
        };
        let conns = build_port_connections("data_in", &arg, None, None, None);
        assert_eq!(conns.len(), 3);
        assert!(conns.iter().any(|c| c.name == "data_in_dout"), "conns: {conns:?}");
        assert!(conns.iter().any(|c| c.name == "data_in_empty_n"), "conns: {conns:?}");
        assert!(conns.iter().any(|c| c.name == "data_in_read"), "conns: {conns:?}");
    }

    #[test]
    fn ostream_connection() {
        let arg = ArgDesign {
            arg: "fifo_0".into(),
            cat: ArgCategory::Ostream,
            extra: std::collections::BTreeMap::default(),
        };
        let conns = build_port_connections("data_out", &arg, None, None, None);
        assert_eq!(conns.len(), 3);
        assert!(conns.iter().any(|c| c.name == "data_out_din"), "conns: {conns:?}");
    }

    #[test]
    fn mmap_connection_has_offset_and_axi() {
        let arg = ArgDesign {
            arg: "mem_a".into(),
            cat: ArgCategory::Mmap,
            extra: std::collections::BTreeMap::default(),
        };
        let conns = build_port_connections("ptr", &arg, None, None, None);
        // offset + 18 read + 21 write = 40 connections
        assert!(conns.len() > 30, "should have many mmap connections, got {}", conns.len());
        assert!(conns.iter().any(|c| c.name == "ptr_offset"), "should have offset");
        assert!(conns.iter().any(|c| c.name == "m_axi_ptr_ARVALID"), "should have AXI");
    }

    #[test]
    fn fifo_instance_params() {
        use tapa_graphir::{Expression, Range};
        let range = Range {
            left: Expression::new_lit("32"),
            right: Expression::new_lit("0"),
        };
        let inst = build_fifo_instance("fifo_0", Some(&range), 16, Some("SLOT_X0Y0"), false);
        assert_eq!(inst.name, "fifo_0");
        assert_eq!(inst.module, "fifo");
        assert_eq!(inst.parameters.len(), 3);
        assert_eq!(inst.floorplan_region.as_deref(), Some("SLOT_X0Y0"));
        // Reset wiring for slot FIFO is `~ap_rst_n`.
        let reset = inst.connections.iter().find(|c| c.name == "reset").unwrap();
        assert_eq!(reset.expr.0.len(), 2);
        assert_eq!(reset.expr.0[0].repr, "~");
        assert_eq!(reset.expr.0[1].repr, "ap_rst_n");
        // DATA_WIDTH is folded to literal `33`.
        let dw = inst.parameters.iter().find(|p| p.name == "DATA_WIDTH").unwrap();
        assert_eq!(dw.expr.0.len(), 1);
        assert_eq!(dw.expr.0[0].repr, "33");
    }

    #[test]
    fn fifo_instance_top_reset() {
        let inst = build_fifo_instance("fifo_top", None, 16, None, true);
        let reset = inst.connections.iter().find(|c| c.name == "reset").unwrap();
        assert_eq!(reset.expr.0.len(), 1);
        assert_eq!(reset.expr.0[0].repr, "rst");
    }

    #[test]
    fn task_instance_has_control_ports() {
        let inst = build_task_instance("child_0", "child", Vec::new(), None);
        assert_eq!(inst.name, "child_0");
        // Should have ap_clk, ap_rst_n, ap_start, ap_done, ap_idle, ap_ready
        assert!(inst.connections.len() >= 6, "got {}", inst.connections.len());
    }

    #[test]
    fn arg_table_includes_scalars() {
        let top: tapa_topology::task::TaskDesign = serde_json::from_str(r#"{
            "level": "upper", "code": "", "target": "xilinx-hls",
            "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
            "tasks": {
                "child": [{"args": {"count": {"arg": "n", "cat": "scalar"}}}]
            },
            "fifos": {}
        }"#).unwrap();
        let table = build_arg_table(&top);
        let child_0 = &table["child_0"];
        assert_eq!(
            child_0["n"], "child_0___n__q0",
            "scalar should be in arg table with Python's __q0 queue-tail suffix"
        );
    }

    #[test]
    fn mmap_connection_filtered_by_rtl_ports() {
        let arg = ArgDesign {
            arg: "mem_a".into(),
            cat: ArgCategory::Mmap,
            extra: std::collections::BTreeMap::default(),
        };
        // Only include ARVALID, ARREADY, ARADDR from the child RTL + offset
        let known: std::collections::HashSet<String> = [
            "ptr_offset", "m_axi_ptr_ARVALID", "m_axi_ptr_ARREADY", "m_axi_ptr_ARADDR",
        ].into_iter().map(String::from).collect();
        let conns = build_port_connections("ptr", &arg, None, Some(&known), None);
        // Should only have offset + 3 AXI channels = 4
        assert_eq!(conns.len(), 4, "filtered conns: {conns:?}");
        assert!(conns.iter().any(|c| c.name == "ptr_offset"));
        assert!(conns.iter().any(|c| c.name == "m_axi_ptr_ARVALID"));
        assert!(conns.iter().any(|c| c.name == "m_axi_ptr_ARREADY"));
        assert!(conns.iter().any(|c| c.name == "m_axi_ptr_ARADDR"));
    }

    #[test]
    fn arg_table_includes_mmap_offsets() {
        let top: tapa_topology::task::TaskDesign = serde_json::from_str(r#"{
            "level": "upper", "code": "", "target": "xilinx-hls",
            "ports": [{"cat": "mmap", "name": "mem", "type": "int*", "width": 64}],
            "tasks": {
                "child": [{"args": {"ptr": {"arg": "mem", "cat": "mmap"}}}]
            },
            "fifos": {}
        }"#).unwrap();
        let table = build_arg_table(&top);
        let child_0 = &table["child_0"];
        assert_eq!(
            child_0["mem"], "child_0___mem_offset__q0",
            "mmap should be keyed on arg.arg and emit Python's _offset__q0 queue-tail wire"
        );
    }
}
