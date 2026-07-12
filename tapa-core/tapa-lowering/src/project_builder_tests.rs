//! Tests for `project_builder`.

use super::*;

use crate::utils::{fifo_wire_range, parse_instance_name, range_msb};

fn make_program() -> Program {
    serde_json::from_str(
        r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                    },
                    "fifos": {}
                },
                "child": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                    "tasks": {}, "fifos": {}
                }
            }
        }"#,
    )
    .unwrap()
}

#[test]
fn promotes_fsm_outputs_assigned_in_always_blocks() {
    let source = "
module slot_fsm (
  input wire ap_clk,
  output wire child__ap_start,
  output wire child__done
);
assign child__done = 1'b1;
always @(posedge ap_clk) begin
  child__ap_start <= 1'b1;
end
endmodule
";
    let module = tapa_rtl::VerilogModule::parse(source).unwrap();
    let mut module = tapa_rtl::mutation::MutableModule::from_parsed(module);

    module.promote_procedural_output_ports(source);

    let emitted = module.emit();
    assert!(
        emitted.contains("output reg child__ap_start"),
        "got:\n{emitted}"
    );
    assert!(
        emitted.contains("output wire child__done"),
        "continuous assignment output should remain a wire:\n{emitted}"
    );
}

#[test]
fn build_project_produces_modules() {
    let prog = make_program();
    let leaf_mods = BTreeMap::from([(
        "child".into(),
        AnyModuleDefinition::new_verilog(
            "child".into(),
            Vec::new(),
            "module child(); endmodule".into(),
        ),
    )]);
    let fsm_mods = BTreeMap::new();
    let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

    let project = build_project(
        &prog,
        &leaf_mods,
        &fsm_mods,
        None,
        &slot_to_insts,
        None,
        None,
        None,
    )
    .unwrap();

    assert!(project.has_module("top_task"), "should have top module");
    assert!(project.has_module("SLOT_0"), "should have slot module");
    assert!(project.has_module("child"), "should have leaf module");
    assert!(project.has_module("fifo"), "should have fifo template");
    assert!(
        project.has_module("reset_inverter"),
        "should have reset_inverter"
    );
    for module_name in ["top_task", "SLOT_0"] {
        let module = project
            .modules
            .module_definitions
            .iter()
            .find(|module| module.name() == module_name)
            .expect("grouped module");
        let AnyModuleDefinition::Grouped { grouped, .. } = module else {
            panic!("{module_name} should be grouped");
        };
        assert!(
            grouped.wires.iter().all(|wire| wire.name != "ap_rst"),
            "{module_name} should not declare an undriven ap_rst net"
        );
    }
}

#[test]
fn slot_child_instances_use_exported_leaf_module_name() {
    let prog = make_program();
    let leaf_mods = BTreeMap::from([(
        "child".into(),
        AnyModuleDefinition::new_verilog(
            "child_rtl".into(),
            Vec::new(),
            "module child_rtl(); endmodule".into(),
        ),
    )]);
    let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

    let project = build_project(
        &prog,
        &leaf_mods,
        &BTreeMap::new(),
        None,
        &slot_to_insts,
        None,
        None,
        None,
    )
    .expect("build project");

    let slot = project
        .modules
        .module_definitions
        .iter()
        .find(|m| m.name() == "SLOT_0")
        .expect("slot module");
    let AnyModuleDefinition::Grouped { grouped, .. } = slot else {
        panic!("slot should be grouped");
    };
    let child_inst = grouped
        .submodules
        .iter()
        .find(|m| m.name == "child_0")
        .expect("child instance");
    assert_eq!(child_inst.module, "child_rtl");
}

