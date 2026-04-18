//! Integration tests exercising the same Rust code paths that the `PyO3`
//! bindings call, without requiring a Python interpreter.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// gen_slot_cpp
// ---------------------------------------------------------------------------

#[test]
fn gen_slot_cpp_produces_expected_output() {
    let ports = vec![
        tapa_slotting::SlotPort {
            cat: "scalar".to_owned(),
            name: "n".to_owned(),
            port_type: "int".to_owned(),
        },
        tapa_slotting::SlotPort {
            cat: "istream".to_owned(),
            name: "in_data".to_owned(),
            port_type: "float".to_owned(),
        },
        tapa_slotting::SlotPort {
            cat: "ostream".to_owned(),
            name: "out_data".to_owned(),
            port_type: "float".to_owned(),
        },
    ];

    let top_cpp =
        "extern \"C\" {\nvoid top_func(int a) { /* body */ }\n}  // extern \"C\"\n";

    let result =
        tapa_slotting::gen_slot_cpp("my_slot", "top_func", &ports, top_cpp)
            .expect("gen_slot_cpp should succeed");

    // The output should contain the slot function name
    assert!(
        result.contains("my_slot"),
        "output should contain slot name 'my_slot', got:\n{result}"
    );

    // The scalar port should appear as a plain C++ parameter
    assert!(
        result.contains("int n"),
        "output should contain 'int n' for scalar port, got:\n{result}"
    );

    // Stream ports should use tapa::istream / tapa::ostream wrappers
    assert!(
        result.contains("tapa::istream<float>"),
        "output should contain istream wrapper, got:\n{result}"
    );
    assert!(
        result.contains("tapa::ostream<float>"),
        "output should contain ostream wrapper, got:\n{result}"
    );

    // HLS pragmas should be present
    assert!(
        result.contains("#pragma HLS"),
        "output should contain HLS pragmas, got:\n{result}"
    );

    // The original top_func body should have been removed
    assert!(
        !result.contains("/* body */"),
        "output should not contain original top_func body, got:\n{result}"
    );
}

// ---------------------------------------------------------------------------
// replace_function rejects empty source
// ---------------------------------------------------------------------------

#[test]
fn replace_function_rejects_empty_source() {
    let result =
        tapa_slotting::cpp_surgery::replace_function("", "any_func", "new body", None);
    assert!(result.is_err(), "empty source should be rejected");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("empty"),
        "error should mention empty, got: {err}"
    );
}

#[test]
fn replace_function_rejects_whitespace_only_source() {
    let result =
        tapa_slotting::cpp_surgery::replace_function("   \n\t  ", "func", "body", None);
    assert!(
        result.is_err(),
        "whitespace-only source should be rejected"
    );
}

// ---------------------------------------------------------------------------
// TopologyWithRtl rejects unknown task attachment
// ---------------------------------------------------------------------------

#[test]
fn topology_with_rtl_rejects_unknown_task() {
    let program: tapa_topology::program::Program = serde_json::from_str(
        r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }"#,
    )
    .expect("program JSON should parse");

    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);
    let module = tapa_rtl::VerilogModule::parse("module unknown_task(); endmodule")
        .expect("trivial module should parse");

    let err = state
        .attach_module("nonexistent_task", module)
        .expect_err("attaching to unknown task should fail");

    assert!(
        err.to_string().contains("not found"),
        "error should mention 'not found', got: {err}"
    );
}

// ---------------------------------------------------------------------------
// generate_rtl runs without error on a minimal design
// ---------------------------------------------------------------------------

