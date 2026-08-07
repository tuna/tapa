//! Tests for the `generate_rtl` orchestration in `lib.rs`.

mod common;

use common::run_manifest::{generate_manifest, rtl_file, run_manifest};
use common::{
    attach_basic_modules, design, handshake_module_src, narrow_axi_id_module_src, parse_module,
    plain, stream_consumer_module_src, stream_producer_module_src, task, wide_axi_id_module_src,
};
use std::process::Command;
use tapa_codegen::generate_rtl;
use tapa_codegen::rtl_state::TopologyWithRtl;

/// Assert that `rtl` contains every `contains` needle and none of the
/// `rejects`, printing the RTL once on failure.
fn check(rtl: &str, contains: &[&str], rejects: &[&str]) {
    for needle in contains {
        assert!(rtl.contains(needle), "missing '{needle}':\n{rtl}");
    }
    for needle in rejects {
        assert!(!rtl.contains(needle), "unexpected '{needle}':\n{rtl}");
    }
}

// ------------------------------------------------------------------
// 1. Simple design: one upper task + one lower child
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_simple_design() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({"child": [{"args": {}}]});
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("child", plain("child", "lower"))],
    );

    let manifest = run_manifest(prog, &["top", "child"], &[]);

    // The manifest should contain the parent .v and an FSM .v
    assert!(
        manifest.files().contains_key("rtl/top.v"),
        "should emit top.v, got keys: {:?}",
        manifest.files().keys().collect::<Vec<_>>()
    );
    assert!(
        manifest.files().contains_key("rtl/top_fsm.v"),
        "should emit top_fsm.v, got keys: {:?}",
        manifest.files().keys().collect::<Vec<_>>()
    );

    // The emitted parent module should contain the child instance
    let parent_v = rtl_file(&manifest, "top.v");
    check(parent_v, &["child child_0"], &[]);

    // The FSM module should contain __tapa_state and pipeline signals
    let fsm_v = rtl_file(&manifest, "top_fsm.v");
    check(
        fsm_v,
        &["__tapa_state", "__tapa_start_q", "__tapa_done_q"],
        &[],
    );
}

#[test]
fn test_generate_rtl_autorun_fsm_start_is_reg_output() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({"child": [{"step": -1, "args": {}}]});
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("child", plain("child", "lower"))],
    );

    let manifest = run_manifest(prog, &["top", "child"], &[]);

    let parent_v = rtl_file(&manifest, "top.v");
    let fsm_v = rtl_file(&manifest, "top_fsm.v");
    check(
        parent_v,
        &["wire child_0__ap_start;"],
        &["reg child_0__ap_start;"],
    );
    check(
        fsm_v,
        &["output reg child_0__ap_start"],
        &["\nreg child_0__ap_start;"],
    );
}

#[test]
fn test_generate_rtl_fsm_uses_explicit_instance_names() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({"child": [{"name": "child_7", "step": -1, "args": {}}]});
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("child", plain("child", "lower"))],
    );

    let manifest = run_manifest(prog, &["top", "child"], &[]);

    let parent_v = rtl_file(&manifest, "top.v");
    let fsm_v = rtl_file(&manifest, "top_fsm.v");
    check(parent_v, &["child child_7"], &[]);
    check(
        fsm_v,
        &["output reg child_7__ap_start"],
        &["child_0__ap_start"],
    );
}

#[test]
fn test_generate_rtl_sanitizes_explicit_instance_names() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({"child": [{"name": "Module1Func#1", "args": {}}]});
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("child", plain("child", "lower"))],
    );

    let manifest = run_manifest(prog, &["top"], &[("child", &handshake_module_src("child"))]);

    let parent_v = rtl_file(&manifest, "top.v");
    let fsm_v = rtl_file(&manifest, "top_fsm.v");
    check(parent_v, &["child Module1Func_1"], &[]);
    check(fsm_v, &["reg [1:0] Module1Func_1__state;"], &[]);
    check(parent_v, &[], &["Module1Func#1"]);
    check(fsm_v, &[], &["Module1Func#1"]);
}

#[test]
fn test_generate_rtl_child_scalar_pipeline_preserves_width() {
    let top = task("top", "upper", |t| {
        t["tasks"] =
            serde_json::json!({"child": [{"args": {"pe_id": {"arg": "1", "cat": "scalar"}}}]});
    });
    let child = task("child", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "scalar", "name": "pe_id", "type": "uint32_t", "width": 32}
        ]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("child", child)]);

    let manifest = run_manifest(
        prog,
        &["top"],
        &[(
            "child",
            "module child(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [31:0] pe_id\n\
                 );\nendmodule",
        )],
    );

    let top_v = rtl_file(&manifest, "top.v");
    let fsm_v = rtl_file(&manifest, "top_fsm.v");
    check(top_v, &["wire [31:0] child_0__pe_id;"], &[]);
    check(
        fsm_v,
        &[
            "input wire [31:0] child_0__pe_id_in",
            "output wire [31:0] child_0__pe_id",
            "reg [31:0] child_0__pe_id_reg;",
        ],
        &[],
    );
}

/// A scalar bound to a constant carries `{width, value}` from the frontend;
/// the RTL backend is the only place that spells it as a Verilog literal.
#[test]
fn test_generate_rtl_child_scalar_constant_becomes_a_verilog_literal() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({
            "child": [{"args": {"pe_id": {"arg": {"width": 32, "value": 7}, "cat": "scalar"}}}]
        });
    });
    let child = task("child", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "scalar", "name": "pe_id", "type": "uint32_t", "width": 32}
        ]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("child", child)]);

    let manifest = run_manifest(
        prog,
        &["top"],
        &[(
            "child",
            "module child(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [31:0] pe_id\n\
                 );\nendmodule",
        )],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(top_v, &[".child_0__pe_id_in(32'd7)"], &["pe_id_in(7)"]);
}

#[test]
fn test_generate_rtl_upper_output_regs_become_nets() {
    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "ostream", "name": "out_q", "type": "int", "width": 32}]);
        t["tasks"] =
            serde_json::json!({"child": [{"args": {"out_q": {"arg": "out_q", "cat": "ostream"}}}]});
    });
    let child = task("child", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "ostream", "name": "out_q", "type": "int", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("child", child)]);

    let manifest = run_manifest(
        prog,
        &[],
        &[
            (
                "top",
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [31:0] out_q_din,\n\
                 input wire out_q_full_n,\n\
                 output reg out_q_write\n\
                 );\nendmodule",
            ),
            (
                "child",
                "module child(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 output wire [31:0] out_q_din,\n\
                 input wire out_q_full_n,\n\
                 output wire out_q_write\n\
                 );\nendmodule",
            ),
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &["output wire out_q_write"],
        &["output reg out_q_write"],
    );
}

