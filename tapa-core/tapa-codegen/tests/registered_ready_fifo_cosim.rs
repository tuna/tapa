//! Verilator cosim for the shallow floorplanned FIFO with registered ready.

mod common;

use common::verilator::run_cosim;

const TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vfifo_almost_full.h"
#include <cstdint>
#include <cstdio>
#include <deque>

static Vfifo_almost_full* dut;

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
    dut = new Vfifo_almost_full;
    dut->clk = 0;
    dut->if_write_ce = 1;
    dut->if_read_ce = 1;
    dut->if_write = 0;
    dut->if_read = 0;
    dut->if_din = 0;

    dut->reset = 1;
    for (int i = 0; i < 8; ++i) posedge();
    dut->reset = 0;
    for (int i = 0; i < 8; ++i) posedge();

    if (!dut->if_full_n || dut->if_empty_n) {
        std::printf("FAIL: invalid state after reset\n");
        return 1;
    }

    // Ready must not have a combinational path from producer or consumer
    // inputs. It can change only after a clock edge updates FIFO state.
    const bool ready_before_toggle = dut->if_full_n;
    dut->if_write = 1;
    dut->if_read = 1;
    dut->if_din = 0xdeadbeefu;
    dut->eval();
    if ((bool)dut->if_full_n != ready_before_toggle) {
        std::printf("FAIL: ready changed without a clock edge\n");
        return 2;
    }
    dut->if_write = 0;
    dut->if_read = 0;

    // Fill until registered backpressure arrives, then hold a long consumer
    // stall. DEPTH=13 is the generated storage for a logical depth of 8.
    std::deque<uint32_t> expected;
    uint32_t next_value = 0;
    for (int cycle = 0; cycle < 64 && dut->if_full_n; ++cycle) {
        dut->if_write = 1;
        dut->if_din = next_value;
        dut->eval();
        if (dut->if_full_n) {
            expected.push_back(next_value++);
        }
        posedge();
    }
    dut->if_write = 0;
    if (dut->if_full_n || expected.size() < 8 || expected.size() >= 13) {
        std::printf("FAIL: registered backpressure used %zu entries\n", expected.size());
        return 3;
    }

    const uint32_t stalled_front = dut->if_dout;
    for (int cycle = 0; cycle < 32; ++cycle) {
        posedge();
        if (!dut->if_empty_n || (uint32_t)dut->if_dout != stalled_front) {
            std::printf("FAIL: stalled output was not stable\n");
            return 4;
        }
    }

    while (!expected.empty()) {
        dut->if_read = 1;
        dut->eval();
        if (!dut->if_empty_n || (uint32_t)dut->if_dout != expected.front()) {
            std::printf("FAIL: fill/drain ordering mismatch\n");
            return 5;
        }
        expected.pop_front();
        posedge();
    }
    dut->if_read = 0;
    dut->eval();
    if (dut->if_empty_n) {
        std::printf("FAIL: FIFO remained nonempty after drain\n");
        return 6;
    }

    for (int cycle = 0; cycle < 16 && !dut->if_full_n; ++cycle) posedge();
    if (!dut->if_full_n) {
        std::printf("FAIL: ready did not recover after drain\n");
        return 7;
    }

    // Randomized traffic includes long backpressure bursts and simultaneous
    // transfers. The software queue is the protocol-level reference model.
    const uint32_t item_count = 500;
    uint32_t sent = 0;
    uint32_t received = 0;
    uint32_t random_state = 0x5a17c9e3u;
    for (int cycle = 0; cycle < 200000 && received < item_count; ++cycle) {
        const uint32_t random_value = next_random(&random_state);
        const bool write = sent < item_count && dut->if_full_n &&
                           ((random_value & 3u) != 0u);
        const bool read = dut->if_empty_n &&
                          (cycle % 47 >= 19) &&
                          (((random_value >> 8) & 3u) != 0u);

        dut->if_write = write;
        dut->if_din = sent;
        dut->if_read = read;
        dut->eval();

        if (read) {
            if (expected.empty() || (uint32_t)dut->if_dout != expected.front()) {
                std::printf("FAIL: randomized ordering mismatch at item %u\n", received);
                return 8;
            }
            expected.pop_front();
            ++received;
        }
        if (write) {
            expected.push_back(sent++);
        }
        posedge();
    }

    if (sent != item_count || received != item_count || !expected.empty()) {
        std::printf("FAIL: sent=%u received=%u queued=%zu\n",
                    sent, received, expected.size());
        return 9;
    }

    std::printf("PASS: registered ready preserved capacity and FIFO ordering\n");
    return 0;
}
"#;

#[test]
fn registered_ready_fifo_is_lossless_under_backpressure() {
    run_cosim(
        "fifo_almost_full",
        &[
            ("DATA_WIDTH", 32),
            ("ADDR_WIDTH", 4),
            ("DEPTH", 13),
            ("GRACE_PERIOD", 1),
        ],
        &["relay_station.v"],
        &[],
        TESTBENCH,
    );
}
