//! Verilator protocol test for distributed control and feed-forward pipelines.

use std::path::Path;
use std::process::Command;

const HARNESS: &str = "
`default_nettype none
module control_harness (
  input  wire       ap_clk,
  input  wire       ap_rst_n,
  input  wire       ap_start,
  input  wire       child_done,
  input  wire       child_ready,
  input  wire [7:0] probe_in,
  output wire [7:0] probe_out,
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

    dut->ap_rst_n = 0;
    posedge();
    dut->ap_rst_n = 1;
    dut->ap_start = 1;
    dut->eval();
    if (dut->fabric_reset_n) {
        std::printf("FAIL: parent fabric left reset before the flush guard\n");
        return 8;
    }

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

fn verilator_available() -> bool {
    Command::new("verilator")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn distributed_control_survives_reset_and_consecutive_launches() {
    if !verilator_available() {
        eprintln!("skipping distributed-control cosim: `verilator` not found on PATH");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let asset = tapa_codegen::support_assets::VerilogAssets::get("tapa_control.v")
        .expect("embedded control RTL");
    std::fs::write(root.join("tapa_control.v"), &asset.data).expect("write control RTL");
    std::fs::write(root.join("harness.v"), HARNESS).expect("write harness");
    std::fs::write(root.join("tb.cpp"), TESTBENCH).expect("write testbench");

    let build = Command::new("verilator")
        .current_dir(root)
        .args([
            "--cc",
            "--exe",
            "--build",
            "--top-module",
            "control_harness",
            "-Wno-WIDTH",
            "-Wno-UNUSEDSIGNAL",
            "-Wno-fatal",
            "--Mdir",
            "obj_dir",
            "-o",
            "sim",
            "tapa_control.v",
            "harness.v",
            "tb.cpp",
        ])
        .output()
        .expect("spawn verilator");
    assert!(
        build.status.success(),
        "verilator build failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let run = Command::new(obj_dir_binary(root))
        .output()
        .expect("run control simulator");
    assert!(
        run.status.success() && String::from_utf8_lossy(&run.stdout).contains("PASS"),
        "distributed-control protocol failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}

fn obj_dir_binary(root: &Path) -> std::path::PathBuf {
    root.join("obj_dir").join("sim")
}
