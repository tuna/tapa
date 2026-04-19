//! Python-equivalent slot grouped-module port synthesis.
//!
//! Mirrors `tapa/graphir_conversion/pipeline/ports_builder.py::
//! get_slot_module_definition_ports` and
//! `tapa/graphir_conversion/utils.py::get_child_port_connection_mapping`.
//! See [`build_slot_ports_python_equivalent`] for the entry point.

use std::collections::BTreeMap;

use tapa_codegen::rtl_state::TopologyWithRtl;
use tapa_graphir::{AnyModuleDefinition, HierarchicalName, ModulePort};

/// Build the slot grouped-module ports by mirroring Python's
/// `get_slot_module_definition_ports`.
///
/// Returns `None` when any required lookup (slot task, child task,
/// child RTL, child IR) is missing — caller keeps the prior port list.
pub fn build_slot_ports_python_equivalent(
    slot_name: &str,
    state: &TopologyWithRtl,
    leaf_modules: &BTreeMap<String, AnyModuleDefinition>,
) -> Option<Vec<ModulePort>> {
    let program = &state.program;
    let slot_task = program.tasks.get(slot_name)?;
    let mut ports: Vec<ModulePort> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for port_def in &slot_task.ports {
        let Some((child_task_name, child_inst_port, child_inst_port_idx)) =
            find_port_child(slot_task, &port_def.name)
        else {
            continue;
        };
        let child_task = program.tasks.get(&child_task_name)?;
        let task_port = child_task
            .ports
            .iter()
            .find(|p| p.name == child_inst_port)?;
        if task_port.cat == tapa_task_graph::port::ArgCategory::AsyncMmap {
            for port in
                crate::utils::expand_port_to_signals(&port_def.name, task_port.cat, task_port.width)
            {
                if seen.insert(port.name.clone()) {
                    ports.push(port);
                }
            }
            continue;
        }
        let child_rtl = state.module_map.get(&child_task_name)?;
        let child_ir = leaf_modules.get(&child_task_name)?;
        let port_map = get_child_port_connection_mapping_rs(
            task_port,
            &child_rtl.inner,
            &port_def.name,
            child_inst_port_idx,
        );
        for (child_port_name, slot_port_name) in port_map {
            let Some(child_ir_port) = child_ir.ports().iter().find(|p| p.name == child_port_name)
            else {
                continue;
            };
            if !seen.insert(slot_port_name.clone()) {
                continue;
            }
            ports.push(ModulePort {
                hierarchical_name: HierarchicalName::get_name(&slot_port_name),
                name: slot_port_name,
                port_type: child_ir_port.port_type.clone(),
                range: child_ir_port.range.clone(),
                extra: BTreeMap::default(),
            });
        }
    }
    // Append handshake ports in Python's order.
    for &(name, is_input) in &[
        ("ap_clk", true),
        ("ap_rst_n", true),
        ("ap_start", true),
        ("ap_done", false),
        ("ap_ready", false),
        ("ap_idle", false),
    ] {
        if seen.insert(name.to_owned()) {
            let port = if is_input {
                crate::utils::input_wire(name, None)
            } else {
                crate::utils::output_wire(name, None)
            };
            ports.push(port);
        }
    }
    Some(ports)
}

/// Rust port of `tapa/graphir_conversion/utils.py::get_child_port_connection_mapping`.
///
/// Returns an ordered list of `(child_rtl_port_name, slot_port_name)`
/// pairs.
fn get_child_port_connection_mapping_rs(
    task_port: &tapa_topology::task::PortDesign,
    task_module_rtl: &tapa_rtl::VerilogModule,
    arg: &str,
    idx: Option<u32>,
) -> Vec<(String, String)> {
    use tapa_task_graph::port::ArgCategory as Cat;
    let mut mapping: Vec<(String, String)> = Vec::new();
    match task_port.cat {
        Cat::Scalar => {
            mapping.push((
                task_port.name.clone(),
                tapa_rtl::module::sanitize_array_name(arg),
            ));
        }
        Cat::Istream | Cat::Istreams => {
            emit_stream_mapping(
                &mut mapping,
                task_module_rtl,
                task_port,
                arg,
                idx,
                crate::utils::ISTREAM_SUFFIXES,
            );
        }
        Cat::Ostream | Cat::Ostreams => {
            emit_stream_mapping(
                &mut mapping,
                task_module_rtl,
                task_port,
                arg,
                idx,
                crate::utils::OSTREAM_SUFFIXES,
            );
        }
        Cat::Mmap | Cat::AsyncMmap | Cat::Immap | Cat::Ommap => {
            mapping.push((
                format!("{}_offset", task_port.name),
                crate::utils::stream_port_name(arg, "_offset"),
            ));
            let rtl_port_names: std::collections::BTreeSet<&str> = task_module_rtl
                .ports
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            for suffix in crate::utils::M_AXI_READ_SUFFIXES
                .iter()
                .chain(crate::utils::M_AXI_WRITE_SUFFIXES.iter())
            {
                let m_axi_port =
                    format!("{}{}{}", crate::utils::M_AXI_PREFIX, task_port.name, suffix);
                if rtl_port_names.contains(m_axi_port.as_str()) {
                    mapping.push((m_axi_port, crate::utils::m_axi_port_name(arg, suffix)));
                }
            }
        }
    }
    mapping
}