// ------------------------------------------------------------------
// 2. Ignored task: custom RTL port shell
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_template_task() {
    let shell = task("shell", "lower", |t| {
        t["synth"] = serde_json::json!("ignore");
        t["ports"] =
            serde_json::json!([{"cat": "scalar", "name": "n", "type": "int", "width": 32}]);
    });
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({"shell": [{"args": {"n": {"arg": "1", "cat": "scalar"}}}]});
    });
    let prog = design("top", "xilinx-hls", &[("shell", shell), ("top", top)]);

    let manifest = run_manifest(prog, &[], &[("top", &handshake_module_src("top"))]);

    // The ignored task is emitted both as a package placeholder and as an
    // author-facing template. It must not create an FSM implementation.
    assert!(
        manifest.files().contains_key("rtl/shell.v"),
        "should emit a shell placeholder, got keys: {:?}",
        manifest.files().keys().collect::<Vec<_>>(),
    );
    let template_v = &manifest.files()["template/shell.v"];
    check(template_v, &["module shell", "endmodule"], &[]);

    // NO FSM module should be generated for a template task
    assert!(
        !manifest.files().contains_key("rtl/shell_fsm.v"),
        "template task should not have an FSM module, got keys: {:?}",
        manifest.files().keys().collect::<Vec<_>>()
    );
}

// ------------------------------------------------------------------
// 3. Top task removes peek ports from istream
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_top_task_removes_peek_ports() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([{"cat": "istream", "name": "data_in", "type": "float", "width": 32}]);
        t["tasks"] = serde_json::json!({"reader": [{"args": {"input": {"arg": "data_in", "cat": "istream"}}}]});
    });
    let reader = task("reader", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "istream", "name": "input", "type": "float", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("reader", reader)]);

    // The top module has istream_peek_* ports that should be removed
    let manifest = run_manifest(
        prog,
        &["reader"],
        &[(
            "top",
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
        )],
    );

    let top_v = rtl_file(&manifest, "top.v");

    // Non-floorplanned builds emit the reset as a plain wire with no
    // synthesis attribute — the max_fanout cap is floorplan-only.
    check(top_v, &["wire ap_rst;"], &["max_fanout"]);

    // Peek ports should be removed from the emitted module declaration
    let decl_section = top_v.split(");").next().unwrap_or("");
    check(
        decl_section,
        &[],
        &[
            "data_in_peek_dout",
            "data_in_peek_empty_n",
            "data_in_peek_read",
        ],
    );

    // Regular istream ports should still be present
    check(decl_section, &["data_in_dout"], &[]);
}

#[test]
fn test_generate_rtl_external_istream_aliases_hls_s_ports() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "istream", "name": "data_stream", "type": "uint64_t", "width": 64}
        ]);
        t["tasks"] = serde_json::json!({"consumer": [{"args": {"data_stream": {"arg": "data_stream", "cat": "istream"}}}]});
        t["fifos"] = serde_json::json!({
            "data_stream": {"consumed_by": ["consumer", 0]}
        });
    });
    let consumer = task("consumer", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "istream", "name": "data_stream", "type": "uint64_t", "width": 64}
        ]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("consumer", consumer)]);

    let manifest = run_manifest(
        prog,
        &[],
        &[
            (
                "top",
                "module top(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [64:0] data_stream_s_dout,\n\
                 input wire data_stream_s_empty_n,\n\
                 output wire data_stream_s_read\n\
                 );\nendmodule",
            ),
            (
                "consumer",
                "module consumer(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire [64:0] data_stream_s_dout,\n\
                 input wire data_stream_s_empty_n,\n\
                 output wire data_stream_s_read\n\
                 );\nendmodule",
            ),
        ],
    );
    let top_v = rtl_file(&manifest, "top.v");

    check(
        top_v,
        &[
            "wire [64:0] data_stream_dout;",
            "assign data_stream_dout = data_stream_s_dout;",
            "assign data_stream_s_read = data_stream_read;",
            ".data_stream_s_dout(data_stream_dout)",
        ],
        &[],
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_vitis_top_streams_use_axis_adapters() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "istream", "name": "a", "type": "uint48_t", "width": 48},
            {"cat": "ostream", "name": "c", "type": "uint64_t", "width": 64}
        ]);
        t["tasks"] = serde_json::json!({
            "worker": [{"args": {
                "a": {"arg": "a", "cat": "istream"},
                "c": {"arg": "c", "cat": "ostream"}
            }}]
        });
        t["fifos"] = serde_json::json!({
            "a": {"consumed_by": ["worker", 0]},
            "c": {"produced_by": ["worker", 0]}
        });
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "istream", "name": "a", "type": "uint48_t", "width": 48},
            {"cat": "ostream", "name": "c", "type": "uint64_t", "width": 64}
        ]);
    });
    let prog = design("top", "xilinx-vitis", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(
        prog,
        &[],
        &[
            (
                "top",
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
            (
                "worker",
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
        ],
    );
    let top_v = rtl_file(&manifest, "top.v");

    check(
        top_v,
        &[
            "wire [48:0] a_dout;",
            "axis_to_stream_adapter #(",
            ".DATA_WIDTH(48)",
            ".s_axis_tdata(a_TDATA)",
            ".m_stream_dout(a_dout)",
            "stream_to_axis_adapter #(",
            ".DATA_WIDTH(64)",
            ".s_stream_din(c_din)",
            ".m_axis_tlast(c_TLAST)",
            "assign c_TKEEP = 8'b11111111;",
            "wire ap_done;",
            "wire ap_idle;",
            "wire ap_ready;",
        ],
        &[],
    );
}

// ------------------------------------------------------------------
// 4. Upper task with a FIFO between producer and consumer children
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_with_fifo() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({
            "producer": [{"args": {"out_data": {"arg": "fifo_0", "cat": "ostream"}}}],
            "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}}}]
        });
        t["fifos"] = serde_json::json!({
            "fifo_0": {
                "depth": 16,
                "produced_by": ["producer", 0],
                "consumed_by": ["consumer", 0]
            }
        });
    });
    let producer = task("producer", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "ostream", "name": "out_data", "type": "float", "width": 32}
        ]);
    });
    let consumer = task("consumer", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "istream", "name": "in_data", "type": "float", "width": 32}
        ]);
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("producer", producer), ("consumer", consumer)],
    );

    // Producer with a _din port so width resolution finds 32 bits
    let manifest = run_manifest(
        prog,
        &["top"],
        &[
            ("producer", &stream_producer_module_src("producer")),
            ("consumer", &stream_consumer_module_src("consumer")),
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");

    // Should contain a FIFO instance (parameterized: "fifo #(...) fifo_0_fifo")
    check(top_v, &["fifo_0_fifo"], &[]);

    // Should contain wire declarations for the FIFO
    assert!(
        top_v.contains("fifo_0_dout") || top_v.contains("fifo_0_din"),
        "parent should contain FIFO wire declarations, got:\n{top_v}"
    );
}

#[test]
fn test_generate_rtl_floorplan_crossing_becomes_head_body_tail_pipeline() {
    let top_v = generate_floorplanned_stream_pipeline(
        tapa_ir::PipelineScheme::Double,
        vec!["SLOT_X0Y0", "SLOT_X0Y1"],
    );

    check(
        &top_v,
        &["tapa_hs_pipeline", "fifo_0_fifo", ".BODY_LEVEL(2)"],
        &[],
    );
    // The original DEPTH (16) is passed; the primitive grows Tail storage.
    check(&top_v, &[".DEPTH(16)"], &[]);
}

#[test]
fn test_generate_rtl_adjacent_single_crossing_keeps_head_and_tail() {
    let top_v = generate_floorplanned_stream_pipeline(tapa_ir::PipelineScheme::Single, Vec::new());

    check(
        &top_v,
        &["tapa_hs_pipeline", ".BODY_LEVEL(0)"],
        &["relay_station"],
    );
}

fn generate_floorplanned_stream_pipeline(
    scheme: tapa_ir::PipelineScheme,
    reg_regions: Vec<&str>,
) -> String {
    use std::collections::BTreeMap;
    use tapa_ir::{FloorplanResult, PipelineRoute, RoutedChannel};

    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({
            "producer": [{"args": {"out_data": {"arg": "fifo_0", "cat": "ostream"}}}],
            "consumer": [{"args": {"in_data": {"arg": "fifo_0", "cat": "istream"}}}]
        });
        t["fifos"] = serde_json::json!({
            "fifo_0": {"depth": 16, "produced_by": ["producer", 0], "consumed_by": ["consumer", 0]}
        });
    });
    let producer = task("producer", "lower", |t| {
        t["ports"] = serde_json::json!([{"cat": "ostream", "name": "out_data", "type": "float", "width": 32}]);
    });
    let consumer = task("consumer", "lower", |t| {
        t["ports"] = serde_json::json!([{"cat": "istream", "name": "in_data", "type": "float", "width": 32}]);
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("producer", producer), ("consumer", consumer)],
    );

    let mut state = TopologyWithRtl::new(prog);
    state.floorplan = Some(FloorplanResult {
        device: "u280".to_string(),
        grid: (2, 3),
        regions: BTreeMap::new(),
        routes: vec![PipelineRoute {
            channel: RoutedChannel::Stream {
                fifo: "fifo_0".to_string(),
            },
            route: vec!["SLOT_X0Y0".to_string(), "SLOT_X0Y1".to_string()],
            scheme,
            reg_regions: reg_regions.into_iter().map(str::to_string).collect(),
        }],
        slot_usage: BTreeMap::new(),
    });
    attach_basic_modules(&mut state, &["top"]);
    state
        .attach_module(
            "producer",
            parse_module(&stream_producer_module_src("producer")),
        )
        .unwrap();
    state
        .attach_module(
            "consumer",
            parse_module(&stream_consumer_module_src("consumer")),
        )
        .unwrap();

    generate_manifest(&mut state).files()["rtl/top.v"].clone()
}

