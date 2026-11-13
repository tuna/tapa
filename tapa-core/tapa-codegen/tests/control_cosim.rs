//! Verilator protocol test for distributed control and feed-forward pipelines.

mod common;

use common::verilator::run_cosim;

const HARNESS: &str = "
`default_nettype none
module control_harness (
  input  wire       ap_clk,
  input  wire       ap_rst_n,
  input  wire       ap_start,
  input  wire       child_done,
  input  wire       child_ready,
  input  wire [7:0] probe_in,
  input  wire       poison_write,
  input  wire [7:0] poison_data,
  output wire [7:0] probe_out,
  output wire       poison_ready,
  output wire       poison_valid,
  output wire [7:0] poison_out,
  output wire       ap_done,
  output wire       ap_ready,
  output wire       ap_idle,
  output wire       child_start,
  output wire       autorun_start,
  output wire       local_reset_n,
  output wire       fabric_reset_n,
  output wire       global_release,
  output wire       local_completion,
  output wire       global_completion
);
  wire global_start;
  wire [1:0] launch_input;
  wire [1:0] launch_output;
  wire autorun_completion;
  assign launch_input = {global_release, global_start};

  tapa_global_controller #(
    .FLUSH_CYCLES(6)
  ) global_controller (
    .ap_clk(ap_clk),
    .ap_rst_n(ap_rst_n),
    .ap_start(ap_start),
    .children_done(global_completion),
    .children_clear(~global_completion),
    .launch_start(global_start),
    .launch_release(global_release),
    .fabric_reset_n(fabric_reset_n),
    .ap_done(ap_done),
    .ap_ready(ap_ready),
    .ap_idle(ap_idle)
  );

  tapa_control_pipeline #(
    .WIDTH(2),
    .BODY_LEVEL(0)
  ) launch_pipeline (
    .clk(ap_clk),
    .in_data(launch_input),
    .out_data(launch_output)
  );

  tapa_control_pipeline #(
    .WIDTH(1),
    .BODY_LEVEL(0)
  ) reset_pipeline (
    .clk(ap_clk),
    .in_data(ap_rst_n),
    .out_data(local_reset_n)
  );

  tapa_local_controller #(
    .AUTORUN(0)
  ) local_controller (
    .ap_clk(ap_clk),
    .reset_n(local_reset_n),
    .launch_start(launch_output[0]),
    .launch_release(launch_output[1]),
    .child_done(child_done),
    .child_ready(child_ready),
    .child_idle(1'b0),
    .child_start(child_start),
    .completion(local_completion)
  );

  tapa_local_controller #(
    .AUTORUN(1)
  ) autorun_controller (
    .ap_clk(ap_clk),
    .reset_n(local_reset_n),
    .launch_start(launch_output[0]),
    .launch_release(1'b0),
    .child_done(1'b0),
    .child_ready(1'b0),
    .child_idle(1'b0),
    .child_start(autorun_start),
    .completion(autorun_completion)
  );

  tapa_control_pipeline #(
    .WIDTH(1),
    .BODY_LEVEL(2)
  ) completion_pipeline (
    .clk(ap_clk),
    .in_data(local_completion),
    .out_data(global_completion)
  );

  tapa_control_pipeline #(
    .WIDTH(8),
    .BODY_LEVEL(1)
  ) probe_pipeline (
    .clk(ap_clk),
    .in_data(probe_in),
    .out_data(probe_out)
  );

  tapa_hs_pipeline #(
    .DATA_WIDTH(8),
    .DEPTH(2),
    .BODY_LEVEL(1)
  ) poison_pipeline (
    .clk(ap_clk),
    .reset(~fabric_reset_n),
    .if_full_n(poison_ready),
    .if_write_ce(1'b1),
    .if_write(poison_write),
    .if_din(poison_data),
    .if_empty_n(poison_valid),
    .if_read_ce(1'b1),
    .if_read(1'b0),
    .if_dout(poison_out)
  );
endmodule
`default_nettype wire
";

const TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vcontrol_harness.h"
#include <cstdio>

static Vcontrol_harness* dut;

static void posedge() {
    dut->ap_clk = 1;
    dut->eval();
    dut->ap_clk = 0;
    dut->eval();
}

static int wait_for(const char* name, CData* signal, int limit) {
    for (int cycle = 0; cycle < limit; ++cycle) {
        dut->eval();
        if (*signal) return cycle;
        posedge();
    }
    std::printf("FAIL: timed out waiting for %s\n", name);
    return -1;
}