#[test]
fn generate_rtl_minimal_design() {
    // A minimal design: upper task with one lower child connected via a FIFO.
    let program: tapa_topology::program::Program = serde_json::from_str(
        r#"{
            "top": "top_task",
            "target": "xilinx-hls",
            "tasks": {
                "top_task": {
                    "level": "upper",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "input_data", "type": "float", "width": 32}
                    ],
                    "tasks": {
                        "child_a": [{"args": {"data": {"arg": "input_data", "cat": "istream"}}}]
                    },
                    "fifos": {
                        "input_data": {
                            "consumed_by": ["child_a", 0]
                        }
                    }
                },
                "child_a": {
                    "level": "lower",
                    "code": "",
                    "target": "xilinx-hls",
                    "ports": [
                        {"cat": "istream", "name": "data", "type": "float", "width": 32}
                    ],
                    "tasks": {},
                    "fifos": {}
                }
            }
        }"#,
    )
    .expect("program JSON should parse");

    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);

    // Attach a minimal Verilog module for the upper task
    let top_verilog = "\
module top_task (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  input wire [31:0] input_data_dout,
  input wire input_data_empty_n,
  output wire input_data_read
);
endmodule
";
    let top_module = tapa_rtl::VerilogModule::parse(top_verilog)
        .expect("top_task Verilog should parse");
    state
        .attach_module("top_task", top_module)
        .expect("attaching top_task should succeed");

    // Attach a minimal Verilog module for the child
    let child_verilog = "\
module child_a (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  input wire [31:0] data_dout,
  input wire data_empty_n,
  output wire data_read
);
endmodule
";
    let child_module = tapa_rtl::VerilogModule::parse(child_verilog)
        .expect("child_a Verilog should parse");
    state
        .attach_module("child_a", child_module)
        .expect("attaching child_a should succeed");

    let gen_result = tapa_codegen::generate_rtl(&mut state);
    assert!(
        gen_result.is_ok(),
        "generate_rtl should succeed on minimal design, got: {:?}",
        gen_result.err()
    );

    // Should produce at least some generated files
    assert!(
        !state.generated_files.is_empty(),
        "generate_rtl should produce generated files"
    );
}

// ---------------------------------------------------------------------------
// VerilogModule parse -> MutableModule -> emit -> reparse round-trip
// ---------------------------------------------------------------------------

#[test]
fn verilog_parse_mutate_emit_reparse_roundtrip() {
    let source = "\
module test_mod (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire [31:0] data_in,
  output wire [31:0] data_out
);
  wire [31:0] internal_sig;
  assign data_out = internal_sig;
endmodule
";

    // Step 1: parse
    let parsed = tapa_rtl::VerilogModule::parse(source)
        .expect("initial parse should succeed");
    assert_eq!(parsed.name, "test_mod", "module name should be test_mod");
    assert_eq!(parsed.ports.len(), 4, "should have 4 ports");

    // Step 2: convert to mutable module
    let mm = tapa_rtl::mutation::MutableModule::from_parsed(parsed);

    // Step 3: emit
    let emitted = mm.emit();
    assert!(
        emitted.contains("test_mod"),
        "emitted Verilog should contain module name"
    );
    assert!(
        emitted.contains("ap_clk"),
        "emitted Verilog should contain ap_clk port"
    );
    assert!(
        emitted.contains("data_out"),
        "emitted Verilog should contain data_out port"
    );

    // Step 4: reparse the emitted Verilog
    let reparsed = tapa_rtl::VerilogModule::parse(&emitted)
        .expect("reparsing emitted Verilog should succeed");
    assert_eq!(
        reparsed.name, "test_mod",
        "reparsed module name should still be test_mod"
    );
    assert_eq!(
        reparsed.ports.len(), 4,
        "reparsed module should still have 4 ports"
    );

    // Verify port names survived the round-trip
    let port_names: Vec<&str> = reparsed.ports.iter().map(|p| p.name.as_str()).collect();
    assert!(port_names.contains(&"ap_clk"), "should contain ap_clk");
    assert!(port_names.contains(&"ap_rst_n"), "should contain ap_rst_n");
    assert!(port_names.contains(&"data_in"), "should contain data_in");
    assert!(port_names.contains(&"data_out"), "should contain data_out");
}

// ---------------------------------------------------------------------------
// floorplan round-trip through JSON (same code path as slotting_mod binding)
// ---------------------------------------------------------------------------

