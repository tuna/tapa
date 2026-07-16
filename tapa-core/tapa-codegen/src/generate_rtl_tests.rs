//! Tests for the `generate_rtl` orchestration in `lib.rs`.

use super::*;
use crate::rtl_state::TopologyWithRtl;
use tapa_ir::Design;
use tapa_rtl::VerilogModule;

/// Helper: build a minimal `Design` from a JSON value.
fn program_from_json(json: serde_json::Value) -> Design {
    design_from_fixture_json(json)
}

/// Helper: parse a minimal Verilog module source.
fn parse_module(src: &str) -> VerilogModule {
    VerilogModule::parse(src).expect("valid Verilog")
}

// ------------------------------------------------------------------
// 1. Simple design: one upper task + one lower child
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_simple_design() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "child": [{"args": {}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);

    // Attach Verilog modules for both tasks
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    // generated_files should contain the parent .v and an FSM .v
    assert!(
        state.generated_files.contains_key("top.v"),
        "should emit top.v, got keys: {:?}",
        state.generated_files.keys().collect::<Vec<_>>()
    );
    assert!(
        state.generated_files.contains_key("top_fsm.v"),
        "should emit top_fsm.v, got keys: {:?}",
        state.generated_files.keys().collect::<Vec<_>>()
    );

    // The emitted parent module should contain the child instance
    let parent_v = &state.generated_files["top.v"];
    assert!(
        parent_v.contains("child child_0"),
        "parent should instantiate child as child_0, got:\n{parent_v}"
    );

    // The FSM module should contain __tapa_state and pipeline signals
    let fsm_v = &state.generated_files["top_fsm.v"];
    assert!(
        fsm_v.contains("__tapa_state"),
        "FSM should contain __tapa_state, got:\n{fsm_v}"
    );
    assert!(
        fsm_v.contains("__tapa_start_q"),
        "FSM should contain __tapa_start_q pipeline signal, got:\n{fsm_v}"
    );
    assert!(
        fsm_v.contains("__tapa_done_q"),
        "FSM should contain __tapa_done_q pipeline signal, got:\n{fsm_v}"
    );
}