int main(int argc, char** argv) {
    Verilated::commandArgs(argc, argv);
    dut = new Vcontrol_harness;
    dut->ap_clk = 0;
    dut->ap_rst_n = 1;
    dut->ap_start = 0;
    dut->child_done = 0;
    dut->child_ready = 0;
    dut->probe_in = 0;
    dut->poison_write = 0;
    dut->poison_data = 0xde;
    dut->eval();

    if (dut->probe_out != 0 || dut->local_reset_n != 0 ||
        dut->global_completion != 0) {
        std::printf("FAIL: feed-forward registers were not initialized to zero\n");
        return 1;
    }

    // BODY_LEVEL=1 means Head, one Body, and Tail: exactly three edges.
    dut->probe_in = 0xa5;
    posedge();
    dut->probe_in = 0;
    if (dut->probe_out != 0) {
        std::printf("FAIL: probe bypassed Head\n");
        return 2;
    }
    posedge();
    if (dut->probe_out != 0) {
        std::printf("FAIL: probe bypassed Body\n");
        return 3;
    }
    posedge();
    if (dut->probe_out != 0xa5) {
        std::printf("FAIL: probe did not emerge from Tail after three edges\n");
        return 4;
    }

    dut->ap_rst_n = 0;
    posedge();
    dut->ap_rst_n = 1;
    for (int i = 0; i < 16; ++i) posedge();

    // Create a held completion, then reset the global controller while that
    // stale high value is still present on the return path.
    dut->ap_start = 1;
    if (wait_for("poison child_start", &dut->child_start, 32) < 0) return 5;
    if (!dut->autorun_start) {
        std::printf("FAIL: autorun start did not latch with Launch\n");
        return 6;
    }
    dut->ap_start = 0;
    dut->child_ready = 1;
    dut->child_done = 1;
    posedge();
    dut->child_ready = 0;
    dut->child_done = 0;
    if (wait_for("stale completion", &dut->global_completion, 32) < 0) return 7;

    // The child keeps writing until its routed reset arrives. The parent-side
    // fabric reset must remain asserted long enough to drain that traffic from
    // the resetless Head and Body into the resettable Tail.
    if (!dut->poison_ready || dut->poison_valid) {
        std::printf("FAIL: poison data path was not empty and writable before reset\n");
        return 29;
    }
    dut->poison_write = 1;
    dut->ap_rst_n = 0;
    posedge();
    dut->ap_rst_n = 1;
    dut->ap_start = 1;
    dut->eval();
    if (dut->fabric_reset_n) {
        std::printf("FAIL: parent fabric left reset before the flush guard\n");
        return 8;
    }

    int local_reset_cycles = 0;
    while (dut->local_reset_n && local_reset_cycles < 16) {
        ++local_reset_cycles;
        posedge();
    }
    if (dut->local_reset_n) {
        std::printf("FAIL: routed reset did not reach the writing child\n");
        return 30;
    }
    dut->poison_write = 0;

    int autorun_clear_cycles = 0;
    while (dut->autorun_start && autorun_clear_cycles < 16) {
        ++autorun_clear_cycles;
        posedge();
    }
    if (dut->autorun_start) {
        std::printf("FAIL: routed reset did not clear autorun start\n");
        return 9;
    }

    int guarded_cycles = 0;
    while (!dut->child_start && guarded_cycles < 64) {
        if (dut->ap_ready || dut->ap_done) {
            std::printf("FAIL: stale completion escaped the reset guard\n");
            return 10;
        }
        ++guarded_cycles;
        posedge();
    }
    if (!dut->child_start || guarded_cycles < 6) {
        std::printf("FAIL: reset guard was too short or never relaunched (%d cycles)\n",
                    guarded_cycles);
        return 11;
    }
    if (!dut->fabric_reset_n || !dut->autorun_start) {
        std::printf("FAIL: autorun did not relatch after routed reset\n");
        return 12;
    }
    for (int i = 0; i < 8; ++i) {
        if (dut->poison_valid) {
            std::printf("FAIL: reset guard released with poison 0x%02x in the data path\n",
                        (unsigned)dut->poison_out);
            return 31;
        }
        posedge();
    }

    // Ready may precede done. The local controller must enter WAITING, drop
    // child_start, and withhold completion until done follows.
    dut->child_ready = 1;
    dut->child_done = 0;
    posedge();
    dut->child_ready = 0;
    if (dut->child_start || dut->local_completion) {
        std::printf("FAIL: ready-before-done did not enter WAITING\n");
        return 13;
    }
    for (int i = 0; i < 3; ++i) {
        if (dut->ap_ready || dut->ap_done) {
            std::printf("FAIL: ready was mistaken for completion\n");
            return 14;
        }
        posedge();
    }
    dut->child_done = 1;
    posedge();
    dut->child_done = 0;

    if (wait_for("release", &dut->global_release, 64) < 0) return 15;
    int release_cycles = 0;
    while (!dut->ap_ready && release_cycles < 64) {
        if (!dut->global_release) {
            std::printf("FAIL: release dropped before all completions cleared\n");
            return 16;
        }
        ++release_cycles;
        posedge();
    }
    if (!dut->ap_ready || !dut->ap_done || release_cycles < 5) {
        std::printf("FAIL: DONE preceded clear or release was too short (%d cycles)\n",
                    release_cycles);
        return 17;
    }

    // Model the AXI-Lite controller clearing the original held start on the
    // ap_ready edge. DONE must not sample that old high as a second launch.
    posedge();
    dut->ap_start = 0;
    dut->eval();
    if (dut->ap_ready || dut->ap_done) {
        std::printf("FAIL: DONE/ready lasted more than one cycle\n");
        return 18;
    }
    for (int i = 0; i < 8; ++i) {
        if (dut->child_start || dut->ap_ready || dut->ap_done) {
            std::printf("FAIL: held original start was duplicated after ap_ready\n");
            return 19;
        }
        posedge();
    }

    // A distinct request asserted from IDLE must launch normally.
    dut->ap_start = 1;
    if (wait_for("second child_start", &dut->child_start, 32) < 0) return 20;

    // Let the accepted request fall, then assert the next request while this
    // invocation is busy and hold it through ap_ready. This is distinct from
    // the original held request above and must be remembered exactly once.
    dut->ap_start = 0;
    posedge();
    dut->ap_start = 1;
    posedge();

    // Neither the queued request nor a stale returned completion may finish
    // the current invocation.
    for (int i = 0; i < 6; ++i) {
        if (dut->ap_ready || dut->ap_done) {
            std::printf("FAIL: queued request or stale completion finished the current run\n");
            return 21;
        }
        posedge();
    }
    dut->child_ready = 1;
    dut->child_done = 1;
    posedge();
    dut->child_ready = 0;
    dut->child_done = 0;
    if (wait_for("second ap_ready", &dut->ap_ready, 64) < 0) return 22;
    if (!dut->ap_done) {
        std::printf("FAIL: ap_ready and ap_done diverged\n");
        return 23;
    }

    // The ready edge clears the held S-AXI start level. The remembered token,
    // rather than that old level, must launch the queued invocation.
    posedge();
    dut->ap_start = 0;
    dut->eval();
    if (dut->ap_ready || dut->ap_done) {
        std::printf("FAIL: second DONE/ready lasted more than one cycle\n");
        return 24;
    }
    if (wait_for("queued child_start", &dut->child_start, 32) < 0) return 25;

    for (int i = 0; i < 8; ++i) {
        if (dut->ap_ready || dut->ap_done) {
            std::printf("FAIL: stale completion prematurely finished the queued run\n");
            return 26;
        }
        posedge();
    }
    dut->child_ready = 1;
    dut->child_done = 1;
    posedge();
    dut->child_ready = 0;
    dut->child_done = 0;
    if (wait_for("queued ap_ready", &dut->ap_ready, 64) < 0) return 27;
    if (!dut->ap_done) {
        std::printf("FAIL: queued ap_ready and ap_done diverged\n");
        return 28;
    }

    std::printf("PASS: distributed control reset, release, held-start, and queued launches\n");
    return 0;
}
"#;