/// Mirror Python's `_find_port_child`: locate any child instance whose
/// `arg.arg` equals `slot_port_name`, returning `(child_task, child_port,
/// array_idx)`. The `array_idx` is present when the child port name is
/// array-subscripted (e.g. `stream_q[3]`), and the returned `child_port`
/// is the base name with brackets stripped.
fn find_port_child(
    slot_task: &tapa_topology::task::TaskDesign,
    slot_port_name: &str,
) -> Option<(String, String, Option<u32>)> {
    for (child_task_name, insts) in &slot_task.tasks {
        for inst in insts {
            for (child_port, arg) in &inst.args {
                if arg.arg != slot_port_name {
                    continue;
                }
                return Some(match tapa_rtl::module::match_array_name(child_port) {
                    Some((base, idx)) => (child_task_name.clone(), base.to_owned(), Some(idx)),
                    None => (child_task_name.clone(), child_port.clone(), None),
                });
            }
        }
    }
    None
}

/// Append `(child_rtl_port, slot_port)` pairs for a stream-category port.
///
/// Applies the optional `_{idx}` array suffix to the task port base name,
/// then for each stream suffix looks up the child RTL port via
/// `get_port_of` and pairs it with the corresponding `{arg}{suffix}`
/// slot-visible wire name.
fn emit_stream_mapping(
    mapping: &mut Vec<(String, String)>,
    task_module_rtl: &tapa_rtl::VerilogModule,
    task_port: &tapa_topology::task::PortDesign,
    arg: &str,
    idx: Option<u32>,
    suffixes: &[&str],
) {
    let full = match idx {
        Some(i) => format!("{}_{}", task_port.name, i),
        None => task_port.name.clone(),
    };
    for suffix in suffixes {
        if let Some(rtl_port) = task_module_rtl.get_port_of(&full, suffix) {
            mapping.push((
                rtl_port.name.clone(),
                crate::utils::stream_port_name(arg, suffix),
            ));
        }
    }
}