#[allow(
    clippy::too_many_lines,
    reason = "fixture includes the complete placement and typed route contract"
)]
fn distributed_control_state() -> TopologyWithRtl {
    use std::collections::BTreeMap;
    use tapa_ir::{
        global_controller_instance_name, local_controller_instance_name, ControlChannel,
        FloorplanResult, PipelineRoute, PipelineScheme, RoutedChannel,
    };

    let top = task("top", "upper", |task| {
        task["ports"] = serde_json::json!([
            {"cat": "scalar", "name": "n", "type": "uint32_t", "width": 17}
        ]);
        task["tasks"] = serde_json::json!({
            "worker": [{
                "name": "worker#0",
                "args": {"count": {"arg": "n", "cat": "scalar"}}
            }],
            "daemon": [{"name": "daemon#0", "step": -1, "args": {}}]
        });
    });
    let worker = task("worker", "lower", |task| {
        task["ports"] = serde_json::json!([
            {"cat": "scalar", "name": "count", "type": "uint32_t", "width": 17}
        ]);
    });
    let mut state = TopologyWithRtl::new(design(
        "top",
        "xilinx-hls",
        &[
            ("top", top),
            ("worker", worker),
            ("daemon", plain("daemon", "lower")),
        ],
    ));
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
                 output wire ap_ready,\n\
                 input wire [16:0] n\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "worker",
            parse_module(
                "module worker(\n\
                 input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
                 output wire ap_done, output wire ap_idle, output wire ap_ready,\n\
                 input wire [16:0] count\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module(
            "daemon",
            parse_module(
                "module daemon(input wire ap_clk, input wire ap_rst_n, input wire ap_start);\nendmodule",
            ),
        )
        .unwrap();

    let worker = "worker#0";
    let daemon = "daemon#0";
    let forward = vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()];
    let reverse = vec!["SLOT_X1Y0".to_string(), "SLOT_X0Y0".to_string()];
    state.floorplan = Some(FloorplanResult {
        device: "u280".to_string(),
        grid: (2, 1),
        regions: BTreeMap::from([
            (
                global_controller_instance_name().to_string(),
                "SLOT_X0Y0".to_string(),
            ),
            (worker.to_string(), "SLOT_X1Y0".to_string()),
            (
                local_controller_instance_name(worker),
                "SLOT_X1Y0".to_string(),
            ),
            (daemon.to_string(), "SLOT_X0Y0".to_string()),
            (
                local_controller_instance_name(daemon),
                "SLOT_X0Y0".to_string(),
            ),
        ]),
        routes: vec![
            PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: worker.to_string(),
                    channel: ControlChannel::Launch,
                },
                route: forward.clone(),
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X0Y0".to_string()],
            },
            PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: worker.to_string(),
                    channel: ControlChannel::Reset,
                },
                route: forward,
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X0Y0".to_string()],
            },
            PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: worker.to_string(),
                    channel: ControlChannel::Completion,
                },
                route: reverse,
                scheme: PipelineScheme::Double,
                reg_regions: vec!["SLOT_X1Y0".to_string(), "SLOT_X0Y0".to_string()],
            },
        ],
        slot_usage: BTreeMap::new(),
    });
    state
}