#[test]
fn distributed_control_survives_reset_and_consecutive_launches() {
    run_cosim(
        "control_harness",
        &[],
        &["relay_station.v", "tapa_hs_pipeline.v", "tapa_control.v"],
        &[("harness.v", HARNESS)],
        TESTBENCH,
    );
}

/// The physical leaf graph must retain the invocation barriers of the
/// original upper scopes, including the scope's scalar binding boundary.
#[test]
fn flattened_detached_scope_preserves_its_joined_children() {
    use common::run_manifest::{generate_manifest, rtl_file};

    let graph = scope_design();
    for blocked in [true, false] {
        for floorplanned in [false, true] {
            let mut source_graph = graph.clone();
            if !blocked {
                source_graph
                    .tasks
                    .get_mut("Top")
                    .unwrap()
                    .tasks
                    .get_mut("Background")
                    .unwrap()[0]
                    .step = 0;
            }
            let finite_sibling = SCOPE_FINISH.replace("module Finish", "module Never");
            let sibling = if blocked {
                SCOPE_NEVER
            } else {
                &finite_sibling
            };
            let mut state = scope_state(source_graph, sibling, floorplanned);
            let manifest = generate_manifest(&mut state);
            let mut sources = vec![("Top.v", rtl_file(&manifest, "Top.v").as_str())];
            if let Some(fsm) = manifest.files().get("rtl/Top_fsm.v") {
                sources.push(("Top_fsm.v", fsm));
            }
            sources.extend([
                ("Once.v", SCOPE_ONCE),
                ("Never.v", sibling),
                ("Finish.v", SCOPE_FINISH),
                ("scope_harness.v", SCOPE_HARNESS),
            ]);
            let testbench = SCOPE_TESTBENCH.replace(
                "const int forwarding_cycles = 1;",
                if floorplanned {
                    "const int forwarding_cycles = 4;"
                } else {
                    "const int forwarding_cycles = 1;"
                },
            );
            let testbench = testbench.replace(
                "const bool blocked = true;",
                if blocked {
                    "const bool blocked = true;"
                } else {
                    "const bool blocked = false;"
                },
            );
            run_cosim(
                "scope_harness",
                &[],
                &["tapa_control.v"],
                &sources,
                &testbench,
            );
        }
    }
}

#[test]
fn flattened_scope_rejects_missing_scalar_argument_before_mutation() {
    let mut graph = scope_design();
    graph
        .tasks
        .get_mut("Top")
        .unwrap()
        .tasks
        .get_mut("Background")
        .unwrap()[0]
        .args
        .clear();
    for floorplanned in [false, true] {
        let mut state = scope_state(graph.clone(), SCOPE_NEVER, floorplanned);
        let before = state.module_map["Top"].emit().unwrap();
        let error = tapa_codegen::generate_rtl(&mut state)
            .expect_err("a missing original upper scalar argument must be rejected");
        assert!(
            error.to_string().contains("captured") && error.to_string().contains("scalar"),
            "floorplanned={floorplanned}: {error}"
        );
        assert_eq!(state.module_map["Top"].emit().unwrap(), before);
        assert!(state.fsm_modules.is_empty());
        assert!(state.generated_files.is_empty());
    }
}

fn scope_state(
    graph: tapa_ir::Design,
    sibling: &str,
    floorplanned: bool,
) -> tapa_codegen::rtl_state::TopologyWithRtl {
    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(graph);
    state.flat_projection = Some(tapa_ir::transforms::flatten_with_origins(&state.design).unwrap());
    for (name, source) in [
        ("Top", SCOPE_TOP),
        ("Background", SCOPE_BACKGROUND),
        ("Once", SCOPE_ONCE),
        ("Never", sibling),
        ("Finish", SCOPE_FINISH),
    ] {
        state
            .attach_module(name, common::parse_module(source))
            .unwrap();
    }
    if floorplanned {
        state.floorplan = Some(scope_floorplan());
    }
    state
}

fn scope_design() -> tapa_ir::Design {
    use common::{design, task};
    use serde_json::json;
    design(
        "Top",
        "xilinx-hls",
        &[
            (
                "Top",
                task("Top", "upper", |t| {
                    t["ports"] =
                        json!([{"cat":"scalar", "name":"value", "type":"uint32_t", "width":32}]);
                    t["tasks"] = json!({
                        "Background":[{"name":"bg", "step":-1, "args":{"captured":{"cat":"scalar", "arg":"value"}}}],
                        "Finish":[{"name":"finish", "args":{}}]
                    });
                }),
            ),
            (
                "Background",
                task("Background", "upper", |t| {
                    t["ports"] =
                        json!([{"cat":"scalar", "name":"captured", "type":"uint32_t", "width":32}]);
                    t["tasks"] = json!({
                        "Once":[{"name":"once", "args":{"value_offset":{"cat":"scalar", "arg":"captured"}}}],
                        "Never":[{"name":"never", "args":{}}]
                    });
                }),
            ),
            (
                "Once",
                task("Once", "lower", |t| {
                    t["ports"] = json!([{"cat":"scalar", "name":"value_offset", "type":"uint32_t", "width":32}]);
                }),
            ),
            ("Never", task("Never", "lower", |_| {})),
            ("Finish", task("Finish", "lower", |_| {})),
        ],
    )
}