#[test]
fn slot_child_instances_resolve_explicit_instance_label_to_task() {
    let prog: Program = serde_json::from_str(
        r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "slot_task_name_to_fp_region": {
                "SLOT_0": "SLOT_0:SLOT_0"
            },
            "tasks": {
                "top_task": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "SLOT_0": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]
                    },
                    "fifos": {}
                },
                "SLOT_0": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "RealFunc": [{
                            "name": "AliasFunc#1",
                            "args": {"n": {"arg": "n", "cat": "scalar"}}
                        }]
                    },
                    "fifos": {}
                },
                "RealFunc": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                    "tasks": {}, "fifos": {}
                }
            }
        }"#,
    )
    .unwrap();
    let leaf_mods = BTreeMap::from([(
        "RealFunc".into(),
        AnyModuleDefinition::new_verilog(
            "RealFunc".into(),
            Vec::new(),
            "module RealFunc(); endmodule".into(),
        ),
    )]);
    let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["AliasFunc_1".into()])]);

    let project = build_project(
        &prog,
        &leaf_mods,
        &BTreeMap::new(),
        None,
        &slot_to_insts,
        None,
        None,
        None,
    )
    .expect("build project");

    let slot = project
        .modules
        .module_definitions
        .iter()
        .find(|m| m.name() == "SLOT_0")
        .expect("slot module");
    let AnyModuleDefinition::Grouped { grouped, .. } = slot else {
        panic!("slot should be grouped");
    };
    let child_inst = grouped
        .submodules
        .iter()
        .find(|m| m.name == "AliasFunc_1")
        .expect("explicitly named child instance");
    assert_eq!(child_inst.module, "RealFunc");
}

#[test]
fn explicit_instance_label_collision_does_not_duplicate_slot_connections() {
    let prog: Program = serde_json::from_str(
        r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "slot_task_name_to_fp_region": {
                "SLOT_0": "SLOT_0:SLOT_0"
            },
            "tasks": {
                "top_task": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                },
                "SLOT_0": {
                    "level": "upper", "code": "", "target": "xilinx-hls",
                    "is_slot": true,
                    "ports": [
                        {"cat": "istream", "name": "from_a", "type": "int", "width": 32},
                        {"cat": "ostream", "name": "to_a", "type": "int", "width": 32},
                        {"cat": "istream", "name": "from_b", "type": "int", "width": 32},
                        {"cat": "ostream", "name": "to_b", "type": "int", "width": 32}
                    ],
                    "tasks": {
                        "Leaf": [
                            {
                                "name": "Leaf#1",
                                "args": {
                                    "fifo_ld_0": {"arg": "from_a", "cat": "istream"},
                                    "fifo_st_0": {"arg": "to_a", "cat": "ostream"}
                                }
                            },
                            {
                                "name": "Leaf#3",
                                "args": {
                                    "fifo_ld_0": {"arg": "from_b", "cat": "istream"},
                                    "fifo_st_0": {"arg": "to_b", "cat": "ostream"}
                                }
                            }
                        ]
                    },
                    "fifos": {}
                },
                "Leaf": {
                    "level": "lower", "code": "", "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "fifo_ld_0", "type": "int", "width": 32},
                        {"cat": "ostream", "name": "fifo_st_0", "type": "int", "width": 32}
                    ],
                    "tasks": {}, "fifos": {}
                }
            }
        }"#,
    )
    .unwrap();
    let leaf_mods = BTreeMap::from([(
        "Leaf".into(),
        AnyModuleDefinition::new_verilog(
            "Leaf".into(),
            Vec::new(),
            "module Leaf(); endmodule".into(),
        ),
    )]);
    let top_arg_table = crate::instantiation_builder::build_arg_table(&prog.tasks[&prog.top]);
    let slot = build_slot_module(
        &prog,
        "SLOT_0",
        &["Leaf_1".into()],
        &leaf_mods,
        &BTreeMap::new(),
        &top_arg_table,
        None,
    );

    let AnyModuleDefinition::Grouped { grouped, .. } = slot else {
        panic!("slot should be grouped");
    };
    let child_inst = grouped
        .submodules
        .iter()
        .find(|m| m.name == "Leaf_1")
        .expect("explicitly named child instance");
    let mut seen = std::collections::BTreeSet::new();
    let duplicates: Vec<_> = child_inst
        .connections
        .iter()
        .filter_map(|conn| (!seen.insert(conn.name.clone())).then_some(conn.name.clone()))
        .collect();
    assert!(
        duplicates.is_empty(),
        "explicit label Leaf#1 must not also match positional Leaf_1; \
         duplicates: {duplicates:?}; connections: {:?}",
        child_inst.connections
    );
    let dout = child_inst
        .connections
        .iter()
        .find(|c| c.name == "fifo_ld_0_dout")
        .expect("istream data connection");
    assert_eq!(dout.expr.0[0].repr, "from_a_dout");
}