fn lint_distributed_top(state: &TopologyWithRtl) {
    if !Command::new("verilator")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        eprintln!("skipping generated distributed-top lint: `verilator` not found on PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let asset = tapa_codegen::support_assets::VerilogAssets::get("tapa_control.v")
        .expect("embedded control RTL");
    std::fs::write(root.join("tapa_control.v"), &asset.data).expect("write control RTL");
    std::fs::write(root.join("top.v"), &state.generated_files["top.v"])
        .expect("write generated top");
    for child in ["worker", "daemon"] {
        std::fs::write(
            root.join(format!("{child}.v")),
            state.module_map[child].emit(),
        )
        .expect("write child RTL");
    }

    let lint = Command::new("verilator")
        .current_dir(root)
        .args([
            "--lint-only",
            "--top-module",
            "top",
            "-Wno-UNUSEDSIGNAL",
            "-Wno-fatal",
            "tapa_control.v",
            "worker.v",
            "daemon.v",
            "top.v",
        ])
        .output()
        .expect("spawn verilator");
    assert!(
        lint.status.success(),
        "generated distributed top failed Verilator lint:\n{}\n{}",
        String::from_utf8_lossy(&lint.stdout),
        String::from_utf8_lossy(&lint.stderr),
    );
}

#[test]
fn test_generate_rtl_distributes_floorplanned_control() {
    let mut state = distributed_control_state();
    generate_rtl(&mut state).unwrap();
    let top = &state.generated_files["top.v"];

    assert!(
        !state.generated_files.contains_key("top_fsm.v"),
        "distributed control must replace the monolithic FSM module"
    );
    check(
        top,
        &[
            "__tapa_global_controller",
            "__tapa_local_controller_worker_0",
            "__tapa_local_controller_daemon_0",
            "__tapa_control_worker_0_launch",
            "__tapa_control_worker_0_reset",
            "__tapa_control_worker_0_completion",
        ],
        &[],
    );
    assert_eq!(top.matches("tapa_control_pipeline #(").count(), 3, "{top}");
    check(top, &[".WIDTH(19)", "assign __tapa_control_worker_0__launch_input = {n, __tapa_control_release, __tapa_control_start}", "assign worker_0__count = __tapa_control_worker_0__launch_output[18:2]", ".FLUSH_CYCLES(7)", "assign ap_rst = !__tapa_control_fabric_reset_n", ".fabric_reset_n(__tapa_control_fabric_reset_n)", "(* max_fanout = 256 *) wire ap_rst;", ".ap_rst_n(__tapa_control_worker_0__reset_n)", "assign __tapa_control_daemon_0__reset_n = ap_rst_n", ".AUTORUN(1)", ".launch_start(__tapa_control_daemon_0__launch_output)",], &["__tapa_control_daemon_0_completion"]);
    lint_distributed_top(&state);
}

#[test]
fn test_generate_rtl_rejects_malformed_control_without_mutation() {
    let mut cases = Vec::new();

    let mut missing = distributed_control_state();
    missing.floorplan.as_mut().unwrap().routes.remove(1);
    cases.push((missing, "missing its Reset route"));

    let mut duplicate = distributed_control_state();
    let route = duplicate.floorplan.as_ref().unwrap().routes[0].clone();
    duplicate.floorplan.as_mut().unwrap().routes.push(route);
    cases.push((duplicate, "more than one Launch route"));

    let mut reversed = distributed_control_state();
    reversed.floorplan.as_mut().unwrap().routes[0]
        .route
        .reverse();
    cases.push((reversed, "inconsistent direction"));

    let mut unexpected_s_axi = distributed_control_state();
    unexpected_s_axi
        .floorplan
        .as_mut()
        .unwrap()
        .regions
        .insert("control_s_axi_U".to_string(), "SLOT_X0Y0".to_string());
    cases.push((unexpected_s_axi, "unexpected control_s_axi_U placement"));

    let mut missing_s_axi = distributed_control_state();
    missing_s_axi.module_map.insert(
        "top".to_string(),
        tapa_rtl::mutation::MutableModule::from_parsed(parse_module(
            "module top(\n\
             input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
             output wire ap_done, output wire ap_idle, output wire ap_ready,\n\
             input wire s_axi_control_AWVALID, input wire [16:0] n\n\
             );\nendmodule",
        )),
    );
    cases.push((missing_s_axi, "missing placement 'control_s_axi_U'"));

    for (mut state, expected) in cases {
        let before = state.module_map["top"].emit();
        let error = generate_rtl(&mut state).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected '{expected}', got {error}"
        );
        assert_eq!(state.module_map["top"].emit(), before);
        assert!(state.fsm_modules.is_empty());
        assert!(state.generated_files.is_empty());
    }
}

#[test]
fn test_generate_rtl_rejects_control_scalar_width_mismatch_without_mutation() {
    let mut state = distributed_control_state();
    state.module_map.insert(
        "worker".to_string(),
        tapa_rtl::mutation::MutableModule::from_parsed(parse_module(
            "module worker(\n\
             input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
             output wire ap_done, output wire ap_idle, output wire ap_ready,\n\
             input wire [15:0] count\n\
             );\nendmodule",
        )),
    );
    let before = state.module_map["top"].emit();
    let error = generate_rtl(&mut state).unwrap_err();
    assert!(
        error.to_string().contains(
            "scalar width mismatch for 'worker.count': topology is 17 bits but RTL is 16 bits"
        ),
        "got {error}"
    );
    assert_eq!(state.module_map["top"].emit(), before);
    assert!(state.generated_files.is_empty());
}

#[test]
fn test_generate_rtl_control_launch_packs_direct_mmap_offset() {
    use tapa_ir::{
        global_controller_instance_name, local_controller_instance_name, ControlChannel,
        PipelineRoute, PipelineScheme, RoutedChannel,
    };

    let mut state = direct_axi_state(Some(direct_axi_routes()));
    let floorplan = state.floorplan.as_mut().unwrap();
    floorplan.regions.extend([
        (
            global_controller_instance_name().to_string(),
            "SLOT_X1Y0".to_string(),
        ),
        (
            local_controller_instance_name("reader#0"),
            "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
        ),
    ]);
    for channel in [ControlChannel::Launch, ControlChannel::Reset] {
        floorplan.routes.push(PipelineRoute {
            channel: RoutedChannel::Control {
                instance: "reader#0".to_string(),
                channel,
            },
            route: vec!["SLOT_X1Y0".to_string(), "SLOT_X0Y0".to_string()],
            scheme: PipelineScheme::Single,
            reg_regions: Vec::new(),
        });
    }
    floorplan.routes.push(PipelineRoute {
        channel: RoutedChannel::Control {
            instance: "reader#0".to_string(),
            channel: ControlChannel::Completion,
        },
        route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
        scheme: PipelineScheme::Single,
        reg_regions: Vec::new(),
    });

    let manifest = generate_manifest(&mut state);
    let top = rtl_file(&manifest, "top.v");
    check(top, &[".WIDTH(66)", "assign __tapa_control_reader_0__launch_input = {mem_offset, __tapa_control_release, __tapa_control_start}", "assign reader_0__data_offset = __tapa_control_reader_0__launch_output[65:2]", ".FLUSH_CYCLES(8)"], &[]);
}

fn direct_axi_endpoint() -> tapa_ir::AxiEndpoint {
    tapa_ir::AxiEndpoint {
        instance: "reader#0".to_string(),
        port: "data".to_string(),
        top_port: "mem".to_string(),
    }
}