fn scope_floorplan() -> tapa_ir::FloorplanResult {
    use std::collections::BTreeMap;
    use tapa_ir::{
        global_controller_instance_name, local_controller_instance_name,
        scope_controller_instance_name, ControlChannel, FloorplanResult, PipelineRoute,
        PipelineScheme, RoutedChannel,
    };
    let central = "SLOT_X0Y0";
    let remote = "SLOT_X1Y0";
    let mut regions = BTreeMap::from([
        (
            global_controller_instance_name().to_owned(),
            central.to_owned(),
        ),
        (scope_controller_instance_name("bg_Top"), central.to_owned()),
        (local_controller_instance_name("bg_Top"), central.to_owned()),
    ]);
    let mut routes = Vec::new();
    for instance in ["once_bg_Top", "never_bg_Top", "finish"] {
        regions.insert(instance.to_owned(), remote.to_owned());
        regions.insert(local_controller_instance_name(instance), remote.to_owned());
        // Even under a detached ancestor these original joined leaves have
        // held completion routed back to their own central scope controller.
        for channel in [
            ControlChannel::Launch,
            ControlChannel::Reset,
            ControlChannel::Completion,
        ] {
            let route = if channel == ControlChannel::Completion {
                vec![remote.to_owned(), central.to_owned()]
            } else {
                vec![central.to_owned(), remote.to_owned()]
            };
            routes.push(PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: instance.to_owned(),
                    channel,
                },
                route,
                scheme: PipelineScheme::Double,
                reg_regions: vec![central.to_owned()],
            });
        }
    }
    FloorplanResult {
        device: "u280".to_owned(),
        grid: (2, 1),
        regions,
        routes,
        slot_usage: BTreeMap::new(),
    }
}

const SCOPE_TOP: &str = "
module Top(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
           output wire ap_done, output wire ap_ready, output wire ap_idle,
           input wire [31:0] value);
endmodule
";
const SCOPE_BACKGROUND: &str = "
module Background(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
           output wire ap_done, output wire ap_ready, output wire ap_idle,
           input wire [31:0] captured);
endmodule
";
const SCOPE_ONCE: &str = "
module Once(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
            output reg ap_done, output wire ap_ready, output wire ap_idle,
            input wire [31:0] value_offset);
  reg busy = 0;
  reg [31:0] count = 0;
  reg [31:0] accepted_value = 0;
  assign ap_ready = ap_done;
  assign ap_idle = !busy && !ap_done;
  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      busy <= 0;
      ap_done <= 0;
      count <= 0;
      accepted_value <= 0;
    end else begin
      ap_done <= 0;
      if (busy) begin
        busy <= 0;
        ap_done <= 1;
      end else if (ap_start && !ap_done) begin
        busy <= 1;
        count <= count + 1;
        accepted_value <= value_offset;
      end
    end
  end
endmodule
";
const SCOPE_NEVER: &str = "
module Never(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
             output wire ap_done, output wire ap_ready, output wire ap_idle);
  assign ap_done = 0;
  assign ap_ready = 0;
  assign ap_idle = !ap_start;
endmodule
";
const SCOPE_FINISH: &str = "
module Finish(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
              output reg ap_done, output wire ap_ready, output wire ap_idle);
  reg busy = 0;
  assign ap_ready = ap_done;
  assign ap_idle = !busy && !ap_done;
  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin busy <= 0; ap_done <= 0; end
    else begin
      ap_done <= 0;
      if (busy) begin busy <= 0; ap_done <= 1; end
      else if (ap_start && !ap_done) busy <= 1;
    end
  end
endmodule
";
const SCOPE_HARNESS: &str = "
module scope_harness(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
                     input wire [31:0] value, output wire ap_done,
                     output wire [31:0] count, output wire [31:0] accepted_value,
                     output wire [31:0] forwarded_value);
  wire ap_ready, ap_idle;
  Top dut(.ap_clk(ap_clk), .ap_rst_n(ap_rst_n), .ap_start(ap_start),
          .ap_done(ap_done), .ap_ready(ap_ready), .ap_idle(ap_idle), .value(value));
  assign count = dut.once_bg_Top.count;
  assign accepted_value = dut.once_bg_Top.accepted_value;
  assign forwarded_value = dut.once_bg_Top.value_offset;
endmodule
";
const SCOPE_TESTBENCH: &str = r#"
#include "Vscope_harness.h"
#include <cstdio>
#include <memory>
int main() {
  const bool blocked = true;
  const int forwarding_cycles = 1;
  auto dut = std::make_unique<Vscope_harness>();
  auto tick = [&]() { dut->ap_clk = 0; dut->eval(); dut->ap_clk = 1; dut->eval(); };
  dut->ap_start = 0; dut->value = 0x1234; dut->ap_rst_n = 0;
  for (int i = 0; i < 4; ++i) tick();
  dut->ap_rst_n = 1;
  for (int i = 0; i < 8; ++i) tick();
  // Observe the actual leaf input: upper formal forwarding remains a
  // one-cycle register even when its datapath wrapper is removed. Routed
  // leaf launch bundles add their declared three pipeline stages.
  dut->value = 0x5678; dut->ap_clk = 0; dut->eval();
  for (int i = 0; i < forwarding_cycles; ++i) {
    if (dut->forwarded_value != 0x1234) {
      std::puts("FAIL: scalar forwarding changed before its clocked boundary"); return 5;
    }
    tick();
  }
  if (dut->forwarded_value != 0x5678) {
    std::puts("FAIL: scalar forwarding did not arrive at its clocked boundary"); return 6;
  }
  dut->value = 0x1234;
  for (int i = 0; i < forwarding_cycles; ++i) tick();
  for (int run = 0; run < 2; ++run) {
    dut->ap_start = 1; tick(); dut->ap_start = 0;
    bool completed = false;
    for (int i = 0; i < 128; ++i) {
      tick();
      if (dut->ap_done) { completed = true; break; }
    }
    if (!completed) { std::puts("FAIL: detached upper blocked parent completion"); return 1; }
    for (int i = 0; i < 32; ++i) tick();
    const unsigned expected_count = blocked ? 1u : unsigned(run + 1);
    if (dut->count != expected_count) { std::printf("FAIL: joined finite child executed %u times (expected %u)\n", dut->count, expected_count); return 2; }
    if (dut->accepted_value != 0x1234) { std::printf("FAIL: upper scalar binding lost: %x\n", dut->accepted_value); return 3; }
  }
  // Root reset is the only event that restarts the unfinished detached group.
  dut->ap_rst_n = 0; for (int i = 0; i < 4; ++i) tick();
  dut->ap_rst_n = 1; for (int i = 0; i < 8; ++i) tick();
  dut->ap_start = 1; tick(); dut->ap_start = 0;
  for (int i = 0; i < 64; ++i) tick();
  if (dut->count != 1) { std::puts("FAIL: root reset did not restart detached group"); return 4; }
  std::puts("PASS: detached scope retains local join and scalar forwarding");
  return 0;
}
"#;