#[test]
fn parse_instance_name_works() {
    assert_eq!(parse_instance_name("producer_0"), ("producer".into(), 0));
    assert_eq!(parse_instance_name("child_a_2"), ("child_a".into(), 2));
    assert_eq!(parse_instance_name("single"), ("single".into(), 0));
}

#[test]
fn fifo_wire_range_only_applies_to_data_wires() {
    let range = range_msb(64);

    assert_eq!(fifo_wire_range("_din", Some(&range)), Some(range.clone()));
    assert_eq!(fifo_wire_range("_dout", Some(&range)), Some(range.clone()));
    assert_eq!(fifo_wire_range("_read", Some(&range)), None);
    assert_eq!(fifo_wire_range("_write", Some(&range)), None);
    assert_eq!(fifo_wire_range("_din", None), None);
}

#[test]
fn build_project_missing_top_task() {
    let prog: Program = serde_json::from_str(
        r#"{
        "top": "nonexistent",
        "target": "xilinx-hls",
        "tasks": {}
    }"#,
    )
    .unwrap();
    let result = build_project(
        &prog,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        &BTreeMap::new(),
        None,
        None,
        None,
    );
    assert!(result.is_err(), "should fail for missing top task");
}

#[test]
fn build_project_rejects_misnamed_ctrl_s_axi_module() {
    let state = TopologyWithRtl::new(make_program());
    let err = build_project_from_state(
        &state,
        "module wrong_name(); endmodule",
        &BTreeMap::new(),
        None,
        None,
    )
    .expect_err("control module name must match the top task");

    assert!(
        err.to_string()
            .contains("expected module `top_task_control_s_axi`, found `wrong_name`"),
        "unexpected error: {err}"
    );
}

