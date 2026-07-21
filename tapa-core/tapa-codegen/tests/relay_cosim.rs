//! Verilator cosim: the `relay_station` primitive must be functionally
//! equivalent to a plain FIFO — it may add latency, but under heavy
//! backpressure (delayed `full_n`) it must neither drop nor reorder data.
//!
//! This is the P3 correctness gate for relay insertion. The test skips cleanly
//! when `verilator` is not on `PATH`.

use std::path::Path;
use std::process::Command;

/// C++ driver: write as fast as `full_n` allows, read one cycle in three (so
/// the buffer fills and the grace period is exercised), and assert every
/// written value comes back exactly once, in order. Exits non-zero on mismatch.
const TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vrelay_station.h"
#include <vector>
#include <cstdio>

static Vrelay_station* dut;

static void posedge() {
    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
}

int main(int argc, char** argv) {
    Verilated::commandArgs(argc, argv);
    dut = new Vrelay_station;
    dut->clk = 0;
    dut->if_write_ce = 1;
    dut->if_read_ce = 1;
    dut->if_write = 0;
    dut->if_read = 0;
    dut->if_din = 0;

    // Active-high reset (tapa's ap_rst).
    dut->reset = 1;
    for (int i = 0; i < 16; i++) posedge();
    dut->reset = 0;
    dut->eval();

    // Fill without reads until registered backpressure reaches the producer,
    // hold that full state, then drain. This directly exercises the Tail's
    // retimed SRL address across both pointer directions.
    const int FILL_LIMIT = 512;
    int filled = 0;
    while (filled < FILL_LIMIT && dut->if_full_n) {
        dut->if_write = 1;
        dut->if_din = (unsigned)filled;
        dut->if_read = 0;
        dut->eval();
        posedge();
        filled++;
    }
    dut->if_write = 0;
    dut->eval();
    if (filled == 0 || filled == FILL_LIMIT || dut->if_full_n) {
        printf("FAIL: backpressure did not stop the deterministic fill (%d)\n", filled);
        return 10;
    }
    if (!dut->if_empty_n) {
        printf("FAIL: filled relay reported empty\n");
        return 11;
    }
    const unsigned held_dout = (unsigned)dut->if_dout;
    for (int i = 0; i < 32; i++) {
        if (dut->if_full_n || !dut->if_empty_n ||
            (unsigned)dut->if_dout != held_dout) {
            printf("FAIL: full relay was not stable under prolonged backpressure\n");
            return 12;
        }
        posedge();
    }

    std::vector<unsigned> fill_got;
    for (long cyc = 0; cyc < 20000 && (int)fill_got.size() < filled; cyc++) {
        int do_read = dut->if_empty_n;
        dut->if_read = do_read;
        dut->eval();
        if (do_read) fill_got.push_back((unsigned)dut->if_dout);
        posedge();
    }
    if ((int)fill_got.size() != filled) {
        printf("FAIL: drained %d of %d deterministically filled items\n",
               (int)fill_got.size(), filled);
        return 13;
    }
    for (int i = 0; i < filled; i++) {
        if (fill_got[i] != (unsigned)i) {
            printf("FAIL: fill_got[%d]=%u expected %d\n", i, fill_got[i], i);
            return 14;
        }
    }

    // Start the long mixed-traffic phase from an independently empty state.
    dut->if_read = 0;
    dut->reset = 1;
    for (int i = 0; i < 16; i++) posedge();
    dut->reset = 0;
    dut->eval();

    const int N = 500;
    std::vector<unsigned> got;
    int next_write = 0;
    int read_phase = 0;

    for (long cyc = 0; cyc < 200000 && (int)got.size() < N; cyc++) {
        int do_write = (next_write < N) && dut->if_full_n;
        int do_read  = dut->if_empty_n && (read_phase == 0);

        dut->if_write = do_write;
        dut->if_din   = do_write ? (unsigned)next_write : 0u;
        dut->if_read  = do_read;
        dut->eval();

        if (do_read)  got.push_back((unsigned)dut->if_dout);
        if (do_write) next_write++;
        read_phase = (read_phase + 1) % 3;

        posedge();
    }

    if ((int)got.size() != N) {
        printf("FAIL: relayed %d of %d items (deadlock or loss)\n", (int)got.size(), N);
        return 1;
    }
    for (int i = 0; i < N; i++) {
        if (got[i] != (unsigned)i) {
            printf("FAIL: got[%d]=%u expected %d (reorder or corruption)\n", i, got[i], i);
            return 2;
        }
    }
    printf("PASS: %d items relayed in order under backpressure\n", N);
    return 0;
}
"#;

fn verilator_available() -> bool {
    Command::new("verilator")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Retrieve the embedded `relay_station.v` asset source.
fn relay_station_source() -> Vec<u8> {
    let asset = tapa_codegen::support_assets::VerilogAssets::get("relay_station.v")
        .expect("relay_station.v is an embedded asset");
    asset.data.into_owned()
}

#[test]
fn srl_tail_address_is_retimed_and_fanout_limited() {
    let source = String::from_utf8(relay_station_source()).expect("relay RTL is UTF-8");
    assert!(source.contains("(* max_fanout = 128 *) reg [REAL_ADDR_WIDTH - 1:0] shiftReg_addr;"));
    assert!(source.contains("shiftReg_addr <= mOutPtrMinusOne[REAL_ADDR_WIDTH]"));
    assert!(source.contains("shiftReg_addr <= mOutPtrPlusOne[REAL_ADDR_WIDTH]"));
    assert!(!source.contains("assign shiftReg_addr = mOutPtr[REAL_ADDR_WIDTH]"));
}

#[test]
fn relay_station_is_functionally_a_fifo() {
    if !verilator_available() {
        eprintln!("skipping relay_station cosim: `verilator` not found on PATH");
        return;
    }
    // A shallow and a deep pipeline: the grace-depth math scales with LEVEL, so
    // both must relay the stream losslessly under backpressure.
    for level in [2u32, 8u32] {
        run_cosim(level);
    }
}

/// Build and run the `relay_station` testbench with `LEVEL` overridden,
/// asserting the stream survives intact.
fn run_cosim(level: u32) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("relay_station.v"), relay_station_source()).expect("write rtl");
    std::fs::write(root.join("tb.cpp"), TESTBENCH).expect("write tb");

    // Build the simulator. Width mismatches in the vendored RTL are benign.
    let build = Command::new("verilator")
        .current_dir(root)
        .args([
            "--cc",
            "--exe",
            "--build",
            "--top-module",
            "relay_station",
            &format!("-GLEVEL={level}"),
            "-Wno-WIDTH",
            "-Wno-UNOPTFLAT",
            "-Wno-CASEINCOMPLETE",
            "-Wno-fatal",
            "--Mdir",
            "obj_dir",
            "-o",
            "sim",
            "relay_station.v",
            "tb.cpp",
        ])
        .output()
        .expect("spawn verilator");
    assert!(
        build.status.success(),
        "verilator build failed (LEVEL={level}):\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let sim = obj_dir_binary(root);
    let run = Command::new(&sim).output().expect("run simulator");
    assert!(
        run.status.success() && String::from_utf8_lossy(&run.stdout).contains("PASS"),
        "relay_station (LEVEL={level}) is NOT equivalent to a fifo:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}

/// Verilator writes the executable as `<root>/obj_dir/sim`.
fn obj_dir_binary(root: &Path) -> std::path::PathBuf {
    root.join("obj_dir").join("sim")
}
