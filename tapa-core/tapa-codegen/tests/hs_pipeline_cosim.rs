//! Verilator cosim for the floorplanned Head/Body/Tail handshake pipeline.
//!
//! In particular this exercises `BODY_LEVEL=0`: adjacent Single and
//! Single-H/Double-V crossings must retain FIFO storage plus the Head and Tail
//! timing cells, rather than becoming a combinational wire.

mod common;

use common::verilator::run_cosim;

const TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vtapa_hs_pipeline.h"
#include <cstdint>
#include <vector>
#include <cstdio>

static Vtapa_hs_pipeline* dut;

static void posedge() {
    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
}

static uint32_t next_random(uint32_t* state) {
    uint32_t value = *state;
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    *state = value;
    return value;
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
    for (int i = 0; i < 128 && !dut->if_full_n; i++) posedge();
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

    // Head and Body deliberately omit reset to avoid a high-fanout reset net.
    // The reset source holds the resettable Tail long enough for all fixed-
    // latency in-flight state to drain into it. Exercise that contract with
    // traffic active on the assertion edge.
    const unsigned RESET_POISON = 0xdeadbeefu;
    dut->reset = 1;
    dut->if_write = 1;
    dut->if_din = RESET_POISON;
    dut->if_read = 1;
    posedge();
    dut->if_write = 0;
    dut->if_din = 0;
    dut->if_read = 0;
    for (int i = 0; i < 16; i++) posedge();
    dut->reset = 0;
    dut->eval();
    for (int i = 0; i < 128; i++) {
        if (dut->if_empty_n) {
            printf("FAIL: Head retained traffic across reset\n");
            return 6;
        }
        posedge();
    }
    if (!dut->if_full_n) {
        printf("FAIL: pipeline did not become ready after traffic reset\n");
        return 7;
    }

    // Put an item beyond Head before asserting reset with idle inputs. For
    // nonzero BODY_LEVEL this verifies that the held Tail reset drains every
    // resetless Body valid/data register before the next invocation.
    const unsigned BODY_POISON = 0xc001d00du;
    dut->if_write = 1;
    dut->if_din = BODY_POISON;
    dut->eval();
    posedge();
    dut->if_write = 0;
    dut->if_din = 0;
    dut->eval();
    posedge();
    dut->reset = 1;
    for (int i = 0; i < 16; i++) posedge();
    dut->reset = 0;
    dut->eval();
    for (int i = 0; i < 128; i++) {
        if (dut->if_empty_n) {
            printf("FAIL: Body retained traffic across reset\n");
            return 8;
        }
        posedge();
    }
    if (!dut->if_full_n) {
        printf("FAIL: pipeline did not become ready after in-flight reset\n");
        return 9;
    }

    const int N = 500;
    std::vector<unsigned> got;
    int next_write = 0;
    uint32_t random_state = 0x31415926u;

    for (long cyc = 0; cyc < 200000 && (int)got.size() < N; cyc++) {
        uint32_t random_value = next_random(&random_state);
        int do_write = (next_write < N) && dut->if_full_n &&
                       ((random_value & 3u) != 0u);
        int do_read  = dut->if_empty_n &&
                       (((random_value >> 8) & 3u) == 0u);

        dut->if_write = do_write;
        dut->if_din = do_write ? (unsigned)next_write : 0u;
        dut->if_read = do_read;
        dut->eval();

        if (do_read) got.push_back((unsigned)dut->if_dout);
        if (do_write) next_write++;
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
    printf("PASS: reset flushed traffic and %d randomized items relayed in order\n", N);
    return 0;
}
"#;

#[test]
fn head_body_tail_pipeline_is_lossless_with_and_without_body_cells() {
    for body_level in [0u32, 2u32, 8u32] {
        run_cosim(
            "tapa_hs_pipeline",
            &[("BODY_LEVEL", body_level)],
            &["relay_station.v", "tapa_hs_pipeline.v"],
            &[],
            TESTBENCH,
        );
    }
}