#[test]
fn build_project_applies_interface_roles() {
    let prog = make_program();
    let leaf_mods = BTreeMap::from([(
        "child".into(),
        AnyModuleDefinition::new_verilog(
            "child".into(),
            Vec::new(),
            "module child(); endmodule".into(),
        ),
    )]);
    let fsm_mods = BTreeMap::new();
    let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

    let project = build_project(
        &prog,
        &leaf_mods,
        &fsm_mods,
        None,
        &slot_to_insts,
        None,
        None,
        None,
    )
    .expect("build succeeded");
    let ifaces = project.ifaces.as_ref().expect("project has interfaces");
    // The FIFO module has handshake interfaces — roles must be source/sink
    // after inference, never the default.
    let fifo_ifaces = ifaces
        .get("fifo")
        .expect("fifo module must have interfaces attached");
    let roles: std::collections::HashSet<_> =
        fifo_ifaces.iter().map(|i| i.base().role.clone()).collect();
    assert!(
        roles.contains("source") || roles.contains("sink"),
        "fifo must have at least one source or sink role, got {roles:?}"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn async_mmap_slot_exposes_axi_ports_to_top_instance() {
    let prog: Program = serde_json::from_str(
        r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "slot_task_name_to_fp_region": {"SLOT_X0Y0_SLOT_X0Y0": "SLOT_X0Y0:SLOT_X0Y0"},
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "async_mmap", "name": "chan[0]", "type": "int*", "width": 512}
                    ],
                    "tasks": {
                        "SLOT_X0Y0_SLOT_X0Y0": [
                            {
                                "name": "SLOT_X0Y0_SLOT_X0Y0_0",
                                "args": {
                                    "mem_Copy_0": {"arg": "chan[0]", "cat": "async_mmap"}
                                }
                            }
                        ]
                    },
                    "fifos": {}
                },
                "SLOT_X0Y0_SLOT_X0Y0": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "is_slot": true,
                    "ports": [
                        {"cat": "async_mmap", "name": "mem_Copy_0", "type": "int*", "width": 512}
                    ],
                    "tasks": {
                        "Copy": [
                            {
                                "name": "Copy_0",
                                "args": {
                                    "mem": {"arg": "mem_Copy_0", "cat": "async_mmap"}
                                }
                            }
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
    let copy_ports = vec![
        crate::utils::output_wire("mem_read_addr_s_din", Some(range_msb(63))),
        crate::utils::input_wire("mem_read_addr_s_full_n", None),
        crate::utils::output_wire("mem_read_addr_s_write", None),
        crate::utils::output_wire("mem_write_data_s_din", Some(range_msb(512))),
        crate::utils::input_wire("mem_write_data_s_full_n", None),
        crate::utils::output_wire("mem_write_data_s_write", None),
    ];
    let leaf_mods = BTreeMap::from([(
        "Copy".into(),
        AnyModuleDefinition::new_verilog(
            "Copy".into(),
            copy_ports,
            "module Copy(); endmodule".into(),
        ),
    )]);
    let slot_to_insts = BTreeMap::from([("SLOT_X0Y0_SLOT_X0Y0".into(), vec!["Copy_0".into()])]);

    let project = build_project(
        &prog,
        &leaf_mods,
        &BTreeMap::new(),
        None,
        &slot_to_insts,
        None,
        None,
        None,
    )
    .expect("build project");

    let slot_def = project
        .modules
        .module_definitions
        .iter()
        .find(|m| m.name() == "SLOT_X0Y0_SLOT_X0Y0")
        .expect("slot module");
    assert!(
        slot_def
            .ports()
            .iter()
            .any(|p| p.name == "m_axi_mem_Copy_0_AWADDR"),
        "slot boundary must expose the async bridge AXI ports"
    );

    let top_def = project
        .modules
        .module_definitions
        .iter()
        .find(|m| m.name() == "top_task")
        .expect("top module");
    let AnyModuleDefinition::Grouped { grouped, .. } = top_def else {
        panic!("top should be grouped");
    };
    let slot_inst = grouped
        .submodules
        .iter()
        .find(|m| m.name == "SLOT_X0Y0_SLOT_X0Y0_0")
        .expect("slot instance");
    assert!(
        slot_inst
            .connections
            .iter()
            .any(|c| c.name == "m_axi_mem_Copy_0_AWADDR"
                && c.expr.0.len() == 1
                && c.expr.0[0].repr == "m_axi_chan_0_AWADDR"),
        "top slot instance must connect async mmap AXI ports, got {:?}",
        slot_inst.connections
    );

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top_task",
            tapa_rtl::VerilogModule::parse(
                "module top_task(input wire ap_clk, input wire ap_rst_n); endmodule",
            )
            .unwrap(),
        )
        .unwrap();
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
    let rewritten_project = build_project_from_state(
        &state,
        "module top_task_control_s_axi(); endmodule",
        &slot_to_insts,
        None,
        None,
    )
    .expect("build project from state");
    let rewritten_top = rewritten_project
        .modules
        .module_definitions
        .iter()
        .find(|m| m.name() == "top_task")
        .expect("rewritten top module");
    let AnyModuleDefinition::Grouped { grouped, .. } = rewritten_top else {
        panic!("rewritten top should be grouped");
    };
    let rewritten_slot_inst = grouped
        .submodules
        .iter()
        .find(|m| m.name == "SLOT_X0Y0_SLOT_X0Y0_0")
        .expect("rewritten slot instance");
    assert!(
        rewritten_slot_inst
            .connections
            .iter()
            .any(|c| c.name == "m_axi_mem_Copy_0_AWADDR"
                && c.expr.0.len() == 1
                && c.expr.0[0].repr == "m_axi_chan_0_AWADDR"),
        "slot-port rewrite must preserve async mmap AXI top connections, got {:?}",
        rewritten_slot_inst.connections
    );
}

#[test]
fn build_project_rejects_invalid_interface_direction() {
    let prog = make_program();
    let leaf_mods = BTreeMap::new();
    let fsm_mods = BTreeMap::new();
    // No ctrl_s_axi → no s_axi_control_* ports → no top handshakes.
    let slot_to_insts = BTreeMap::from([("SLOT_0".into(), vec!["child_0".into()])]);

    // Build the project normally, then deliberately corrupt the fifo's
    // if_write port direction so role inference fails.
    let mut project = build_project(
        &prog,
        &leaf_mods,
        &fsm_mods,
        None,
        &slot_to_insts,
        None,
        None,
        None,
    )
    .expect("build succeeded");
    for def in &mut project.modules.module_definitions {
        if let AnyModuleDefinition::Verilog { base, .. } = def {
            if base.name == "fifo" {
                for port in &mut base.ports {
                    if port.name == "if_write" {
                        port.port_type = "output wire".into();
                    }
                }
            }
        }
    }
    // Re-run role inference — should now fail.
    let mut ifaces = project.ifaces.clone().unwrap_or_default();
    let defs = project.modules.module_definitions.clone();
    let err = crate::iface_roles::apply_iface_roles(&defs, &mut ifaces)
        .expect_err("corrupted fifo must fail role inference");
    assert!(
        matches!(err, LoweringError::InterfaceDirection(_)),
        "expected InterfaceDirection error, got: {err:?}"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn aggregate_slot_params_use_alphabetical_order() {
    // The input lists `zleaf` first, but `BTreeMap` iteration makes
    // `aleaf` the first parameter source.
    let prog_json = r#"{
        "top": "top_task",
        "target": "xilinx-hls",
        "tasks": {
            "top_task": {
                "level": "upper", "code": "", "target": "xilinx-hls",
                "ports": [],
                "tasks": {
                    "slot_A": [{"args": {}}]
                },
                "fifos": {}
            },
            "slot_A": {
                "level": "upper", "code": "", "target": "xilinx-hls",
                "is_slot": true,
                "ports": [],
                "tasks": {
                    "zleaf": [{"args": {}}],
                    "aleaf": [{"args": {}}]
                },
                "fifos": {}
            },
            "zleaf": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [], "tasks": {}, "fifos": {}
            },
            "aleaf": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [], "tasks": {}, "fifos": {}
            }
        }
    }"#;
    let prog: Program = serde_json::from_str(prog_json).unwrap();
    // Slot children iterate alphabetically.
    let slot_a = prog.tasks.get("slot_A").unwrap();
    let keys: Vec<&String> = slot_a.tasks.keys().collect();
    assert_eq!(
        keys,
        vec!["aleaf", "zleaf"],
        "BTreeMap must iterate alphabetically"
    );

    // Build a minimal TopologyWithRtl where each leaf carries a
    // distinct expression for parameter `P`. aggregate_slot_leaf_parameters
    // should pick the first-seen (alphabetical = `aleaf`) expression.
    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(prog);
    let make_rtl = |mod_name: &str, p_val: &str| -> tapa_rtl::VerilogModule {
        let src = format!("module {mod_name} #(parameter P = {p_val}) (); endmodule\n");
        tapa_rtl::VerilogModule::parse(&src).unwrap()
    };
    state
        .attach_module("zleaf", make_rtl("zleaf", "10'd1"))
        .expect("attach zleaf");
    state
        .attach_module("aleaf", make_rtl("aleaf", "3'd1"))
        .expect("attach aleaf");

    let slot_to_insts =
        BTreeMap::from([("slot_A".into(), vec!["aleaf_0".into(), "zleaf_0".into()])]);
    let fsm_mods = BTreeMap::new();
    let leaf_mods = BTreeMap::from([
        (
            "zleaf".into(),
            AnyModuleDefinition::new_verilog(
                "zleaf".into(),
                Vec::new(),
                "module zleaf; endmodule".into(),
            ),
        ),
        (
            "aleaf".into(),
            AnyModuleDefinition::new_verilog(
                "aleaf".into(),
                Vec::new(),
                "module aleaf; endmodule".into(),
            ),
        ),
    ]);

    let project = build_project(
        &state.program,
        &leaf_mods,
        &fsm_mods,
        None,
        &slot_to_insts,
        None,
        None,
        Some(&state),
    )
    .expect("build succeeded");

    // Run the same post-pass aggregation `build_project_from_state`
    // would run — this is where the ordering matters.
    let mut project = project;
    aggregate_slot_leaf_parameters(&mut project, &state, &slot_to_insts);

    let slot_def = project
        .modules
        .module_definitions
        .iter()
        .find(|m| m.name() == "slot_A")
        .expect("slot_A module");
    let AnyModuleDefinition::Grouped { base, .. } = slot_def else {
        panic!("slot_A should be grouped");
    };
    let p_param = base
        .parameters
        .iter()
        .find(|p| p.name == "P")
        .expect("slot_A should carry aggregated parameter P");
    assert_eq!(
        p_param.expr.0.len(),
        1,
        "expected single-token expression, got {:?}",
        p_param.expr.0
    );
    assert_eq!(
        p_param.expr.0[0].repr, "3'd1",
        "alphabetical-first leaf `aleaf` (P = 3'd1) should win, matching \
         dict(sorted(...)) semantics — not `zleaf`'s 10'd1"
    );
}