/// Auto-declare wires for submodule-connection identifiers missing from ports.
///
/// Walks each submodule's connection expressions; any bare identifier
/// that is not yet declared as a port in the current port-name set and
/// not already present in `grouped.wires` gets appended as a new wire
/// (with no range). This preserves the post-port-replacement invariant
/// that every submodule connection references a declared signal, which
/// the exporter's DRC pass requires.
pub fn declare_missing_connection_wires(
    grouped: &mut tapa_graphir::GroupedFields,
    ports: &std::collections::BTreeSet<String>,
) {
    let mut wire_names: std::collections::BTreeSet<String> =
        grouped.wires.iter().map(|w| w.name.clone()).collect();
    let mut to_add: Vec<String> = Vec::new();
    for inst in &grouped.submodules {
        for conn in &inst.connections {
            for tok in &conn.expr.0 {
                if !tok.is_id() {
                    continue;
                }
                let name = &tok.repr;
                if ports.contains(name) || wire_names.contains(name) {
                    continue;
                }
                if !to_add.iter().any(|n| n == name) {
                    to_add.push(name.clone());
                }
            }
        }
    }
    for name in to_add {
        wire_names.insert(name.clone());
        grouped.wires.push(crate::utils::make_wire(&name, None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use tapa_rtl::port::{Direction, Port};
    use tapa_task_graph::port::ArgCategory;
    use tapa_topology::task::PortDesign;

    fn port(name: &str) -> Port {
        Port {
            name: name.to_owned(),
            direction: Direction::Input,
            width: None,
            pragma: None,
        }
    }

    fn module_with_ports(names: &[&str]) -> tapa_rtl::VerilogModule {
        tapa_rtl::VerilogModule {
            name: "Child".to_owned(),
            ports: names.iter().map(|name| port(name)).collect(),
            parameters: Vec::new(),
            signals: Vec::new(),
            pragmas: Vec::new(),
            source: String::new(),
        }
    }

    fn task_port(name: &str, cat: ArgCategory) -> PortDesign {
        PortDesign {
            cat,
            name: name.to_owned(),
            ctype: "int".to_owned(),
            width: 32,
            chan_count: None,
            chan_size: None,
            extra: BTreeMap::default(),
        }
    }

    #[test]
    fn stream_slot_ports_sanitize_indexed_arg_names() {
        let module = module_with_ports(&["a_0_dout", "a_0_empty_n", "a_0_read"]);
        let mapping = get_child_port_connection_mapping_rs(
            &task_port("a", ArgCategory::Istreams),
            &module,
            "a[0]_Cannon",
            Some(0),
        );

        assert_eq!(
            mapping,
            vec![
                ("a_0_dout".to_owned(), "a_0_Cannon_dout".to_owned()),
                ("a_0_empty_n".to_owned(), "a_0_Cannon_empty_n".to_owned()),
                ("a_0_read".to_owned(), "a_0_Cannon_read".to_owned()),
            ],
        );
    }

    #[test]
    fn mmap_slot_ports_sanitize_indexed_arg_names() {
        let module = module_with_ports(&["mem_offset", "m_axi_mem_ARVALID"]);
        let mapping = get_child_port_connection_mapping_rs(
            &task_port("mem", ArgCategory::Mmap),
            &module,
            "mem[1]_Cannon",
            None,
        );

        assert_eq!(
            mapping,
            vec![
                ("mem_offset".to_owned(), "mem_1_Cannon_offset".to_owned()),
                (
                    "m_axi_mem_ARVALID".to_owned(),
                    "m_axi_mem_1_Cannon_ARVALID".to_owned()
                ),
            ],
        );
    }

    #[test]
    fn async_mmap_slot_ports_expose_bridge_axi_boundary() {
        let prog: tapa_topology::program::Program = serde_json::from_str(
            r#"{
                "top": "top",
                "target": "xilinx-hls",
                "tasks": {
                    "top": {
                        "level": "upper",
                        "code": "",
                        "target": "xilinx-hls",
                        "ports": [],
                        "tasks": {},
                        "fifos": {}
                    },
                    "slot": {
                        "level": "upper",
                        "code": "",
                        "target": "xilinx-hls",
                        "ports": [
                            {"cat": "async_mmap", "name": "mem_Copy_0", "type": "int*", "width": 512}
                        ],
                        "tasks": {
                            "Copy": [
                                {"name": "Copy_0", "args": {"mem": {"arg": "mem_Copy_0", "cat": "async_mmap"}}}
                            ]
                        },
                        "fifos": {}
                    },
                    "Copy": {
                        "level": "lower",
                        "code": "",
                        "target": "xilinx-hls",
                        "ports": [
                            {"cat": "async_mmap", "name": "mem", "type": "int*", "width": 512}
                        ],
                        "tasks": {},
                        "fifos": {}
                    }
                }
            }"#,
        )
        .unwrap();
        let mut state = TopologyWithRtl::new(prog);
        state
            .attach_module(
                "Copy",
                tapa_rtl::VerilogModule::parse(
                    "module Copy(\n\
                     output wire [63:0] mem_read_addr_s_din,\n\
                     input wire mem_read_addr_s_full_n,\n\
                     output wire mem_read_addr_s_write,\n\
                     output wire [512:0] mem_write_data_s_din,\n\
                     input wire mem_write_data_s_full_n,\n\
                     output wire mem_write_data_s_write\n\
                     ); endmodule",
                )
                .unwrap(),
            )
            .unwrap();
        let leaf_modules = BTreeMap::from([(
            "Copy".to_owned(),
            AnyModuleDefinition::new_verilog(
                "Copy".to_owned(),
                vec![
                    crate::utils::output_wire(
                        "mem_read_addr_s_din",
                        Some(crate::utils::range_msb(63)),
                    ),
                    crate::utils::input_wire("mem_read_addr_s_full_n", None),
                    crate::utils::output_wire("mem_read_addr_s_write", None),
                    crate::utils::output_wire(
                        "mem_write_data_s_din",
                        Some(crate::utils::range_msb(512)),
                    ),
                    crate::utils::input_wire("mem_write_data_s_full_n", None),
                    crate::utils::output_wire("mem_write_data_s_write", None),
                ],
                "module Copy(); endmodule".to_owned(),
            ),
        )]);

        let ports = build_slot_ports_python_equivalent("slot", &state, &leaf_modules).unwrap();
        let port_names = ports.iter().map(|p| p.name.as_str()).collect::<Vec<_>>();
        assert!(
            port_names.contains(&"m_axi_mem_Copy_0_AWADDR"),
            "async mmap slot boundary must expose bridge AXI ports, got {port_names:?}"
        );
    }
}