/// A detached task that returns runs once per invocation of its parent, with
/// that invocation's arguments, under every control: the per-task FSM, a
/// flattened top's scoped controllers, and a floorplan's distributed ones. A
/// detached leaf (`w`) and the joined leaf of a detached upper task
/// (`g`/`inner`) each run for longer than their parent: one invocation must
/// not restart them on stale arguments once they return, and an invocation
/// starting while they still run must start them once more after they
/// finish.
#[test]
fn a_finite_detached_task_runs_once_per_parent_invocation() {
    use common::run_manifest::generate_manifest;

    for (flattened, floorplanned) in [(false, false), (true, false), (true, true)] {
        let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(detached_design());
        if flattened {
            state.flat_projection =
                Some(tapa_ir::transforms::flatten_with_origins(&state.design).unwrap());
        }
        for (name, source) in [
            ("Top", DETACHED_TOP),
            ("Group", DETACHED_GROUP),
            ("Worker", DETACHED_WORKER),
            ("Finish", SCOPE_FINISH),
        ] {
            state
                .attach_module(name, common::parse_module(source))
                .unwrap();
        }
        if floorplanned {
            state.floorplan = Some(detached_floorplan());
        }
        let manifest = generate_manifest(&mut state);
        let inner = if flattened { "inner_g_Top" } else { "g.inner" };
        let harness = DETACHED_HARNESS.replace("INNER", inner);
        let mut sources: Vec<(&str, &str)> = manifest
            .design_files()
            .map(|(path, rtl)| (path.trim_start_matches("rtl/"), rtl.as_str()))
            .collect();
        sources.extend([
            ("Worker.v", DETACHED_WORKER),
            ("Finish.v", SCOPE_FINISH),
            ("detached_harness.v", harness.as_str()),
        ]);
        run_cosim(
            "detached_harness",
            &[],
            &["tapa_control.v"],
            &sources,
            DETACHED_TESTBENCH,
        );
    }
}

fn detached_design() -> tapa_ir::Design {
    use common::{design, task};
    use serde_json::json;
    let value = |arg: &str| json!({"cat": "scalar", "arg": arg});
    design(
        "Top",
        "xilinx-hls",
        &[
            (
                "Top",
                task("Top", "upper", |t| {
                    t["ports"] =
                        json!([{"cat":"scalar", "name":"value", "type":"uint32_t", "width":32}]);
                    t["tasks"] = json!({
                        "Worker": [{"name": "w", "step": -1, "args": {"value": value("value")}}],
                        "Group": [{"name": "g", "step": -1, "args": {"captured": value("value")}}],
                        "Finish": [{"name": "f", "args": {}}]
                    });
                }),
            ),
            (
                "Group",
                task("Group", "upper", |t| {
                    t["ports"] =
                        json!([{"cat":"scalar", "name":"captured", "type":"uint32_t", "width":32}]);
                    t["tasks"] = json!({
                        "Worker": [{"name": "inner", "args": {"value": value("captured")}}]
                    });
                }),
            ),
            (
                "Worker",
                task("Worker", "lower", |t| {
                    t["ports"] =
                        json!([{"cat":"scalar", "name":"value", "type":"uint32_t", "width":32}]);
                }),
            ),
            ("Finish", task("Finish", "lower", |_| {})),
        ],
    )
}

/// The leaves away from the controllers, so every launch crosses a pipeline.
fn detached_floorplan() -> tapa_ir::FloorplanResult {
    use std::collections::BTreeMap;
    use tapa_ir::{
        global_controller_instance_name, local_controller_instance_name,
        scope_controller_instance_name, ControlChannel, FloorplanResult, PipelineRoute,
        PipelineScheme, RoutedChannel,
    };
    let central = "SLOT_X0Y0";
    let remote = "SLOT_X1Y0";
    let mut regions = BTreeMap::from([
        (
            global_controller_instance_name().to_owned(),
            central.to_owned(),
        ),
        (scope_controller_instance_name("g_Top"), central.to_owned()),
        (local_controller_instance_name("g_Top"), central.to_owned()),
    ]);
    let mut routes = Vec::new();
    for (instance, joined) in [("w", false), ("inner_g_Top", true), ("f", true)] {
        regions.insert(instance.to_owned(), remote.to_owned());
        regions.insert(local_controller_instance_name(instance), remote.to_owned());
        let mut channels = vec![ControlChannel::Launch, ControlChannel::Reset];
        if joined {
            channels.push(ControlChannel::Completion);
        }
        for channel in channels {
            let route = if channel == ControlChannel::Completion {
                vec![remote.to_owned(), central.to_owned()]
            } else {
                vec![central.to_owned(), remote.to_owned()]
            };
            routes.push(PipelineRoute {
                channel: RoutedChannel::Control {
                    instance: instance.to_owned(),
                    channel,
                },
                route,
                scheme: PipelineScheme::Double,
                reg_regions: vec![central.to_owned()],
            });
        }
    }
    FloorplanResult {
        device: "u280".to_owned(),
        grid: (2, 1),
        regions,
        routes,
        slot_usage: BTreeMap::new(),
    }
}

const DETACHED_TOP: &str = "
module Top(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
           output wire ap_done, output wire ap_ready, output wire ap_idle,
           input wire [31:0] value);
endmodule
";
const DETACHED_GROUP: &str = "
module Group(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
             output wire ap_done, output wire ap_ready, output wire ap_idle,
             input wire [31:0] captured);