fn direct_axi_routes() -> Vec<tapa_ir::PipelineRoute> {
    use tapa_ir::{
        AxiChannel, MemoryBank, MemoryKind, PipelineRoute, PipelineScheme, RoutedChannel,
    };

    let endpoint = direct_axi_endpoint();
    let bank = MemoryBank {
        kind: MemoryKind::Hbm,
        index: 0,
    };
    [
        AxiChannel::ReadAddress,
        AxiChannel::ReadData,
        AxiChannel::WriteAddress,
        AxiChannel::WriteData,
        AxiChannel::WriteResponse,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, channel)| {
        let outgoing = matches!(
            channel,
            AxiChannel::ReadAddress | AxiChannel::WriteAddress | AxiChannel::WriteData
        );
        PipelineRoute {
            channel: RoutedChannel::Axi {
                endpoint: endpoint.clone(),
                bank,
                channel,
            },
            route: if outgoing {
                vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()]
            } else {
                vec!["SLOT_X1Y0".to_string(), "SLOT_X0Y0".to_string()]
            },
            scheme: PipelineScheme::Double,
            reg_regions: (0..index).map(|_| "SLOT_X0Y0".to_string()).collect(),
        }
    })
    .collect()
}

fn compact_m_axi_child_module_src() -> String {
    use tapa_protocol::{
        axi_subport_from_suffix, axi_subport_width, m_axi_port_direction, PortDir, AXI_ADDR_WIDTH,
        M_AXI_SUFFIXES_COMPACT,
    };

    let mut ports = vec![
        "input wire ap_clk".to_string(),
        "input wire ap_rst_n".to_string(),
        "input wire ap_start".to_string(),
        "output wire ap_done".to_string(),
        "output wire ap_idle".to_string(),
        "output wire ap_ready".to_string(),
        "input wire [63:0] data_offset".to_string(),
    ];
    for suffix in M_AXI_SUFFIXES_COMPACT {
        let output = matches!(m_axi_port_direction(suffix), Some(PortDir::Output));
        let direction = if output { "output" } else { "input" };
        let width = axi_subport_width(axi_subport_from_suffix(suffix), 32, AXI_ADDR_WIDTH, 3);
        let width = if width == 1 {
            String::new()
        } else {
            format!(" [{}:0]", width - 1)
        };
        ports.push(format!("{direction} wire{width} m_axi_data{suffix}"));
    }
    format!("module reader(\n  {}\n);\nendmodule", ports.join(",\n  "))
}

fn direct_axi_state(routes: Option<Vec<tapa_ir::PipelineRoute>>) -> TopologyWithRtl {
    use std::collections::BTreeMap;
    use tapa_ir::FloorplanResult;

    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mem", "type": "int*", "width": 32}]);
        t["tasks"] = serde_json::json!({
            "reader": [{
                "name": "reader#0",
                "args": {"data": {"arg": "mem", "cat": "mmap"}}
            }]
        });
    });
    let reader = task("reader", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "int*", "width": 32}]);
    });
    let mut state = TopologyWithRtl::new(design(
        "top",
        "xilinx-hls",
        &[("top", top), ("reader", reader)],
    ));
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
                 output wire ap_ready,\n\
                 input wire [63:0] mem_offset\n\
                 );\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module("reader", parse_module(&compact_m_axi_child_module_src()))
        .unwrap();
    state.floorplan = routes.map(|routes| FloorplanResult {
        device: "u280".to_string(),
        grid: (2, 1),
        regions: BTreeMap::from([(
            direct_axi_endpoint().instance,
            "SLOT_X0Y0_TO_SLOT_X0Y0".to_string(),
        )]),
        routes,
        slot_usage: BTreeMap::new(),
    });
    state
}

#[test]
fn test_generate_rtl_floorplanned_direct_axi_pipelines_all_five_channels() {
    let mut state = direct_axi_state(Some(direct_axi_routes()));
    let manifest = generate_manifest(&mut state);
    let top_v = rtl_file(&manifest, "top.v");

    assert_eq!(top_v.matches("tapa_hs_pipeline #(").count(), 5, "{top_v}");
    check(
        top_v,
        &[
            "__tapa_axi_mem_ar",
            "__tapa_axi_mem_r",
            "__tapa_axi_mem_aw",
            "__tapa_axi_mem_w",
            "__tapa_axi_mem_b",
        ],
        &[],
    );
    for body_level in 0..5 {
        check(top_v, &[format!(".BODY_LEVEL({body_level})").as_str()], &[]);
    }
    assert_eq!(top_v.matches(".DEPTH(2)").count(), 5, "{top_v}");
    check(top_v, &[".m_axi_data_ARADDR(__tapa_axi_mem_child_ARADDR)", ".if_full_n(__tapa_axi_mem_child_ARREADY)", ".if_write(__tapa_axi_mem_child_ARVALID)", ".if_empty_n(m_axi_mem_ARVALID)", ".if_read(m_axi_mem_ARREADY)", ".if_full_n(m_axi_mem_RREADY)", ".if_write(m_axi_mem_RVALID)", ".if_empty_n(__tapa_axi_mem_child_RVALID)", ".if_read(__tapa_axi_mem_child_RREADY)", ".if_din({__tapa_axi_mem_child_ARADDR, __tapa_axi_mem_child_ARBURST, __tapa_axi_mem_child_ARID, __tapa_axi_mem_child_ARLEN, __tapa_axi_mem_child_ARSIZE})", "output wire [2:0] m_axi_mem_ARID"], &[]);
    check(
        top_v,
        &[
            "assign m_axi_mem_AWLOCK = 1'b0",
            "assign m_axi_mem_AWCACHE = 4'b0011",
            "assign m_axi_mem_AWPROT = 3'b000",
            "assign m_axi_mem_AWQOS = 4'b0000",
            "assign m_axi_mem_ARLOCK = 1'b0",
            "assign m_axi_mem_ARCACHE = 4'b0011",
            "assign m_axi_mem_ARPROT = 3'b000",
            "assign m_axi_mem_ARQOS = 4'b0000",
        ],
        &[],
    );
}

#[test]
fn test_generate_rtl_co_located_direct_axi_preserves_original_wiring() {
    let mut without_floorplan = direct_axi_state(None);
    let without_manifest = generate_manifest(&mut without_floorplan);
    let mut co_located = direct_axi_state(Some(Vec::new()));
    let co_located_manifest = generate_manifest(&mut co_located);

    assert_eq!(
        rtl_file(&without_manifest, "top.v"),
        rtl_file(&co_located_manifest, "top.v"),
        "a co-located endpoint must preserve the direct RTL byte-for-byte"
    );
}

