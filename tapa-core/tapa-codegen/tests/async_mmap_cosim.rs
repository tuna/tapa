//! Verilator cosim for `async_mmap`'s write bursts.
//!
//! An HLS loop that writes through an `async_mmap` pushes each element's
//! address together with its data. The write path must still merge such
//! element writes into whole AXI bursts: a user blocked on a full write
//! data buffer must not make the burst detector time out and close short
//! bursts, with the AXI slave taking a beat every cycle or slower.

mod common;

use common::verilator::run_cosim;

const TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vasync_mmap.h"
#include <cstdint>
#include <cstdio>
#include <deque>

// Write N elements of 4 bytes from address 0, each address pushed with its
// data, while the slave takes a W beat every `every` cycles. Every burst is
// whole (256 beats), in order, and ends in a write response.
static bool run(int every) {
  const int n = 4096;
  const int beats_per_burst = 256;
  Vasync_mmap* dut = new Vasync_mmap;
  dut->clk = 0;
  dut->rst = 1;
  for (int i = 0; i < 4; ++i) {
    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
  }
  dut->rst = 0;

  std::deque<int> bursts;  // beats left of each accepted burst
  int responses = 0;       // write responses the slave owes
  int pushed = 0, beats = 0, accepted = 0, acked = 0;
  long cycle = 0;
  bool ok = true;
  while (acked < n && cycle < 100000) {
    const bool push = pushed < n && dut->write_addr_full_n && dut->write_data_full_n;
    dut->write_addr_din = static_cast<uint64_t>(pushed) * 4;
    dut->write_data_din = pushed;
    dut->write_addr_write = push;
    dut->write_data_write = push;
    dut->m_axi_AWREADY = 1;
    const bool wready = !bursts.empty() && cycle % every == 0;
    dut->m_axi_WREADY = wready;
    dut->m_axi_BVALID = responses > 0;
    dut->m_axi_BRESP = 0;
    dut->m_axi_BID = 0;
    dut->write_resp_read = 1;
    dut->eval();

    if (dut->m_axi_AWVALID) {
      const int len = dut->m_axi_AWLEN + 1;
      const uint64_t addr = static_cast<uint64_t>(accepted) * beats_per_burst * 4;
      if (len != beats_per_burst || dut->m_axi_AWADDR != addr) {
        std::printf("every %d: burst %d of %d beats at 0x%llx, expected %d at 0x%llx\n",
                    every, accepted, len, (unsigned long long)dut->m_axi_AWADDR,
                    beats_per_burst, (unsigned long long)addr);
        ok = false;
      }
      bursts.push_back(len);
      ++accepted;
    }
    if (dut->m_axi_WVALID && wready) {
      const bool last = bursts.front() == 1;
      if (dut->m_axi_WDATA != static_cast<uint32_t>(beats) || dut->m_axi_WLAST != last) {
        std::printf("every %d: beat %d carries %u, last %d\n", every, beats,
                    (unsigned)dut->m_axi_WDATA, (int)dut->m_axi_WLAST);
        ok = false;
      }
      ++beats;
      if (--bursts.front() == 0) {
        bursts.pop_front();
        ++responses;
      }
    }
    if (dut->m_axi_BVALID && dut->m_axi_BREADY) --responses;
    if (dut->write_resp_empty_n) acked += dut->write_resp_dout + 1;
    if (push) ++pushed;

    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
    ++cycle;
  }
  // The slave's pace and the first burst's 256 addresses bound the run.
  const long budget = static_cast<long>(n) * every + 2 * beats_per_burst;
  if (acked != n || cycle > budget) {
    std::printf("every %d: %d of %d elements acknowledged in %ld cycles (budget %ld)\n",
                every, acked, n, cycle, budget);
    ok = false;
  }
  delete dut;
  return ok;
}

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  const bool ok = run(1) && run(8);
  std::printf(ok ? "PASS\n" : "FAIL\n");
  return ok ? 0 : 1;
}
"#;

#[test]
fn element_writes_pushed_with_their_data_make_whole_bursts() {
    run_cosim(
        "async_mmap",
        &[
            ("DataWidth", 32),
            ("DataWidthBytesLog", 2),
            ("WaitTimeWidth", 2),
            ("MaxWaitTime", 3),
            ("BurstLenWidth", 8),
            ("MaxBurstLen", 255),
            ("EnableReadChannel", 0),
        ],
        &[
            "async_mmap.v",
            "detect_burst.v",
            "generate_last.v",
            "burst_expand.v",
            "relay_station.v",
            "fifo.v",
            "fifo_fwd.v",
            "fifo_srl.v",
            "fifo_bram.v",
        ],
        &[],
        TESTBENCH,
    );
}