#[test]
fn floorplan_graph_json_roundtrip() {
    let graph_json = r#"{
        "top": "top_func",
        "tasks": {
            "top_func": {
                "level": "upper",
                "code": "extern \"C\" {\nvoid top_func(int a) { /* body */ }\n}  // extern \"C\"\n",
                "target": "xilinx-hls",
                "ports": [
                    {"cat": "scalar", "name": "size", "type": "int", "width": 32}
                ],
                "tasks": {
                    "producer": [
                        {"args": {"data_out": {"arg": "fifo_0", "cat": "ostream"}, "n": {"arg": "size", "cat": "scalar"}}, "step": 0}
                    ],
                    "consumer": [
                        {"args": {"data_in": {"arg": "fifo_0", "cat": "istream"}, "n": {"arg": "size", "cat": "scalar"}}, "step": 1}
                    ]
                },
                "fifos": {
                    "fifo_0": {
                        "depth": 16,
                        "consumed_by": ["consumer", 0],
                        "produced_by": ["producer", 0]
                    }
                }
            },
            "producer": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [
                    {"cat": "ostream", "name": "data_out", "type": "float", "width": 32},
                    {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                ],
                "tasks": {}, "fifos": {}
            },
            "consumer": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [
                    {"cat": "istream", "name": "data_in", "type": "float", "width": 32},
                    {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                ],
                "tasks": {}, "fifos": {}
            }
        }
    }"#;

    let graph: serde_json::Value =
        serde_json::from_str(graph_json).expect("graph JSON should parse");
    let mut slot_to_insts = BTreeMap::new();
    slot_to_insts.insert(
        "SLOT_0".to_owned(),
        vec!["producer_0".to_owned(), "consumer_0".to_owned()],
    );

    let result = tapa_slotting::floorplan::get_floorplan_graph(&graph, &slot_to_insts)
        .expect("floorplan should succeed");

    // Verify the result can be serialized back to JSON
    let json_str = serde_json::to_string_pretty(&result)
        .expect("result should serialize to JSON");
    assert!(
        !json_str.is_empty(),
        "serialized result should not be empty"
    );

    // Verify it can be reparsed
    let reparsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("result should reparse from JSON");
    assert!(
        reparsed["tasks"]["SLOT_0"].is_object(),
        "reparsed result should contain SLOT_0 task"
    );
}

// ---------------------------------------------------------------------------
// tapa-floorplan: ABGraph generation round-trip
// ---------------------------------------------------------------------------

#[test]
fn floorplan_abgraph_generation() {
    let program_json = r#"{
        "top": "top_task",
        "target": "xilinx-hls",
        "tasks": {
            "top_task": {
                "level": "upper",
                "code": "",
                "target": "xilinx-hls",
                "ports": [
                    {"cat": "istream", "name": "input_data", "type": "float", "width": 32},
                    {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                ],
                "tasks": {
                    "producer": [{"args": {"out": {"arg": "fifo_0", "cat": "ostream"}, "n": {"arg": "n", "cat": "scalar"}}}],
                    "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}}}]
                },
                "fifos": {
                    "fifo_0": {"depth": 16, "consumed_by": ["consumer", 0], "produced_by": ["producer", 0]}
                }
            },
            "producer": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [
                    {"cat": "ostream", "name": "out", "type": "float", "width": 32},
                    {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                ],
                "tasks": {}, "fifos": {}
            },
            "consumer": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [{"cat": "istream", "name": "in_data", "type": "float", "width": 32}],
                "tasks": {}, "fifos": {}
            }
        }
    }"#;

    let program: tapa_topology::program::Program =
        serde_json::from_str(program_json).expect("program should parse");
    let preassignments = BTreeMap::new();

    let graph = tapa_floorplan::gen_abgraph::get_top_level_ab_graph(
        &program,
        &preassignments,
        "top_task_fsm",
    )
    .expect("should generate graph");

    // Verify the graph has vertices and edges
    assert!(graph.vs.len() >= 2, "should have at least 2 vertices, got {}", graph.vs.len());
    assert!(!graph.es.is_empty(), "should have edges");

    // JSON round-trip
    let json = serde_json::to_string(&graph).expect("should serialize");
    let reparsed: tapa_floorplan::ABGraph =
        serde_json::from_str(&json).expect("should reparse");
    assert_eq!(graph.vs.len(), reparsed.vs.len());
}