#[test]
fn test_generate_rtl_autorun_fsm_start_is_reg_output() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "child": [{"step": -1, "args": {}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let parent_v = &state.generated_files["top.v"];
    let fsm_v = &state.generated_files["top_fsm.v"];
    assert!(
        parent_v.contains("wire child_0__ap_start;"),
        "parent-side autorun start is driven by the FSM instance and must be a net:\n{parent_v}"
    );
    assert!(
        !parent_v.contains("reg child_0__ap_start;"),
        "parent-side autorun start should not be a reg:\n{parent_v}"
    );
    assert!(
        fsm_v.contains("output reg child_0__ap_start"),
        "autorun start is assigned procedurally and must be an output reg:\n{fsm_v}"
    );
    assert!(
        !fsm_v.contains("\nreg child_0__ap_start;"),
        "reg port should not be redeclared:\n{fsm_v}"
    );
}

#[test]
fn test_generate_rtl_fsm_uses_explicit_instance_names() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "child": [{"name": "child_7", "step": -1, "args": {}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let parent_v = &state.generated_files["top.v"];
    let fsm_v = &state.generated_files["top_fsm.v"];
    assert!(
        parent_v.contains("child child_7"),
        "parent instance should preserve explicit instance name:\n{parent_v}"
    );
    assert!(
        fsm_v.contains("output reg child_7__ap_start"),
        "FSM ports must use the explicit child instance name:\n{fsm_v}"
    );
    assert!(
        !fsm_v.contains("child_0__ap_start"),
        "FSM must not use local index names when an explicit instance name exists:\n{fsm_v}"
    );
}

#[test]
fn test_generate_rtl_sanitizes_explicit_instance_names() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "child": [{"name": "Module1Func#1", "args": {}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n  input wire ap_clk,\n  input wire ap_rst_n,\n  input wire ap_start,\n  output wire ap_done,\n  output wire ap_idle,\n  output wire ap_ready\n);\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let parent_v = &state.generated_files["top.v"];
    let fsm_v = &state.generated_files["top_fsm.v"];
    assert!(
        parent_v.contains("child Module1Func_1"),
        "parent instance name must be a Verilog identifier:\n{parent_v}"
    );
    assert!(
        fsm_v.contains("reg [1:0] Module1Func_1__state;"),
        "FSM state signal must be a Verilog identifier:\n{fsm_v}"
    );
    assert!(
        !parent_v.contains("Module1Func#1") && !fsm_v.contains("Module1Func#1"),
        "generated RTL must not contain unsanitized frontend instance labels"
    );
}

#[test]
fn test_generate_rtl_child_scalar_pipeline_preserves_width() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "child": [{"args": {"pe_id": {"arg": "1", "cat": "scalar"}}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "scalar", "name": "pe_id", "type": "uint32_t", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [31:0] pe_id\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    let fsm_v = &state.generated_files["top_fsm.v"];
    assert!(
        top_v.contains("wire [31:0] child_0__pe_id;"),
        "parent scalar pipeline wire should match child port width, got:\n{top_v}"
    );
    assert!(
        fsm_v.contains("input wire [31:0] child_0__pe_id_in"),
        "FSM scalar input should match child port width, got:\n{fsm_v}"
    );
    assert!(
        fsm_v.contains("output wire [31:0] child_0__pe_id"),
        "FSM scalar output should match child port width, got:\n{fsm_v}"
    );
    assert!(
        fsm_v.contains("reg [31:0] child_0__pe_id_reg;"),
        "FSM scalar pipeline register should match child port width, got:\n{fsm_v}"
    );
}

#[test]
fn test_generate_rtl_upper_output_regs_become_nets() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "ostream", "name": "out_q", "type": "int", "width": 32}
                ],
                "tasks": {
                    "child": [{"args": {"out_q": {"arg": "out_q", "cat": "ostream"}}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "ostream", "name": "out_q", "type": "int", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [31:0] out_q_din,\n\
                 input wire out_q_full_n,\n\
                 output reg out_q_write\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [31:0] out_q_din,\n\
                 input wire out_q_full_n,\n\
                 output wire out_q_write\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains("output wire out_q_write"),
        "upper output driven by child instance should be a net, got:\n{top_v}"
    );
    assert!(
        !top_v.contains("output reg out_q_write"),
        "stale HLS output reg should not remain, got:\n{top_v}"
    );
}

// ------------------------------------------------------------------
// 2. Ignored task: custom RTL port shell
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_template_task() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "shell": {
                "readable_name": "shell",
                "level": "lower",
                "code": "",
                "synth": "ignore",
                "ports": [
                    {"cat": "scalar", "name": "n", "type": "int", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "shell": [{"args": {"n": {"arg": "1", "cat": "scalar"}}}]
                },
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire ap_start,\n\
                 output wire ap_done,\n\
                 output wire ap_idle,\n\
                 output wire ap_ready\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    // The ignored task is emitted both as a package placeholder and as an
    // author-facing template. It must not create an FSM implementation.
    assert!(
        state.generated_files.contains_key("shell.v"),
        "should emit a shell placeholder, got keys: {:?}",
        state.generated_files.keys().collect::<Vec<_>>(),
    );
    let template_v = &state.template_files["shell.v"];
    assert!(
        template_v.contains("module shell"),
        "template should contain module declaration, got:\n{template_v}"
    );
    assert!(
        template_v.contains("endmodule"),
        "template should end with endmodule, got:\n{template_v}"
    );

    // NO FSM module should be generated for a template task
    assert!(
        !state.generated_files.contains_key("shell_fsm.v"),
        "template task should not have an FSM module, got keys: {:?}",
        state.generated_files.keys().collect::<Vec<_>>()
    );
    assert!(
        !state.fsm_modules.contains_key("shell"),
        "template task should not have fsm_modules entry"
    );
}

// ------------------------------------------------------------------
// 3. Top task removes peek ports from istream
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_top_task_removes_peek_ports() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "data_in", "type": "float", "width": 32}
                ],
                "tasks": {
                    "reader": [{"args": {"input": {"arg": "data_in", "cat": "istream"}}}]
                },
                "fifos": {}
            },
            "reader": {
                "readable_name": "reader",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "input", "type": "float", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);

    // The top module has istream_peek_* ports that should be removed
    state
        .attach_module(
            "top",
            parse_module(
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [31:0] data_in_dout,\n\
                 input wire data_in_empty_n,\n\
                 output wire data_in_read,\n\
                 input wire [31:0] data_in_peek_dout,\n\
                 input wire data_in_peek_empty_n,\n\
                 output wire data_in_peek_read\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "reader",
            parse_module(
                "module reader(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];

    // Peek ports should be removed from the emitted module declaration
    let decl_section = top_v.split(");").next().unwrap_or("");
    assert!(
        !decl_section.contains("data_in_peek_dout"),
        "peek dout port should be removed from declaration, got:\n{decl_section}"
    );
    assert!(
        !decl_section.contains("data_in_peek_empty_n"),
        "peek empty_n port should be removed from declaration, got:\n{decl_section}"
    );
    assert!(
        !decl_section.contains("data_in_peek_read"),
        "peek read port should be removed from declaration, got:\n{decl_section}"
    );

    // Regular istream ports should still be present
    assert!(
        decl_section.contains("data_in_dout"),
        "regular data_in_dout should remain, got:\n{decl_section}"
    );
}

#[test]
fn test_generate_rtl_external_istream_aliases_hls_s_ports() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "data_stream", "type": "uint64_t", "width": 64}
                ],
                "tasks": {
                    "consumer": [{"args": {"data_stream": {"arg": "data_stream", "cat": "istream"}}}]
                },
                "fifos": {
                    "data_stream": {"consumed_by": ["consumer", 0]}
                }
            },
            "consumer": {
                "readable_name": "consumer",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "data_stream", "type": "uint64_t", "width": 64}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [64:0] data_stream_s_dout,\n\
                 input wire data_stream_s_empty_n,\n\
                 output wire data_stream_s_read\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "consumer",
            parse_module(
                "module consumer(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [64:0] data_stream_s_dout,\n\
                 input wire data_stream_s_empty_n,\n\
                 output wire data_stream_s_read\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();
    let top_v = &state.generated_files["top.v"];

    assert!(
        top_v.contains("wire [64:0] data_stream_dout;"),
        "canonical child-facing data wire should be declared:\n{top_v}"
    );
    assert!(
        top_v.contains("assign data_stream_dout = data_stream_s_dout;"),
        "external HLS input port should drive canonical child-facing wire:\n{top_v}"
    );
    assert!(
        top_v.contains("assign data_stream_s_read = data_stream_read;"),
        "canonical child read should drive external HLS read port:\n{top_v}"
    );
    assert!(
        top_v.contains(".data_stream_s_dout(data_stream_dout)"),
        "child HLS stream port should connect through the canonical alias:\n{top_v}"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_vitis_top_streams_use_axis_adapters() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-vitis",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "a", "type": "uint48_t", "width": 48},
                    {"cat": "ostream", "name": "c", "type": "uint64_t", "width": 64}
                ],
                "tasks": {
                    "worker": [{"args": {
                        "a": {"arg": "a", "cat": "istream"},
                        "c": {"arg": "c", "cat": "ostream"}
                    }}]
                },
                "fifos": {
                    "a": {"consumed_by": ["worker", 0]},
                    "c": {"produced_by": ["worker", 0]}
                }
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "a", "type": "uint48_t", "width": 48},
                    {"cat": "ostream", "name": "c", "type": "uint64_t", "width": 64}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [47:0] a_TDATA,\n\
                 input wire a_TVALID,\n\
                 output wire a_TREADY,\n\
                 input wire [0:0] a_TLAST,\n\
                 output wire [63:0] c_TDATA,\n\
                 output wire c_TVALID,\n\
                 input wire c_TREADY,\n\
                 output wire [0:0] c_TLAST,\n\
                 output wire [7:0] c_TKEEP\n\
                 );\n\
                 reg ap_done;\n\
                 reg ap_idle;\n\
                 reg ap_ready;\n\
                 endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module(
                "module worker(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [48:0] a_s_dout,\n\
                 input wire a_s_empty_n,\n\
                 output wire a_s_read,\n\
                 output wire [64:0] c_s_din,\n\
                 input wire c_s_full_n,\n\
                 output wire c_s_write\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();
    let top_v = &state.generated_files["top.v"];

    assert!(
        top_v.contains("wire [48:0] a_dout;"),
        "input AXIS adapter should drive canonical stream data:\n{top_v}"
    );
    assert!(
        top_v.contains("axis_to_stream_adapter #(")
            && top_v.contains(".DATA_WIDTH(48)")
            && top_v.contains(".s_axis_tdata(a_TDATA)")
            && top_v.contains(".m_stream_dout(a_dout)"),
        "input AXIS adapter should be instantiated with compatible ports:\n{top_v}"
    );
    assert!(
        top_v.contains("stream_to_axis_adapter #(")
            && top_v.contains(".DATA_WIDTH(64)")
            && top_v.contains(".s_stream_din(c_din)")
            && top_v.contains(".m_axis_tlast(c_TLAST)"),
        "output AXIS adapter should be instantiated with compatible ports:\n{top_v}"
    );
    assert!(
        top_v.contains("assign c_TKEEP = 8'b11111111;"),
        "output AXIS TKEEP should be tied high:\n{top_v}"
    );
    assert!(
        top_v.contains("wire ap_done;")
            && top_v.contains("wire ap_idle;")
            && top_v.contains("wire ap_ready;"),
        "generated submodule outputs should drive nets, not regs:\n{top_v}"
    );
}