#[test]
fn test_generate_rtl_rejects_incomplete_or_inconsistent_direct_axi_routes() {
    let mut cases = Vec::new();

    let mut partial = direct_axi_routes();
    partial.pop();
    cases.push((partial, "partial AXI route set"));

    let mut duplicate = direct_axi_routes();
    duplicate.push(duplicate[0].clone());
    cases.push((duplicate, "more than one ReadAddress route"));

    let mut unknown = direct_axi_routes();
    if let tapa_ir::RoutedChannel::Axi { endpoint, .. } = &mut unknown[0].channel {
        endpoint.top_port = "other".to_string();
    }
    cases.push((unknown, "unknown direct M-AXI endpoint"));

    let mut reversed = direct_axi_routes();
    reversed[0].route.reverse();
    cases.push((reversed, "route must start at child slot"));

    for (routes, expected) in cases {
        let error = generate_rtl(&mut direct_axi_state(Some(routes))).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected '{expected}', got {error}"
        );
    }
}

#[test]
fn test_generate_rtl_does_not_reemit_lower_modules() {
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({"child": [{"args": {}}]});
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("child", plain("child", "lower"))],
    );

    let manifest = run_manifest(
        prog,
        &["top"],
        &[(
            "child",
            "module child(\n  input wire ap_clk,\n  output wire ap_done\n);\n\
                 reg ap_done;\n\
                 always @(*) begin ap_done = 1'b1; end\n\
                 endmodule",
        )],
    );

    assert!(
        !manifest.files().contains_key("rtl/child.v"),
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
    let top = task("top", "upper", |t| {
        t["tasks"] = serde_json::json!({
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
        });
        t["fifos"] = serde_json::json!({
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
        });
    });
    let producer = task("producer", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "ostream", "name": "small", "type": "uint8_t", "width": 8},
            {"cat": "ostream", "name": "wide", "type": "uint32_t", "width": 32}
        ]);
    });
    let consumer = task("consumer", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "istream", "name": "small_in", "type": "uint8_t", "width": 8},
            {"cat": "istream", "name": "wide_in", "type": "uint32_t", "width": 32}
        ]);
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("producer", producer), ("consumer", consumer)],
    );

    let manifest = run_manifest(
        prog,
        &["top"],
        &[
            (
                "producer",
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
            (
                "consumer",
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
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "small_fifo_fifo",
            ".DATA_WIDTH(9)",
            "wire [8:0] small_fifo_dout;",
            "wire [8:0] small_fifo_din;",
            "wide_fifo_fifo",
            ".DATA_WIDTH(33)",
            "wire [32:0] wide_fifo_dout;",
            "wire [32:0] wide_fifo_din;",
        ],
        &[],
    );
}

// ------------------------------------------------------------------
// 5. Multi-thread mmap: two children sharing an mmap arg
// ------------------------------------------------------------------

#[test]
fn test_generate_rtl_multithread_mmap() {
    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mem", "type": "float*", "width": 32}]);
        t["tasks"] = serde_json::json!({
            "worker": [
                {"args": {"data": {"arg": "mem", "cat": "mmap"}}},
                {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
            ]
        });
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(prog, &["top", "worker"], &[]);

    let top_v = rtl_file(&manifest, "top.v");

    // Crossbar instance should appear (2 threads sharing 'mem')
    check(top_v, &["axi_crossbar"], &[]);

    // Downstream wires m_axi_mem_s0_* and m_axi_mem_s1_* should be declared
    check(top_v, &["m_axi_mem_s0_", "m_axi_mem_s1_"], &[]);

    // Crossbar auxiliary RTL file should be generated
    assert!(
        manifest
            .design_files()
            .any(|(path, _)| path.contains("axi_crossbar")),
        "should emit crossbar RTL file, got keys: {:?}",
        manifest
            .design_files()
            .map(|(path, _)| path)
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_generate_rtl_nested_shared_mmap_threads() {
    // `mid` internally shares its mmap between two leaves; `top`
    // shares `mem` between `mid` and a plain leaf. The top-level
    // crossbar must provision the aggregated per-slave thread
    // counts (leaf: 1, mid: 2), not a flat 1.
    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mem", "type": "float*", "width": 32}]);
        t["tasks"] = serde_json::json!({
            "leaf": [{"args": {"d": {"arg": "mem", "cat": "mmap"}}}],
            "mid": [{"args": {"data": {"arg": "mem", "cat": "mmap"}}}]
        });
    });
    let mid = task("mid", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
        t["tasks"] = serde_json::json!({
            "leaf": [
                {"args": {"d": {"arg": "data", "cat": "mmap"}}},
                {"args": {"d": {"arg": "data", "cat": "mmap"}}}
            ]
        });
    });
    let leaf = task("leaf", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "d", "type": "float*", "width": 32}]);
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("top", top), ("mid", mid), ("leaf", leaf)],
    );

    let manifest = run_manifest(prog, &["top", "mid", "leaf"], &[]);

    // mid's own crossbar arbitrates two leaves, 1 thread each.
    let mid_v = rtl_file(&manifest, "mid.v");
    check(mid_v, &["S00_THREADS(1)", "S01_THREADS(1)"], &[]);

    // top's crossbar: slave 0 = leaf (1 thread), slave 1 = mid
    // (2 aggregated threads). Task iteration is alphabetical.
    let top_v = rtl_file(&manifest, "top.v");
    check(top_v, &["S00_THREADS(1)", "S01_THREADS(2)"], &[]);
}

#[test]
fn test_generate_rtl_single_child_mmap_preserves_child_id_width() {
    let mid = task("mid", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "elems", "type": "float*", "width": 32}]);
        t["tasks"] =
            serde_json::json!({"mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]});
    });
    let prog = design("top", "xilinx-hls", &[("mid", mid), ("top", top)]);

    let manifest = run_manifest(prog, &["top"], &[("mid", &wide_axi_id_module_src("mid"))]);

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "output wire [1:0] m_axi_elems_ARID",
            ".m_axi_data_ARID(m_axi_elems_ARID)",
        ],
        &[],
    );
}