endmodule
";
/// A sequential `ap_ctrl_hs` leaf, as Vitis HLS builds one: it takes its
/// argument when it starts, runs for 64 cycles, and is ready only when done.
/// It counts its runs and keeps the arguments of the last two.
const DETACHED_WORKER: &str = "
module Worker(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
              output wire ap_done, output wire ap_ready, output wire ap_idle,
              input wire [31:0] value);
  reg busy = 0;
  reg [7:0] left = 0;
  reg [31:0] count = 0;
  reg [31:0] previous = 0;
  reg [31:0] last = 0;
  assign ap_done = busy && left == 0;
  assign ap_ready = ap_done;
  assign ap_idle = !busy;
  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      busy <= 0;
      left <= 0;
      count <= 0;
    end else if (busy) begin
      if (left == 0) busy <= 0;
      else left <= left - 1;
    end else if (ap_start) begin
      busy <= 1;
      left <= 63;
      count <= count + 1;
      previous <= last;
      last <= value;
    end
  end
endmodule
";
const DETACHED_HARNESS: &str = "
module detached_harness(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
                        input wire [31:0] value, output wire ap_ready,
                        output wire [31:0] w_count, output wire [31:0] w_previous,
                        output wire [31:0] w_last, output wire [31:0] g_count,
                        output wire [31:0] g_previous, output wire [31:0] g_last);
  wire ap_done, ap_idle;
  Top dut(.ap_clk(ap_clk), .ap_rst_n(ap_rst_n), .ap_start(ap_start),
          .ap_done(ap_done), .ap_ready(ap_ready), .ap_idle(ap_idle), .value(value));
  assign w_count = dut.w.count;
  assign w_previous = dut.w.previous;
  assign w_last = dut.w.last;
  assign g_count = dut.INNER.count;
  assign g_previous = dut.INNER.previous;
  assign g_last = dut.INNER.last;
endmodule
";
const DETACHED_TESTBENCH: &str = r#"
#include "Vdetached_harness.h"
#include <cstdio>
#include <memory>
int main() {
  auto dut = std::make_unique<Vdetached_harness>();
  auto tick = [&]() { dut->ap_clk = 0; dut->eval(); dut->ap_clk = 1; dut->eval(); };
  dut->ap_start = 0; dut->value = 0; dut->ap_rst_n = 0;
  for (int i = 0; i < 4; ++i) tick();
  dut->ap_rst_n = 1;
  for (int i = 0; i < 32; ++i) tick();
  // One invocation as the control registers make it: the arguments first,
  // then the start, held until ready. The parent waits for `f` only.
  auto invoke = [&](unsigned value) {
    dut->value = value;
    tick(); tick();
    dut->ap_start = 1;
    for (int i = 0; i < 256; ++i) {
      tick();
      if (dut->ap_ready) { dut->ap_start = 0; return true; }
    }
    return false;
  };
  auto settle = [&]() { for (int i = 0; i < 512; ++i) tick(); };
  auto check = [&](const char* phase, unsigned count, unsigned previous, unsigned last) {
    const struct { const char* name; unsigned count, previous, last; } runs[] = {
      {"detached leaf", dut->w_count, dut->w_previous, dut->w_last},
      {"detached upper's leaf", dut->g_count, dut->g_previous, dut->g_last},
    };
    for (const auto& run : runs) {
      if (run.count != count || run.last != last ||
          (count > 1 && run.previous != previous)) {
        std::printf("FAIL: %s: %s ran %u times, last two on %x, %x (expected %u, on %x, %x)\n",
                    phase, run.name, run.count, run.previous, run.last, count, previous, last);
        return false;
      }
    }
    return true;
  };

  // A run that ends before the next invocation is not restarted on its
  // stale arguments.
  if (!invoke(0xa)) { std::puts("FAIL: first invocation never finished"); return 1; }
  settle();
  if (!check("one invocation", 1, 0, 0xa)) return 2;
  settle();
  if (!check("one invocation, later", 1, 0, 0xa)) return 3;

  // The next invocation starts while the run of this one is still going: it
  // runs once more after that, on the arguments it was started with.
  if (!invoke(0xb)) { std::puts("FAIL: second invocation never finished"); return 4; }
  if (!invoke(0xc)) { std::puts("FAIL: third invocation never finished"); return 5; }
  if (dut->w_count != 2 || dut->g_count != 2) {
    std::printf("FAIL: the third invocation did not overlap a run (%u, %u)\n",
                dut->w_count, dut->g_count);
    return 6;
  }
  settle();
  if (!check("overlapping invocations", 3, 0xb, 0xc)) return 7;
  settle();
  if (!check("overlapping invocations, later", 3, 0xb, 0xc)) return 8;
  std::puts("PASS: a finite detached task runs once per parent invocation");
  return 0;
}
"#;

/// An output ring holds the top's done until the frames its task released
/// are in memory, under every control: the per-task FSM, a floorplan's
/// distributed controllers, and a flattened top's scoped ones. Its drained
/// state is also the idle one, so it must not keep a distributed top from
/// starting.
#[test]
fn an_output_ring_holds_done_until_its_frames_are_in_memory() {
    use common::run_manifest::{generate_manifest, rtl_file};

    for (floorplanned, flattened) in [(false, false), (true, false), (false, true)] {
        let mut state = ring_state(floorplanned, flattened);
        let manifest = generate_manifest(&mut state);
        let mut sources = vec![("top.v", rtl_file(&manifest, "top.v").as_str())];
        if let Some(fsm) = manifest.files().get("rtl/top_fsm.v") {
            sources.push(("top_fsm.v", fsm));
        }
        sources.push(("worker.v", RING_WORKER));
        run_cosim(
            "top",
            &[],
            &[
                "frame_ring.v",
                "async_mmap.v",
                "detect_burst.v",
                "generate_last.v",
                "burst_split.v",
                "relay_station.v",
                "fifo.v",
                "fifo_fwd.v",
                "fifo_srl.v",
                "fifo_bram.v",
                "tapa_control.v",
                "tapa_hs_pipeline.v",
            ],
            &sources,
            RING_TESTBENCH,
        );
    }
}