// ------------------------------------------------------------------
// 4. Upper task with a FIFO between producer and consumer children
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_with_fifo() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "producer": [{"args": {"out_data": {"arg": "fifo_0", "cat": "ostream"}}}],
                    "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}}}]
                },
                "fifos": {
                    "fifo_0": {
                        "depth": 16,
                        "produced_by": ["producer", 0],
                        "consumed_by": ["consumer", 0]
                    }
                }
            },
            "producer": {
                "readable_name": "producer",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "ostream", "name": "out_data", "type": "float", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "consumer": {
                "readable_name": "consumer",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "in_data", "type": "float", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    // Producer with a _din port so width resolution finds 32 bits
    state
        .attach_module(
            "producer",
            parse_module(
                "module producer(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [31:0] out_data_din,\n\
                 output wire out_data_write,\n\
                 input wire out_data_full_n\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "consumer",
            parse_module(
                "module consumer(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [31:0] in_data_dout,\n\
                 input wire in_data_empty_n,\n\
                 output wire in_data_read\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];

    // Should contain a FIFO instance (parameterized: "fifo #(...) fifo_0_fifo")
    assert!(
        top_v.contains("fifo_0_fifo"),
        "parent should contain FIFO instance, got:\n{top_v}"
    );

    // Should contain wire declarations for the FIFO
    assert!(
        top_v.contains("fifo_0_dout") || top_v.contains("fifo_0_din"),
        "parent should contain FIFO wire declarations, got:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_does_not_reemit_lower_modules() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "child": [{"args": {}}]
                },
                "fifos": {}
            },
            "child": {
                "readable_name": "child",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "child",
            parse_module(
                "module child(\n  input wire ap_clk,\n  output wire ap_done\n);\n\
                 reg ap_done;\n\
                 always @(*) begin ap_done = 1'b1; end\n\
                 endmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    assert!(
        !state.generated_files.contains_key("child.v"),
        "lower HLS modules are copied from the original files; re-emitting \
         them drops legal port-reg redeclarations"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_fifo_width_uses_bound_producer_port() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [],
                "tasks": {
                    "producer": [{
                        "args": {
                            "small": {"arg": "small_fifo", "cat": "ostream"},
                            "wide": {"arg": "wide_fifo", "cat": "ostream"}
                        }
                    }],
                    "consumer": [{
                        "args": {
                            "small_in": {"arg": "small_fifo", "cat": "istream"},
                            "wide_in": {"arg": "wide_fifo", "cat": "istream"}
                        }
                    }]
                },
                "fifos": {
                    "small_fifo": {
                        "depth": 2,
                        "produced_by": ["producer", 0],
                        "consumed_by": ["consumer", 0]
                    },
                    "wide_fifo": {
                        "depth": 2,
                        "produced_by": ["producer", 0],
                        "consumed_by": ["consumer", 0]
                    }
                }
            },
            "producer": {
                "readable_name": "producer",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "ostream", "name": "small", "type": "uint8_t", "width": 8},
                    {"cat": "ostream", "name": "wide", "type": "uint32_t", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "consumer": {
                "readable_name": "consumer",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "istream", "name": "small_in", "type": "uint8_t", "width": 8},
                    {"cat": "istream", "name": "wide_in", "type": "uint32_t", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "producer",
            parse_module(
                "module producer(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [8:0] small_s_din,\n\
                 output wire small_s_write,\n\
                 input wire small_s_full_n,\n\
                 output wire [32:0] wide_s_din,\n\
                 output wire wide_s_write,\n\
                 input wire wide_s_full_n\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "consumer",
            parse_module(
                "module consumer(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [8:0] small_in_s_dout,\n\
                 input wire small_in_s_empty_n,\n\
                 output wire small_in_s_read,\n\
                 input wire [32:0] wide_in_s_dout,\n\
                 input wire wide_in_s_empty_n,\n\
                 output wire wide_in_s_read\n\
                 );\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains("small_fifo_fifo")
            && top_v.contains(".DATA_WIDTH(9)")
            && top_v.contains("wire [8:0] small_fifo_dout;")
            && top_v.contains("wire [8:0] small_fifo_din;"),
        "small_fifo should use the producer's small port width:\n{top_v}"
    );
    assert!(
        top_v.contains("wide_fifo_fifo")
            && top_v.contains(".DATA_WIDTH(33)")
            && top_v.contains("wire [32:0] wide_fifo_dout;")
            && top_v.contains("wire [32:0] wide_fifo_din;"),
        "wide_fifo should use the producer's wide port width:\n{top_v}"
    );
}

// ------------------------------------------------------------------
// 5. Multi-thread mmap: two children sharing an mmap arg
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_multithread_mmap() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mem", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "worker": [
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}}},
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module(
                "module worker(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];

    // Crossbar instance should appear (2 threads sharing 'mem')
    assert!(
        top_v.contains("axi_crossbar"),
        "parent should contain crossbar instance, got:\n{top_v}"
    );

    // Downstream wires m_axi_mem_s0_* and m_axi_mem_s1_* should be declared
    assert!(
        top_v.contains("m_axi_mem_s0_"),
        "parent should have m_axi_mem_s0_* wires, got:\n{top_v}"
    );
    assert!(
        top_v.contains("m_axi_mem_s1_"),
        "parent should have m_axi_mem_s1_* wires, got:\n{top_v}"
    );

    // Crossbar auxiliary RTL file should be generated
    assert!(
        state
            .generated_files
            .keys()
            .any(|k| k.contains("axi_crossbar")),
        "should emit crossbar RTL file, got keys: {:?}",
        state.generated_files.keys().collect::<Vec<_>>()
    );
}

#[test]
fn test_generate_rtl_nested_shared_mmap_threads() {
    // `mid` internally shares its mmap between two leaves; `top`
    // shares `mem` between `mid` and a plain leaf. The top-level
    // crossbar must provision the aggregated per-slave thread
    // counts (leaf: 1, mid: 2), not a flat 1.
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mem", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "leaf": [{"args": {"d": {"arg": "mem", "cat": "mmap"}}}],
                    "mid": [{"args": {"data": {"arg": "mem", "cat": "mmap"}}}]
                },
                "fifos": {}
            },
            "mid": {
                "readable_name": "mid",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "leaf": [
                        {"args": {"d": {"arg": "data", "cat": "mmap"}}},
                        {"args": {"d": {"arg": "data", "cat": "mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "leaf": {
                "readable_name": "leaf",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "d", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    for (name, module) in [
        (
            "top",
            "module top(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
        ),
        (
            "mid",
            "module mid(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
        ),
        (
            "leaf",
            "module leaf(\n  input wire ap_clk,\n  input wire ap_rst_n\n);\nendmodule",
        ),
    ] {
        state.attach_module(name, parse_module(module)).unwrap();
    }

    generate_rtl(&mut state).unwrap();

    // mid's own crossbar arbitrates two leaves, 1 thread each.
    let mid_v = &state.generated_files["mid.v"];
    assert!(
        mid_v.contains("S00_THREADS(1)") && mid_v.contains("S01_THREADS(1)"),
        "mid crossbar should provision 1 thread per leaf slave, got:\n{mid_v}"
    );

    // top's crossbar: slave 0 = leaf (1 thread), slave 1 = mid
    // (2 aggregated threads). Task iteration is alphabetical.
    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains("S00_THREADS(1)"),
        "leaf slave should provision 1 thread, got:\n{top_v}"
    );
    assert!(
        top_v.contains("S01_THREADS(2)"),
        "mid slave should provision its aggregated 2 threads, got:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_single_child_mmap_preserves_child_id_width() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "mid": {
                "readable_name": "mid",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                },
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "mid",
            parse_module(
                "module mid(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [1:0] m_axi_data_ARID,\n\
                 output wire [1:0] m_axi_data_AWID,\n\
                 input wire [1:0] m_axi_data_BID,\n\
                 input wire [1:0] m_axi_data_RID\n\
                 ); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "top",
            parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains("output wire [1:0] m_axi_elems_ARID"),
        "top mmap ID ports must preserve a wider child AXI ID even without a parent crossbar:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_data_ARID(m_axi_elems_ARID)"),
        "child should bind directly to the widened parent ID port:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_parent_crossbar_zero_extends_narrow_child_ids() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "leaf": {
                "readable_name": "leaf", "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "mid": {
                "readable_name": "mid", "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "top": {
                "readable_name": "top", "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "leaf": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}],
                    "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
                },
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "leaf",
            parse_module(
                "module leaf(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire m_axi_mmap_ARID,\n\
                 output wire m_axi_mmap_AWID,\n\
                 input wire m_axi_mmap_BID,\n\
                 input wire m_axi_mmap_RID\n\
                 ); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "mid",
            parse_module(
                "module mid(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [1:0] m_axi_data_ARID,\n\
                 output wire [1:0] m_axi_data_AWID,\n\
                 input wire [1:0] m_axi_data_BID,\n\
                 input wire [1:0] m_axi_data_RID\n\
                 ); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "top",
            parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains("wire [1:0] m_axi_elems_s0_ARID"),
        "parent crossbar slave wires should use the widest child ID:\n{top_v}"
    );
    assert!(
        top_v.contains("assign m_axi_elems_s0_ARID[1:1] = 1'd0"),
        "narrow child read IDs should be zero-extended into the parent crossbar:\n{top_v}"
    );
    assert!(
        top_v.contains("assign m_axi_elems_s0_AWID[1:1] = 1'd0"),
        "narrow child write IDs should be zero-extended into the parent crossbar:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_mmap_ARID(m_axi_elems_s0_ARID[0:0])"),
        "narrow child read ID ports should connect only to the low crossbar ID bit:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_mmap_BID(m_axi_elems_s0_BID[0:0])"),
        "narrow child response ID ports should consume only the low crossbar ID bit:\n{top_v}"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_parent_crossbar_slices_generated_narrow_upper_child_ids() {
    let prog = program_from_json(serde_json::json!({
        "top": "VecTop",
        "target": "xilinx-hls",
        "tasks": {
            "Awide": {
                "readable_name": "Awide",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "Leaf": {
                "readable_name": "Leaf",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            },
            "Store": {
                "readable_name": "Store",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "Leaf": [{"args": {"mmap": {"arg": "mmap", "cat": "mmap"}}}]
                },
                "fifos": {}
            },
            "VecTop": {
                "readable_name": "VecTop",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "elems", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "Awide": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}],
                    "Store": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}]
                },
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "Awide",
            parse_module(
                "module Awide(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [1:0] m_axi_data_ARID,\n\
                 output wire [1:0] m_axi_data_AWID,\n\
                 input wire [1:0] m_axi_data_BID,\n\
                 input wire [1:0] m_axi_data_RID\n\
                 ); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "Leaf",
            parse_module(
                "module Leaf(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire m_axi_mmap_ARID,\n\
                 output wire m_axi_mmap_AWID,\n\
                 input wire m_axi_mmap_BID,\n\
                 input wire m_axi_mmap_RID\n\
                 ); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "Store",
            parse_module("module Store(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "VecTop",
            parse_module("module VecTop(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["VecTop.v"];
    assert!(
        top_v.contains("wire [1:0] m_axi_elems_s1_ARID"),
        "second crossbar slave should inherit the widest child ID width:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_data_ARID(m_axi_elems_s0_ARID)"),
        "wide sibling child read ID port should keep the full crossbar slave ID:\n{top_v}"
    );
    assert!(
        !top_v.contains("assign m_axi_elems_s0_ARID[1:1]"),
        "wide sibling child IDs should not be zero-extended as if they were narrow:\n{top_v}"
    );
    assert!(
        top_v.contains("assign m_axi_elems_s1_ARID[1:1] = 1'd0"),
        "generated narrow upper child read IDs should be zero-extended:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_mmap_ARID(m_axi_elems_s1_ARID[0:0])"),
        "generated narrow upper child read ID port should connect only to the low bit:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_mmap_BID(m_axi_elems_s1_BID[0:0])"),
        "generated narrow upper child response ID port should consume only the low bit:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_hmap_uses_parent_channels() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {
                        "cat": "mmap",
                        "name": "mem",
                        "type": "float*",
                        "width": 32,
                        "chan_count": 2,
                        "chan_size": 1024
                    }
                ],
                "tasks": {
                    "worker": [
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}}},
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top(input wire ap_clk, input wire ap_rst_n, input wire [63:0] mem_0_offset, input wire [63:0] mem_1_offset); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(top_v.contains("m_axi_mem_0_ARADDR"), "got:\n{top_v}");
    assert!(top_v.contains("m_axi_mem_1_ARADDR"), "got:\n{top_v}");
    assert!(
        top_v.contains(".m_axi_data_ARADDR(m_axi_mem_s0_ARADDR)"),
        "got:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_data_ARADDR(m_axi_mem_s1_ARADDR)"),
        "got:\n{top_v}"
    );
    assert!(top_v.contains("axi_crossbar__mem"), "got:\n{top_v}");
    assert!(top_v.contains("m_axi_mem_0_ARADDR_raw"), "got:\n{top_v}");
    assert!(
        top_v.contains("assign m_axi_mem_1_ARADDR = (mem_1_offset + m_axi_mem_1_ARADDR_raw[11:0])"),
        "got:\n{top_v}"
    );
    assert!(
        top_v.contains(".worker_0__data_offset_in(64'd0)"),
        "got:\n{top_v}"
    );
    assert!(
        top_v.contains(".worker_1__data_offset_in(64'd0)"),
        "got:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_single_channel_hmap_keeps_indexed_channel() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {
                        "cat": "mmap",
                        "name": "mem",
                        "type": "float*",
                        "width": 32,
                        "chan_count": 1,
                        "chan_size": 1024
                    }
                ],
                "tasks": {
                    "worker": [
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top(input wire ap_clk, input wire ap_rst_n, input wire [63:0] mem_0_offset); endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(top_v.contains("m_axi_mem_0_ARADDR"), "got:\n{top_v}");
    assert!(top_v.contains("m_axi_mem_0_ARADDR_raw"), "got:\n{top_v}");
    assert!(
        top_v.contains("assign m_axi_mem_0_ARADDR = (mem_0_offset + m_axi_mem_0_ARADDR_raw[11:0])"),
        "got:\n{top_v}"
    );
    assert!(
        top_v.contains(".worker_0__data_offset_in(64'd0)"),
        "got:\n{top_v}"
    );
    assert!(!top_v.contains("output wire [63:0] m_axi_mem_ARADDR"));
}

#[test]
fn test_generate_rtl_sanitizes_indexed_mmap_names() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "chan[0]", "type": "float*", "width": 32}
                ],
                "tasks": {
                    "worker": [
                        {"args": {"mem": {"arg": "chan[0]", "cat": "mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mem", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(top_v.contains("m_axi_chan_0_ARADDR"), "got:\n{top_v}");
    assert!(
        top_v.contains(".worker_0__mem_offset_in(chan_0_offset)"),
        "got:\n{top_v}"
    );
    assert!(!top_v.contains("chan[0]"), "got:\n{top_v}");
}

#[test]
fn test_generate_rtl_instantiates_async_mmap_bridge() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "chan[0]", "type": "Elem*", "width": 512}
                ],
                "tasks": {
                    "copy": [
                        {"args": {"mem": {"arg": "chan[0]", "cat": "async_mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "copy": {
                "readable_name": "copy",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "async_mmap", "name": "mem", "type": "Elem*", "width": 512}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module("module top(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();
    state
        .attach_module(
            "copy",
            parse_module(
                "module copy(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [63:0] mem_read_addr_s_din,\n\
                 input wire mem_read_addr_s_full_n,\n\
                 output wire mem_read_addr_s_write,\n\
                 input wire [63:0] mem_read_addr_offset,\n\
                 input wire [512:0] mem_read_data_s_dout,\n\
                 input wire mem_read_data_s_empty_n,\n\
                 output wire mem_read_data_s_read\n\
                 ); endmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains("async_mmap #(") && top_v.contains("chan_0__m_axi"),
        "top should instantiate an async_mmap bridge:\n{top_v}"
    );
    assert!(
        top_v.contains("wire [63:0] chan_0_read_addr__din;"),
        "bridge stream wires should be declared:\n{top_v}"
    );
    assert!(
        top_v.contains(".m_axi_ARADDR(m_axi_chan_0_ARADDR)"),
        "bridge should connect to the top-level AXI port:\n{top_v}"
    );
    assert!(
        top_v.contains(".read_data_dout(chan_0_read_data__dout)"),
        "bridge should drive read data stream wire:\n{top_v}"
    );
    assert!(
        top_v.contains(".mem_read_addr_s_din(chan_0_read_addr__din)"),
        "child should consume bridge stream wires:\n{top_v}"
    );
    assert!(
        top_v.contains(".mem_read_data_s_dout({1'b0, chan_0_read_data__dout})"),
        "child read data should get a false EOT bit:\n{top_v}"
    );
    assert!(
        !top_v.contains(".m_axi_mem_ARADDR"),
        "async mmap child should not receive direct AXI ports:\n{top_v}"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_top_instantiates_control_s_axi() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "mem", "type": "float*", "width": 32},
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ],
                "tasks": {
                    "worker": [
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}, "n": {"arg": "n", "cat": "scalar"}}}
                    ]
                },
                "fifos": {}
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32},
                    {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top #(\n\
                   parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6,\n\
                   parameter C_S_AXI_CONTROL_DATA_WIDTH = 32\n\
                 ) (\n\
                   input wire ap_clk,\n\
                   input wire ap_rst_n,\n\
                   input wire s_axi_control_AWVALID,\n\
                   output wire s_axi_control_AWREADY,\n\
                   input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_AWADDR,\n\
                   input wire s_axi_control_WVALID,\n\
                   output wire s_axi_control_WREADY,\n\
                   input wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_WDATA,\n\
                   input wire [3:0] s_axi_control_WSTRB,\n\
                   input wire s_axi_control_ARVALID,\n\
                   output wire s_axi_control_ARREADY,\n\
                   input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_ARADDR,\n\
                   output wire s_axi_control_RVALID,\n\
                   input wire s_axi_control_RREADY,\n\
                   output wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_RDATA,\n\
                   output wire [1:0] s_axi_control_RRESP,\n\
                   output wire s_axi_control_BVALID,\n\
                   input wire s_axi_control_BREADY,\n\
                   output wire [1:0] s_axi_control_BRESP,\n\
                   output wire interrupt\n\
                 );\n\
                 wire ap_start;\n\
                 wire ap_done;\n\
                 wire ap_idle;\n\
                 wire ap_ready;\n\
                 wire [63:0] mem_offset;\n\
                 wire [63:0] n;\n\
                 reg [1:0] ap_CS_fsm;\n\
                 always @(posedge ap_clk) begin\n\
                   if (ap_CS_fsm == 2'd0) begin\n\
                   end else begin\n\
                   end\n\
                 end\n\
                 assign ap_done = ap_start;\n\
                 assign ap_ready = ap_start;\n\
                 assign ap_idle = 1'b1;\n\
                 endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module(
                "module worker(\n\
                   input wire ap_clk,\n\
                   input wire ap_rst_n,\n\
                   input wire ap_start,\n\
                   output wire ap_done,\n\
                   output wire ap_idle,\n\
                   output wire ap_ready,\n\
                   input wire [63:0] data_offset,\n\
                   input wire [63:0] n\n\
                 );\n\
                 endmodule",
            ),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(top_v.contains("top_control_s_axi"), "got:\n{top_v}");
    assert!(top_v.contains("control_s_axi_U"), "got:\n{top_v}");
    assert!(top_v.contains(".mem_offset(mem_offset)"), "got:\n{top_v}");
    assert!(top_v.contains(".n(n)"), "got:\n{top_v}");
    assert!(
        !top_v.contains("assign ap_done = ap_start"),
        "placeholder ap_done assign should be removed, got:\n{top_v}"
    );
    assert!(
        !top_v.contains("assign ap_ready = ap_start"),
        "placeholder ap_ready assign should be removed, got:\n{top_v}"
    );
    assert!(
        !top_v.contains("ap_CS_fsm"),
        "upper task emission should drop the original HLS FSM body, got:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_top_control_unrolls_hmap_offsets() {
    let prog = program_from_json(serde_json::json!({
        "top": "top",
        "target": "xilinx-hls",
        "tasks": {
            "top": {
                "readable_name": "top",
                "level": "upper",
                "code": "",
                "synth": "hls",
                "ports": [
                    {
                        "cat": "mmap",
                        "name": "mem",
                        "type": "float*",
                        "width": 32,
                        "chan_count": 2,
                        "chan_size": 1024
                    }
                ],
                "tasks": {
                    "worker": [
                        {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
                    ]
                },
                "fifos": {}
            },
            "worker": {
                "readable_name": "worker",
                "level": "lower",
                "code": "",
                "synth": "hls",
                "ports": [
                    {"cat": "mmap", "name": "data", "type": "float*", "width": 32}
                ],
                "tasks": {},
                "fifos": {}
            }
        }
    }));

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module(
            "top",
            parse_module(
                "module top #(\n\
                   parameter C_S_AXI_CONTROL_ADDR_WIDTH = 6,\n\
                   parameter C_S_AXI_CONTROL_DATA_WIDTH = 32\n\
                 ) (\n\
                   input wire ap_clk,\n\
                   input wire ap_rst_n,\n\
                   input wire s_axi_control_AWVALID,\n\
                   output wire s_axi_control_AWREADY,\n\
                   input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_AWADDR,\n\
                   input wire s_axi_control_WVALID,\n\
                   output wire s_axi_control_WREADY,\n\
                   input wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_WDATA,\n\
                   input wire [3:0] s_axi_control_WSTRB,\n\
                   input wire s_axi_control_ARVALID,\n\
                   output wire s_axi_control_ARREADY,\n\
                   input wire [C_S_AXI_CONTROL_ADDR_WIDTH-1:0] s_axi_control_ARADDR,\n\
                   output wire s_axi_control_RVALID,\n\
                   input wire s_axi_control_RREADY,\n\
                   output wire [C_S_AXI_CONTROL_DATA_WIDTH-1:0] s_axi_control_RDATA,\n\
                   output wire [1:0] s_axi_control_RRESP,\n\
                   output wire s_axi_control_BVALID,\n\
                   input wire s_axi_control_BREADY,\n\
                   output wire [1:0] s_axi_control_BRESP,\n\
                   output wire interrupt\n\
                 );\n\
                 wire ap_start;\n\
                 wire ap_done;\n\
                 wire ap_idle;\n\
                 wire ap_ready;\n\
                 wire [63:0] mem_0_offset;\n\
                 wire [63:0] mem_1_offset;\n\
                 endmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module("module worker(input wire ap_clk, input wire ap_rst_n); endmodule"),
        )
        .unwrap();

    generate_rtl(&mut state).unwrap();

    let top_v = &state.generated_files["top.v"];
    assert!(
        top_v.contains(".mem_0_offset(mem_0_offset)"),
        "got:\n{top_v}"
    );
    assert!(
        top_v.contains(".mem_1_offset(mem_1_offset)"),
        "got:\n{top_v}"
    );
    assert!(
        !top_v.contains(".mem_offset(mem_offset)"),
        "hmap control offsets should remain unrolled, got:\n{top_v}"
    );
}