// ---------------------------------------------------------------------------
// tapa-lowering: Project building
// ---------------------------------------------------------------------------

#[test]
fn lowering_build_project() {
    let program_json = r#"{
        "top": "top_task",
        "target": "xilinx-hls",
        "tasks": {
            "top_task": {
                "level": "upper", "code": "", "target": "xilinx-hls",
                "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                "tasks": {"child": [{"args": {"n": {"arg": "n", "cat": "scalar"}}}]},
                "fifos": {}
            },
            "child": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [{"cat": "scalar", "name": "n", "type": "int", "width": 32}],
                "tasks": {}, "fifos": {}
            }
        }
    }"#;

    let program: tapa_topology::program::Program =
        serde_json::from_str(program_json).expect("program should parse");
    let leaf_modules = BTreeMap::from([(
        "child".to_owned(),
        tapa_graphir::AnyModuleDefinition::new_verilog(
            "child".into(),
            Vec::new(),
            "module child(); endmodule".into(),
        ),
    )]);
    let slot_to_instances = BTreeMap::from([
        ("SLOT_0".to_owned(), vec!["child_0".to_owned()]),
    ]);

    let project = tapa_lowering::build_project(
        &program,
        &leaf_modules,
        &BTreeMap::new(),
        None,
        &slot_to_instances,
        None,
        None,
        None,
    )
    .expect("should build project");

    assert!(project.has_module("top_task"), "should have top module");
    assert!(project.has_module("SLOT_0"), "should have slot module");
    assert!(project.has_module("child"), "should have leaf module");

    // JSON round-trip
    let json = project.to_json().expect("should serialize");
    let reparsed = tapa_graphir::Project::from_json(&json);
    assert!(reparsed.is_ok(), "should reparse, got: {:?}", reparsed.err());
}

// ---------------------------------------------------------------------------
// tapa-graphir-export: Verilog rendering
// ---------------------------------------------------------------------------

#[test]
fn graphir_export_renders_grouped_module() {
    let module = tapa_graphir::AnyModuleDefinition::new_grouped(
        "test_mod".into(),
        vec![tapa_graphir::ModulePort {
            name: "clk".into(),
            hierarchical_name: tapa_graphir::HierarchicalName::none(),
            port_type: "input wire".into(),
            range: None,
            extra: BTreeMap::default(),
        }],
        Vec::new(),
        Vec::new(),
    );

    let verilog = tapa_graphir_export::verilog::render_module(&module);
    assert!(verilog.contains("module test_mod"), "should contain module name");
    assert!(verilog.contains("input wire"), "should contain port type");
    assert!(verilog.contains("endmodule"), "should contain endmodule");
}

#[test]
fn graphir_export_renders_stub() {
    let module = tapa_graphir::AnyModuleDefinition::new_stub("fifo_stub".into(), Vec::new());
    let verilog = tapa_graphir_export::verilog::render_module(&module);
    assert!(verilog.contains("excluded"), "stub should be exclusion comment");
}

// ---------------------------------------------------------------------------
// RTL-based floorplan: FIFO width derivation from Verilog modules
// ---------------------------------------------------------------------------