#[test]
fn test_generate_rtl_parent_crossbar_zero_extends_narrow_child_ids() {
    let leaf = task("leaf", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}]);
    });
    let mid = task("mid", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "elems", "type": "float*", "width": 32}]);
        t["tasks"] = serde_json::json!({
            "leaf": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}],
            "mid": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}]
        });
    });
    let prog = design(
        "top",
        "xilinx-hls",
        &[("leaf", leaf), ("mid", mid), ("top", top)],
    );

    let manifest = run_manifest(
        prog,
        &["top"],
        &[
            ("leaf", &narrow_axi_id_module_src("leaf")),
            ("mid", &wide_axi_id_module_src("mid")),
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "wire [1:0] m_axi_elems_s0_ARID",
            "assign m_axi_elems_s0_ARID[1:1] = 1'd0",
            "assign m_axi_elems_s0_AWID[1:1] = 1'd0",
            ".m_axi_mmap_ARID(m_axi_elems_s0_ARID[0:0])",
            ".m_axi_mmap_BID(m_axi_elems_s0_BID[0:0])",
        ],
        &[],
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_parent_crossbar_slices_generated_narrow_upper_child_ids() {
    let awide = task("Awide", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let leaf = task("Leaf", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}]);
    });
    let store = task("Store", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mmap", "type": "float*", "width": 32}]);
        t["tasks"] =
            serde_json::json!({"Leaf": [{"args": {"mmap": {"arg": "mmap", "cat": "mmap"}}}]});
    });
    let vec_top = task("VecTop", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "elems", "type": "float*", "width": 32}]);
        t["tasks"] = serde_json::json!({
            "Awide": [{"args": {"data": {"arg": "elems", "cat": "mmap"}}}],
            "Store": [{"args": {"mmap": {"arg": "elems", "cat": "mmap"}}}]
        });
    });
    let prog = design(
        "VecTop",
        "xilinx-hls",
        &[
            ("Awide", awide),
            ("Leaf", leaf),
            ("Store", store),
            ("VecTop", vec_top),
        ],
    );

    let manifest = run_manifest(
        prog,
        &["Store", "VecTop"],
        &[
            ("Awide", &wide_axi_id_module_src("Awide")),
            ("Leaf", &narrow_axi_id_module_src("Leaf")),
        ],
    );

    let top_v = rtl_file(&manifest, "VecTop.v");
    check(
        top_v,
        &[
            "wire [1:0] m_axi_elems_s1_ARID",
            ".m_axi_data_ARID(m_axi_elems_s0_ARID)",
            "assign m_axi_elems_s1_ARID[1:1] = 1'd0",
            ".m_axi_mmap_ARID(m_axi_elems_s1_ARID[0:0])",
            ".m_axi_mmap_BID(m_axi_elems_s1_BID[0:0])",
        ],
        &["assign m_axi_elems_s0_ARID[1:1]"],
    );
}

#[test]
fn test_generate_rtl_hmap_uses_parent_channels() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([
            {
                "cat": "mmap",
                "name": "mem",
                "type": "float*",
                "width": 32,
                "chan_count": 2,
                "chan_size": 1024
            }
        ]);
        t["tasks"] = serde_json::json!({
            "worker": [
                {"args": {"data": {"arg": "mem", "cat": "mmap"}}},
                {"args": {"data": {"arg": "mem", "cat": "mmap"}}}
            ]
        });
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(
        prog,
        &["worker"],
        &[
            (
                "top",
                "module top(input wire ap_clk, input wire ap_rst_n, input wire [63:0] mem_0_offset, input wire [63:0] mem_1_offset); endmodule",
            ),
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "m_axi_mem_0_ARADDR",
            "m_axi_mem_1_ARADDR",
            ".m_axi_data_ARADDR(m_axi_mem_s0_ARADDR)",
            ".m_axi_data_ARADDR(m_axi_mem_s1_ARADDR)",
            "axi_crossbar__mem",
            "m_axi_mem_0_ARADDR_raw",
            "assign m_axi_mem_1_ARADDR = (mem_1_offset + m_axi_mem_1_ARADDR_raw[11:0])",
            ".worker_0__data_offset_in(64'd0)",
            ".worker_1__data_offset_in(64'd0)",
        ],
        &[],
    );
}

#[test]
fn test_generate_rtl_single_channel_hmap_keeps_indexed_channel() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([
            {
                "cat": "mmap",
                "name": "mem",
                "type": "float*",
                "width": 32,
                "chan_count": 1,
                "chan_size": 1024
            }
        ]);
        t["tasks"] =
            serde_json::json!({"worker": [{"args": {"data": {"arg": "mem", "cat": "mmap"}}}]});
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(
        prog,
        &["worker"],
        &[
            (
                "top",
                "module top(input wire ap_clk, input wire ap_rst_n, input wire [63:0] mem_0_offset); endmodule",
            ),
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "m_axi_mem_0_ARADDR",
            "m_axi_mem_0_ARADDR_raw",
            "assign m_axi_mem_0_ARADDR = (mem_0_offset + m_axi_mem_0_ARADDR_raw[11:0])",
            ".worker_0__data_offset_in(64'd0)",
        ],
        &["output wire [63:0] m_axi_mem_ARADDR"],
    );
}

#[test]
fn test_generate_rtl_sanitizes_indexed_mmap_names() {
    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "chan[0]", "type": "float*", "width": 32}]);
        t["tasks"] =
            serde_json::json!({"worker": [{"args": {"mem": {"arg": "chan[0]", "cat": "mmap"}}}]});
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "mem", "type": "float*", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(prog, &["top", "worker"], &[]);

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "m_axi_chan_0_ARADDR",
            ".worker_0__mem_offset_in(chan_0_offset)",
        ],
        &["chan[0]"],
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "fixture covers bridge wiring plus the complete typed floorplan contract"
)]
fn async_mmap_state(floorplanned: bool) -> TopologyWithRtl {
    use std::collections::BTreeMap;
    use tapa_ir::{
        async_mmap_bridge_instance_name, global_controller_instance_name,
        local_controller_instance_name, AxiChannel, AxiEndpoint, ControlChannel, FloorplanResult,
        MemoryBank, MemoryKind, PipelineRoute, PipelineScheme, RoutedChannel,
    };

    let top = task("top", "upper", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "chan[0]", "type": "Elem*", "width": 512}]);
        t["tasks"] = serde_json::json!({"copy": [{"args": {"mem": {"arg": "chan[0]", "cat": "async_mmap"}}}]});
    });
    let copy = task("copy", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "async_mmap", "name": "mem", "type": "Elem*", "width": 512}
        ]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("copy", copy)]);

    let mut state = TopologyWithRtl::new(prog);
    state
        .attach_module("top", parse_module(&handshake_module_src("top")))
        .unwrap();
    state
        .attach_module(
            "copy",
            parse_module(
                "module copy(\n\
                 input wire ap_clk,\n\
                 input wire ap_rst_n,\n\
                 input wire ap_start,\n\
                 output wire ap_done,\n\
                 output wire ap_idle,\n\
                 output wire ap_ready,\n\
                 output wire [63:0] mem_read_addr_s_din,\n\
                 input wire mem_read_addr_s_full_n,\n\
                 output wire mem_read_addr_s_write,\n\
                 input wire [63:0] mem_read_addr_offset,\n\
                 input wire [512:0] mem_read_data_s_dout,\n\
                 input wire mem_read_data_s_empty_n,\n\
                 output wire mem_read_data_s_read,\n\
                 output wire mem_write_addr_s_write,\n\
                 output wire mem_write_data_s_write,\n\
                 output wire mem_write_resp_s_read\n\
                 );\n\
                 assign mem_write_addr_s_write = 1'b0;\n\
                 assign mem_write_data_s_write = 1'b0;\n\
                 assign mem_write_resp_s_read = 1'b0;\n\
                 endmodule",
            ),
        )
        .unwrap();

    if floorplanned {
        let endpoint = AxiEndpoint {
            instance: "copy_0".to_string(),
            port: "mem".to_string(),
            top_port: "chan[0]".to_string(),
        };
        let bank = MemoryBank {
            kind: MemoryKind::Hbm,
            index: 0,
        };
        let mut routes = [AxiChannel::ReadAddress, AxiChannel::ReadData]
            .into_iter()
            .map(|channel| {
                let outgoing = channel == AxiChannel::ReadAddress;
                PipelineRoute {
                    channel: RoutedChannel::Axi {
                        endpoint: endpoint.clone(),
                        bank,
                        channel,
                    },
                    route: if outgoing {
                        vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()]
                    } else {
                        vec!["SLOT_X1Y0".to_string(), "SLOT_X0Y0".to_string()]
                    },
                    scheme: PipelineScheme::Single,
                    reg_regions: Vec::new(),
                }
            })
            .collect::<Vec<_>>();
        for channel in [ControlChannel::Launch, ControlChannel::Reset] {
            routes.push(PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: endpoint.instance.clone(),
                    channel,
                },
                route: vec!["SLOT_X1Y0".to_string(), "SLOT_X0Y0".to_string()],
                scheme: PipelineScheme::Single,
                reg_regions: Vec::new(),
            });
        }
        routes.push(PipelineRoute {
            channel: RoutedChannel::Control {
                instance: endpoint.instance.clone(),
                channel: ControlChannel::Completion,
            },
            route: vec!["SLOT_X0Y0".to_string(), "SLOT_X1Y0".to_string()],
            scheme: PipelineScheme::Single,
            reg_regions: Vec::new(),
        });
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_string(),
            grid: (2, 1),
            regions: BTreeMap::from([
                (endpoint.instance.clone(), "SLOT_X0Y0".to_string()),
                (
                    async_mmap_bridge_instance_name("chan[0]"),
                    "SLOT_X0Y0".to_string(),
                ),
                (
                    global_controller_instance_name().to_string(),
                    "SLOT_X1Y0".to_string(),
                ),
                (
                    local_controller_instance_name(&endpoint.instance),
                    "SLOT_X0Y0".to_string(),
                ),
            ]),
            routes,
            slot_usage: BTreeMap::new(),
        });
    }
    state
}