/// A top whose task writes `n` elements, a frame of 4, to the top's output
/// ring `out`.
fn ring_state(floorplanned: bool, flattened: bool) -> tapa_codegen::rtl_state::TopologyWithRtl {
    use common::{design, parse_module, task};
    use serde_json::json;
    use std::collections::BTreeMap;
    use tapa_ir::{
        global_controller_instance_name, local_controller_instance_name, ControlChannel,
        FloorplanResult, PipelineRoute, PipelineScheme, RoutedChannel,
    };

    let top = task("top", "upper", |t| {
        t["ports"] = json!([
            {"cat": "scalar", "name": "n", "type": "uint32_t", "width": 32},
            {"cat": "ostream", "name": "out", "type": "uint32_t", "width": 32, "ring": 4}
        ]);
        t["tasks"] = json!({"worker": [{"name": "worker#0", "args": {
            "count": {"arg": "n", "cat": "scalar"},
            "o": {"arg": "out", "cat": "ostream"}
        }}]});
        t["fifos"] = json!({"out": {"produced_by": ["worker", 0]}});
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] = json!([
            {"cat": "scalar", "name": "count", "type": "uint32_t", "width": 32},
            {"cat": "ostream", "name": "o", "type": "uint32_t", "width": 32, "frame": 4}
        ]);
    });
    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(design(
        "top",
        "xilinx-vitis",
        &[("top", top), ("worker", worker)],
    ));
    state
        .attach_module(
            "top",
            parse_module(
                "module top(input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
                 output wire ap_done, output wire ap_idle, output wire ap_ready,\n\
                 input wire [31:0] n);\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module("worker", parse_module(RING_WORKER))
        .unwrap();
    if flattened {
        state.flat_projection =
            Some(tapa_ir::transforms::flatten_with_origins(&state.design).unwrap());
    }
    if floorplanned {
        let worker = "worker#0";
        let (central, remote) = ("SLOT_X0Y0".to_owned(), "SLOT_X1Y0".to_owned());
        let routes = [
            ControlChannel::Launch,
            ControlChannel::Reset,
            ControlChannel::Completion,
        ]
        .into_iter()
        .map(|channel| PipelineRoute {
            channel: RoutedChannel::Control {
                instance: worker.to_owned(),
                channel,
            },
            route: if channel == ControlChannel::Completion {
                vec![remote.clone(), central.clone()]
            } else {
                vec![central.clone(), remote.clone()]
            },
            scheme: PipelineScheme::Single,
            reg_regions: Vec::new(),
        })
        .collect();
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_owned(),
            grid: (2, 1),
            regions: BTreeMap::from([
                (global_controller_instance_name().to_owned(), central),
                (worker.to_owned(), remote.clone()),
                (local_controller_instance_name(worker), remote.clone()),
                ("out".to_owned(), remote),
            ]),
            routes,
            slot_usage: BTreeMap::new(),
        });
    }
    state
}

const RING_WORKER: &str = "
module worker(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
              output wire ap_done, output wire ap_idle, output wire ap_ready,
              input wire [31:0] count,
              output wire [32:0] o_s_din, input wire o_s_full_n, output wire o_s_write);
  reg busy = 0;
  reg finished = 0;
  reg [31:0] index = 0;
  assign o_s_write = busy && o_s_full_n;
  assign o_s_din = {1'b0, index};
  assign ap_done = finished;
  assign ap_ready = finished;
  assign ap_idle = !busy && !finished;
  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      busy <= 0; finished <= 0; index <= 0;
    end else begin
      finished <= 0;
      if (!busy && !finished && ap_start) begin
        busy <= 1; index <= 0;
      end else if (o_s_write) begin
        index <= index + 1;
        if (index + 1 == count) begin busy <= 0; finished <= 1; end
      end
    end
  end
endmodule
";

const RING_TESTBENCH: &str = r#"
#include "Vtop.h"
#include <cstdio>
#include <deque>
#include <memory>
// Two runs each write a frame; memory acknowledges a burst 50 cycles after
// its last beat. ap_done must come, and only after that acknowledgement.
int main() {
  auto d = std::make_unique<Vtop>();
  auto tick = [&]() { d->ap_clk = 1; d->eval(); d->ap_clk = 0; d->eval(); };
  d->ap_clk = 0; d->ap_rst_n = 0; d->ap_start = 0; d->n = 4;
  d->m_axi_out_AWREADY = 1; d->m_axi_out_WREADY = 1; d->m_axi_out_ARREADY = 1;
  for (int i = 0; i < 16; ++i) tick();
  d->ap_rst_n = 1;
  for (int i = 0; i < 16; ++i) tick();
  std::deque<long> responses;  // cycles at which each burst's B is due
  long cycle = 0;
  for (int run = 0; run < 2; ++run) {
    d->ap_start = 1;
    long acked = -1, done = -1;
    for (int i = 0; i < 1000 && done < 0; ++i, ++cycle) {
      d->m_axi_out_BVALID = !responses.empty() && responses.front() <= cycle;
      d->eval();
      if (d->m_axi_out_WVALID && d->m_axi_out_WLAST) responses.push_back(cycle + 50);
      if (d->m_axi_out_BVALID && d->m_axi_out_BREADY) {
        responses.pop_front();
        acked = cycle;
      }
      if (d->ap_done) done = cycle;
      if (d->ap_ready) d->ap_start = 0;
      tick();
    }
    if (done < 0 || acked < 0 || done <= acked) {
      std::printf("FAIL: run %d done at %ld, its frame acknowledged at %ld\n", run, done,
                  acked);
      return 1;
    }
  }
  std::printf("PASS\n");
  return 0;
}
"#;

/// Every control delivers each scalar at its child port's width, as a C++
/// argument converts to its parameter: a 64-bit parent wire bound to a
/// 32-bit and an 8-bit port, beside a 1-bit one, under the per-task FSM,
/// a floorplan's distributed controllers, and a flattened top's.
#[test]
fn every_control_delivers_scalars_at_the_ports_width() {
    use common::run_manifest::{generate_manifest, rtl_file};

    for (floorplanned, flattened) in [(false, false), (true, false), (false, true)] {
        let mut state = scalar_state(floorplanned, flattened);
        let manifest = generate_manifest(&mut state);
        let mut sources = vec![("top.v", rtl_file(&manifest, "top.v").as_str())];
        if let Some(fsm) = manifest.files().get("rtl/top_fsm.v") {
            sources.push(("top_fsm.v", fsm));
        }
        sources.extend([("worker.v", SCALAR_WORKER), ("harness.v", SCALAR_HARNESS)]);
        run_cosim(
            "scalar_harness",
            &[],
            &["tapa_control.v"],
            &sources,
            SCALAR_TESTBENCH,
        );
    }
}

fn scalar_state(floorplanned: bool, flattened: bool) -> tapa_codegen::rtl_state::TopologyWithRtl {
    use common::{design, parse_module, task};
    use serde_json::json;
    use std::collections::BTreeMap;
    use tapa_ir::{
        global_controller_instance_name, local_controller_instance_name, ControlChannel,
        FloorplanResult, PipelineRoute, PipelineScheme, RoutedChannel,
    };

    let top = task("top", "upper", |t| {
        t["ports"] = json!([
            {"cat": "scalar", "name": "n", "type": "uint64_t", "width": 64},
            {"cat": "scalar", "name": "flag", "type": "bool", "width": 1}
        ]);
        t["tasks"] = json!({"worker": [{"name": "worker#0", "args": {
            "count": {"arg": "n", "cat": "scalar"},
            "flag": {"arg": "flag", "cat": "scalar"},
            "tiny": {"arg": "n", "cat": "scalar"}
        }}]});
    });
    let worker = task("worker", "lower", |t| {
        t["ports"] = json!([
            {"cat": "scalar", "name": "count", "type": "uint32_t", "width": 32},
            {"cat": "scalar", "name": "flag", "type": "bool", "width": 1},
            {"cat": "scalar", "name": "tiny", "type": "uint8_t", "width": 8}
        ]);
    });
    let mut state = tapa_codegen::rtl_state::TopologyWithRtl::new(design(
        "top",
        "xilinx-hls",
        &[("top", top), ("worker", worker)],
    ));
    state
        .attach_module(
            "top",
            parse_module(
                "module top(input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
                 output wire ap_done, output wire ap_idle, output wire ap_ready,\n\
                 input wire [63:0] n, input wire flag);\nendmodule",
            ),
        )
        .unwrap();
    state
        .attach_module("worker", parse_module(SCALAR_WORKER))
        .unwrap();
    if flattened {
        state.flat_projection =
            Some(tapa_ir::transforms::flatten_with_origins(&state.design).unwrap());
    }
    if floorplanned {
        let worker = "worker#0";
        let (central, remote) = ("SLOT_X0Y0".to_owned(), "SLOT_X1Y0".to_owned());
        let routes = [
            ControlChannel::Launch,
            ControlChannel::Reset,
            ControlChannel::Completion,
        ]
        .into_iter()
        .map(|channel| PipelineRoute {
            channel: RoutedChannel::Control {
                instance: worker.to_owned(),
                channel,
            },
            route: if channel == ControlChannel::Completion {
                vec![remote.clone(), central.clone()]
            } else {
                vec![central.clone(), remote.clone()]
            },
            scheme: PipelineScheme::Single,
            reg_regions: Vec::new(),
        })
        .collect();
        state.floorplan = Some(FloorplanResult {
            device: "u280".to_owned(),
            grid: (2, 1),
            regions: BTreeMap::from([
                (global_controller_instance_name().to_owned(), central),
                (worker.to_owned(), remote.clone()),
                (local_controller_instance_name(worker), remote),
            ]),
            routes,
            slot_usage: BTreeMap::new(),
        });
    }
    state
}