#[test]
fn floorplan_rtl_fifo_width_derivation() {
    let program: tapa_topology::program::Program = serde_json::from_str(r#"{
        "top": "top_task",
        "target": "xilinx-hls",
        "tasks": {
            "top_task": {
                "level": "upper", "code": "", "target": "xilinx-hls",
                "ports": [
                    {"cat": "istream", "name": "input_data", "type": "float", "width": 32}
                ],
                "tasks": {
                    "producer": [{"args": {"out": {"arg": "fifo_0", "cat": "ostream"}}}],
                    "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}}}]
                },
                "fifos": {
                    "fifo_0": {"depth": 16, "consumed_by": ["consumer", 0], "produced_by": ["producer", 0]}
                }
            },
            "producer": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [{"cat": "ostream", "name": "out", "type": "float", "width": 32}],
                "tasks": {}, "fifos": {}
            },
            "consumer": {
                "level": "lower", "code": "", "target": "xilinx-hls",
                "ports": [{"cat": "istream", "name": "in_data", "type": "float", "width": 32}],
                "tasks": {}, "fifos": {}
            }
        }
    }"#).unwrap();

    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(program);

    // Attach RTL module for producer with 64-bit output port
    let producer_rtl = "\
module producer (
  input wire ap_clk,
  input wire ap_rst_n,
  input wire ap_start,
  output wire ap_done,
  output wire ap_idle,
  output wire ap_ready,
  output wire [63:0] out_din,
  input wire out_full_n,
  output wire out_write
);
endmodule";
    state.attach_module("producer", tapa_rtl::VerilogModule::parse(producer_rtl).unwrap()).unwrap();

    // Derive FIFO widths from RTL — should get 64 from producer's out_din port
    let widths = tapa_floorplan::gen_abgraph::collect_fifo_width_from_rtl(&state);
    assert_eq!(
        widths.get("fifo_0").copied(),
        Some(64),
        "FIFO width should be 64 from RTL port [63:0], got: {widths:?}"
    );

    // Generate full graph from RTL
    let preassignments = BTreeMap::new();
    let graph = tapa_floorplan::gen_abgraph::get_top_level_ab_graph_from_rtl(
        &state,
        &preassignments,
        "top_task_fsm",
    )
    .unwrap();

    // Verify FIFO edge has width 64 from RTL
    let fifo_edge = graph.es.iter().find(|e| {
        e.source_vertex.contains("producer") && e.target_vertex.contains("consumer")
    });
    assert!(fifo_edge.is_some(), "should have producer->consumer edge");
    assert_eq!(fifo_edge.unwrap().width, 64, "FIFO edge should use RTL-derived width");
}

// ---------------------------------------------------------------------------
// Semantic equivalence: exported Verilog reparsed by tapa-rtl
// ---------------------------------------------------------------------------

#[test]
fn exported_grouped_verilog_reparsable() {
    // Build a grouped module, export it, then reparse with tapa-rtl
    let module = tapa_graphir::AnyModuleDefinition::new_grouped(
        "test_reparse".into(),
        vec![
            tapa_graphir::ModulePort {
                name: "ap_clk".into(),
                hierarchical_name: tapa_graphir::HierarchicalName::none(),
                port_type: "input wire".into(),
                range: None,
                extra: BTreeMap::default(),
            },
            tapa_graphir::ModulePort {
                name: "data_out".into(),
                hierarchical_name: tapa_graphir::HierarchicalName::none(),
                port_type: "output wire".into(),
                range: Some(tapa_graphir::Range {
                    left: tapa_graphir::Expression::new_lit("31"),
                    right: tapa_graphir::Expression::new_lit("0"),
                }),
                extra: BTreeMap::default(),
            },
        ],
        Vec::new(),
        Vec::new(),
    );

    let verilog = tapa_graphir_export::verilog::render_module(&module);

    // Reparse with tapa-rtl
    let reparsed = tapa_rtl::VerilogModule::parse(&verilog)
        .expect("exported Verilog should be reparsable by tapa-rtl");

    // Semantic equivalence: same module name and port structure
    assert_eq!(reparsed.name, "test_reparse");
    assert_eq!(reparsed.ports.len(), 2, "should have 2 ports");
    let port_names: Vec<&str> = reparsed.ports.iter().map(|p| p.name.as_str()).collect();
    assert!(port_names.contains(&"ap_clk"), "should have ap_clk");
    assert!(port_names.contains(&"data_out"), "should have data_out");
}
