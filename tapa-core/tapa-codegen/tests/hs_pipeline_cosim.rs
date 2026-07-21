//! Verilator cosim for the floorplanned Head/Body/Tail handshake pipeline.
//!
//! In particular this exercises `BODY_LEVEL=0`: adjacent Single and
//! Single-H/Double-V crossings must retain FIFO storage plus the Head and Tail
//! timing cells, rather than becoming a combinational wire.

use std::path::Path;
use std::process::Command;

const TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vtapa_hs_pipeline.h"
#include <vector>
#include <cstdio>

static Vtapa_hs_pipeline* dut;

static void posedge() {
    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
}

int main(int argc, char** argv) {
    Verilated::commandArgs(argc, argv);
    dut = new Vtapa_hs_pipeline;
    dut->clk = 0;
    dut->if_write_ce = 1;
    dut->if_read_ce = 1;
    dut->if_write = 0;
    dut->if_read = 0;
    dut->if_din = 0;

    dut->reset = 1;
    for (int i = 0; i < 16; i++) posedge();
    dut->reset = 0;
    for (int i = 0; i < 8; i++) posedge();
    dut->eval();

    // A one-cycle pulse must spend an edge in Head before it can reach Tail.
    // This catches both a combinational-valid shortcut (visible too early) and
    // a combinational-data shortcut (the pulse later carries the cleared 0).
    const unsigned MARKER = 0x5a5a1234u;
    if (!dut->if_full_n) {
        printf("FAIL: pipeline did not become ready after reset\n");
        return 3;
    }
    dut->if_write = 1;
    dut->if_din = MARKER;
    dut->eval();
    posedge();
    dut->if_write = 0;
    dut->if_din = 0;
    dut->eval();
    if (dut->if_empty_n) {
        printf("FAIL: Head valid/data bypassed its register\n");
        return 4;
    }
    for (int i = 0; i < 100 && !dut->if_empty_n; i++) posedge();
    if (!dut->if_empty_n || (unsigned)dut->if_dout != MARKER) {
        printf("FAIL: Head did not preserve registered valid/data\n");
        return 5;
    }

    // Clear the directed probe before the sustained backpressure test.
    dut->reset = 1;
    for (int i = 0; i < 16; i++) posedge();
    dut->reset = 0;
    for (int i = 0; i < 8; i++) posedge();
    dut->eval();

    const int N = 500;
    std::vector<unsigned> got;
    int next_write = 0;
    int read_phase = 0;

    for (long cyc = 0; cyc < 200000 && (int)got.size() < N; cyc++) {
        int do_write = (next_write < N) && dut->if_full_n;
        int do_read  = dut->if_empty_n && (read_phase == 0);

        dut->if_write = do_write;
        dut->if_din = do_write ? (unsigned)next_write : 0u;
        dut->if_read = do_read;
        dut->eval();

        if (do_read) got.push_back((unsigned)dut->if_dout);
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
        .is_ok_and(|output| output.status.success())
}

fn asset_source(name: &str) -> Vec<u8> {
    tapa_codegen::support_assets::VerilogAssets::get(name)
        .unwrap_or_else(|| panic!("{name} is an embedded asset"))
        .data
        .into_owned()
}

#[test]
fn head_body_tail_pipeline_is_lossless_with_and_without_body_cells() {
    if !verilator_available() {
        eprintln!("skipping handshake-pipeline cosim: `verilator` not found on PATH");
        return;
    }

    for body_level in [0u32, 2u32, 8u32] {
        run_cosim(body_level);
    }
}

fn run_cosim(body_level: u32) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(
        root.join("relay_station.v"),
        asset_source("relay_station.v"),
    )
    .expect("write Tail FIFO RTL");
    std::fs::write(
        root.join("tapa_hs_pipeline.v"),
        asset_source("tapa_hs_pipeline.v"),
    )
    .expect("write pipeline RTL");
    std::fs::write(root.join("tb.cpp"), TESTBENCH).expect("write testbench");

    let build = Command::new("verilator")
        .current_dir(root)
        .args([
            "--cc",
            "--exe",
            "--build",
            "--top-module",
            "tapa_hs_pipeline",
            &format!("-GBODY_LEVEL={body_level}"),
            "-Wno-WIDTH",
            "-Wno-UNOPTFLAT",
            "-Wno-CASEINCOMPLETE",
            "-Wno-fatal",
            "--Mdir",
            "obj_dir",
            "-o",
            "sim",
            "relay_station.v",
            "tapa_hs_pipeline.v",
            "tb.cpp",
        ])
        .output()
        .expect("spawn verilator");
    assert!(
        build.status.success(),
        "verilator build failed (BODY_LEVEL={body_level}):\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let run = Command::new(obj_dir_binary(root))
        .output()
        .expect("run simulator");
    assert!(
        run.status.success() && String::from_utf8_lossy(&run.stdout).contains("PASS"),
        "handshake pipeline (BODY_LEVEL={body_level}) is not lossless:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}

fn obj_dir_binary(root: &Path) -> std::path::PathBuf {
    root.join("obj_dir").join("sim")
}