/// A task that records the scalars it starts with.
const SCALAR_WORKER: &str = "
module worker(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
              output wire ap_done, output wire ap_idle, output wire ap_ready,
              input wire [31:0] count, input wire flag, input wire [7:0] tiny);
  reg [31:0] seen_count = 0;
  reg seen_flag = 0;
  reg [7:0] seen_tiny = 0;
  reg busy = 0;
  reg finished = 0;
  assign ap_done = finished;
  assign ap_ready = finished;
  assign ap_idle = !busy && !finished;
  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      busy <= 0; finished <= 0;
    end else begin
      finished <= 0;
      if (busy) begin
        busy <= 0; finished <= 1;
      end else if (ap_start && !finished) begin
        busy <= 1; seen_count <= count; seen_flag <= flag; seen_tiny <= tiny;
      end
    end
  end
endmodule
";

const SCALAR_HARNESS: &str = "
module scalar_harness(input wire ap_clk, input wire ap_rst_n, input wire ap_start,
                      input wire [63:0] n, input wire flag, output wire ap_done,
                      output wire [31:0] count, output wire seen_flag,
                      output wire [7:0] tiny);
  wire ap_idle, ap_ready;
  top dut(.ap_clk(ap_clk), .ap_rst_n(ap_rst_n), .ap_start(ap_start),
          .ap_done(ap_done), .ap_idle(ap_idle), .ap_ready(ap_ready), .n(n),
          .flag(flag));
  assign count = dut.worker_0.seen_count;
  assign seen_flag = dut.worker_0.seen_flag;
  assign tiny = dut.worker_0.seen_tiny;
endmodule
";

const SCALAR_TESTBENCH: &str = r#"
#include "Vscalar_harness.h"
#include <cstdio>
#include <memory>
int main() {
  auto d = std::make_unique<Vscalar_harness>();
  auto tick = [&]() { d->ap_clk = 1; d->eval(); d->ap_clk = 0; d->eval(); };
  d->ap_clk = 0; d->ap_rst_n = 0; d->ap_start = 0;
  d->n = 0x123456789AULL; d->flag = 1;
  for (int i = 0; i < 16; ++i) tick();
  d->ap_rst_n = 1;
  for (int i = 0; i < 16; ++i) tick();
  d->ap_start = 1;
  for (int i = 0; i < 200 && !d->ap_done; ++i) tick();
  if (!d->ap_done || d->count != 0x3456789Au || d->seen_flag != 1 || d->tiny != 0x9A) {
    std::printf("FAIL: done %d count 0x%x flag %d tiny 0x%x\n", (int)d->ap_done,
                (unsigned)d->count, (int)d->seen_flag, (unsigned)d->tiny);
    return 1;
  }
  std::printf("PASS\n");
  return 0;
}
"#;