#[test]
fn test_generate_rtl_instantiates_async_mmap_bridge() {
    let mut state = async_mmap_state(false);
    let manifest = generate_manifest(&mut state);

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "async_mmap #(",
            "chan_0__m_axi",
            ".rst(ap_rst)",
            ".EnableWriteChannel(0)",
            "wire [63:0] chan_0_read_addr__din;",
            ".m_axi_ARADDR(m_axi_chan_0_ARADDR)",
            ".read_data_dout(chan_0_read_data__dout)",
            ".mem_read_addr_s_din(chan_0_read_addr__din)",
            ".mem_read_data_s_dout({1'b0, chan_0_read_data__dout})",
        ],
        &[".m_axi_mem_ARADDR"],
    );
}

#[test]
fn test_generate_rtl_pipelines_only_enabled_async_mmap_channels() {
    let mut state = async_mmap_state(true);
    let manifest = generate_manifest(&mut state);

    let top_v = rtl_file(&manifest, "top.v");
    assert_eq!(top_v.matches("tapa_hs_pipeline #(").count(), 2, "{top_v}");
    check(
        top_v,
        &["__tapa_axi_chan_0_ar", "__tapa_axi_chan_0_r"],
        &[
            "__tapa_axi_chan_0_aw",
            "__tapa_axi_chan_0_w",
            "__tapa_axi_chan_0_b",
        ],
    );
    check(
        top_v,
        &[
            "async_mmap #(",
            "chan_0__m_axi",
            ".rst(!__tapa_control_copy_0__reset_n)",
            ".ap_rst_n(__tapa_control_copy_0__reset_n)",
            ".m_axi_ARADDR(__tapa_axi_chan_0_child_ARADDR)",
            ".m_axi_AWADDR(m_axi_chan_0_AWADDR)",
            ".m_axi_ARLOCK()",
            ".m_axi_AWLOCK(m_axi_chan_0_AWLOCK)",
        ],
        &[
            ".rst(ap_rst)",
            "__tapa_axi_chan_0_child__m_axi",
            "__tapa_axi_chan_0_child_ARLOCK",
            "__tapa_axi_chan_0_child_AWLOCK",
        ],
    );
    check(
        top_v,
        &[
            "assign m_axi_chan_0_ARLOCK = 1'b0",
            "assign m_axi_chan_0_ARCACHE = 4'b0011",
            "assign m_axi_chan_0_ARPROT = 3'b000",
            "assign m_axi_chan_0_ARQOS = 4'b0000",
        ],
        &[],
    );
    check(top_v, &[], &["assign m_axi_chan_0_AWLOCK"]);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "integration test with many assertions"
)]
fn test_generate_rtl_top_instantiates_control_s_axi() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "mmap", "name": "mem", "type": "float*", "width": 32},
            {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
        ]);
        t["tasks"] = serde_json::json!({
            "worker": [{
                "args": {"data": {"arg": "mem", "cat": "mmap"}, "n": {"arg": "n", "cat": "scalar"}}
            }]
        });
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] = serde_json::json!([
            {"cat": "mmap", "name": "data", "type": "float*", "width": 32},
            {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64}
        ]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(
        prog,
        &[],
        &[
            (
                "top",
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
            (
                "worker",
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
        ],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[
            "top_control_s_axi",
            "control_s_axi_U",
            ".mem_offset(mem_offset)",
            ".n(n)",
        ],
        &[
            "assign ap_done = ap_start",
            "assign ap_ready = ap_start",
            "ap_CS_fsm",
        ],
    );
}

#[test]
fn test_generate_rtl_top_control_unrolls_hmap_offsets() {
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([
            {
                "cat": "mmap",
                "name": "mem",
                "type": "float*",
                "width": 32,
                "chan_count": 2,
                "chan_size": 1024
            }
        ]);
        t["tasks"] =
            serde_json::json!({"worker": [{"args": {"data": {"arg": "mem", "cat": "mmap"}}}]});
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] =
            serde_json::json!([{"cat": "mmap", "name": "data", "type": "float*", "width": 32}]);
    });
    let prog = design("top", "xilinx-hls", &[("top", top), ("worker", worker)]);

    let manifest = run_manifest(
        prog,
        &["worker"],
        &[(
            "top",
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
        )],
    );

    let top_v = rtl_file(&manifest, "top.v");
    check(
        top_v,
        &[".mem_0_offset(mem_0_offset)", ".mem_1_offset(mem_1_offset)"],
        &[".mem_offset(mem_offset)"],
    );
}
