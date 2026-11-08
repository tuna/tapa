//! Verilator cosims for the AXI bursts `async_mmap` makes.
//!
//! An HLS loop that writes through an `async_mmap` pushes each element's
//! address together with its data. The write path must still merge such
//! element writes into whole AXI bursts: a user blocked on a full write
//! data buffer must not make the burst detector time out and close short
//! bursts, with the AXI slave taking a beat every cycle or slower.
//!
//! A burst request (`tapa::burst`) goes out as AXI bursts of up to 4 KB
//! without waiting to merge, but joins the requests that arrive back to
//! back with it, in order.

mod common;

use common::verilator::run_cosim;

/// RTL assets of an `async_mmap`.
const ASSETS: &[&str] = &[
    "async_mmap.v",
    "detect_burst.v",
    "generate_last.v",
    "burst_split.v",
    "relay_station.v",
    "fifo.v",
    "fifo_fwd.v",
    "fifo_srl.v",
    "fifo_bram.v",
];

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
        ASSETS,
        &[],
        TESTBENCH,
    );
}

const BURST_TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vasync_mmap.h"
#include <cstdint>
#include <cstdio>
#include <deque>
#include <map>
#include <utility>
#include <vector>

// 16-byte elements: 4 KB is 256 beats, element bursts stop at 1 KB (64).
// A request is {first element, count}; a count of 1 is an element request.
// `gap` is the idle cycles before it is pushed, 0 being back to back.
struct Request {
  uint32_t first, count;
  int gap;
};

static std::vector<Request> Requests() {
  std::vector<Request> r = {{0, 300, 0}};
  for (uint32_t i = 300; i < 310; ++i) r.push_back({i, 1, 0});
  r.push_back({310, 600, 0});
  r.push_back({910, 1, 0});
  r.push_back({911, 1, 0});
  for (uint32_t i = 1000; i < 1010; ++i) r.push_back({i, 1, 0});
  r.push_back({1010, 20, 0});
  for (uint32_t i = 4096; i < 4200; ++i) r.push_back({i, 1, 0});
  r.push_back({4200, 0, 0});
  r.push_back({5000, 4, 100});
  r.push_back({5004, 1, 1});
  return r;
}

// {first element, beats}. A burst request is cut at 4 KB boundaries;
// requests arriving back to back join its bursts up to 4 KB (256 at 256,
// 768 + 910 + 911), and an element run joins the burst request after it
// (1000 + 1010). Element runs alone stop at 1 KB (4096). An empty request
// makes no burst. A burst request alone goes out at once; a request a cycle
// later starts a new burst.
static const std::vector<std::pair<uint32_t, int>> kBursts = {
    {0, 256},    {256, 256},  {512, 256},  {768, 144}, {1000, 24},
    {1024, 6},   {4096, 64},  {4160, 40},  {5000, 4},  {5004, 1},
};

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  const std::vector<Request> requests = Requests();
  std::vector<uint32_t> elements;
  for (const auto& r : requests)
    for (uint32_t i = 0; i < r.count; ++i) elements.push_back(r.first + i);
  const int n = elements.size();

  Vasync_mmap* dut = new Vasync_mmap;
  dut->clk = 0;
  dut->rst = 1;
  for (int i = 0; i < 4; ++i) {
    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
  }
  dut->rst = 0;

  std::vector<std::pair<uint32_t, int>> aw, ar;
  std::map<uint32_t, long> pushed, aw_at, ar_at;  // by first element
  std::deque<int> w_left;                       // beats left of each AW burst
  std::deque<std::pair<uint32_t, int>> r_left;  // {next element, beats left}
  size_t req = 0;
  int wdata = 0, wbeats = 0, rdata = 0, responses = 0, acked = 0;
  long cycle = 0, next_push = 0;
  bool ok = true;
  while ((rdata < n || acked < n) && cycle < 20000) {
    // Both channels take the same requests in the same cycles.
    const bool push = req < requests.size() && cycle >= next_push &&
                      dut->write_addr_full_n && dut->read_addr_full_n;
    const Request& r = requests[push ? req : 0];
    dut->write_addr_din[0] = dut->read_addr_din[0] = r.first * 16;
    dut->write_addr_din[1] = dut->read_addr_din[1] = 0;
    dut->write_addr_din[2] = dut->read_addr_din[2] = r.count;
    dut->write_addr_write = dut->read_addr_write = push;
    const bool push_d = wdata < n && dut->write_data_full_n;
    for (int i = 0; i < 4; ++i) dut->write_data_din[i] = 0;
    dut->write_data_din[0] = push_d ? elements[wdata] : 0;
    dut->write_data_write = push_d;

    dut->m_axi_AWREADY = 1;
    dut->m_axi_ARREADY = 1;
    const bool wready = !w_left.empty();
    dut->m_axi_WREADY = wready;
    dut->m_axi_BVALID = responses > 0;
    dut->m_axi_BRESP = 0;
    dut->m_axi_BID = 0;
    dut->m_axi_RVALID = !r_left.empty();
    for (int i = 0; i < 4; ++i) dut->m_axi_RDATA[i] = 0;
    dut->m_axi_RDATA[0] = r_left.empty() ? 0 : r_left.front().first;
    dut->m_axi_RLAST = !r_left.empty() && r_left.front().second == 1;
    dut->m_axi_RID = 0;
    dut->m_axi_RRESP = 0;
    dut->read_data_read = 1;
    dut->write_resp_read = 1;
    dut->eval();

    if (dut->m_axi_AWVALID) {
      const uint32_t first = dut->m_axi_AWADDR / 16;
      aw.emplace_back(first, dut->m_axi_AWLEN + 1);
      aw_at[first] = cycle;
      w_left.push_back(dut->m_axi_AWLEN + 1);
    }
    if (dut->m_axi_ARVALID) {
      const uint32_t first = dut->m_axi_ARADDR / 16;
      ar.emplace_back(first, dut->m_axi_ARLEN + 1);
      ar_at[first] = cycle;
    }
    if (dut->m_axi_WVALID && wready) {
      const bool last = w_left.front() == 1;
      if (wbeats >= n || dut->m_axi_WDATA[0] != elements[wbeats] ||
          dut->m_axi_WLAST != last) {
        std::printf("W beat %d carries %u, last %d\n", wbeats,
                    (unsigned)dut->m_axi_WDATA[0], (int)dut->m_axi_WLAST);
        ok = false;
      }
      ++wbeats;
      if (--w_left.front() == 0) {
        w_left.pop_front();
        ++responses;
      }
    }
    if (dut->m_axi_RVALID && dut->m_axi_RREADY) {
      auto& beat = r_left.front();
      ++beat.first;
      if (--beat.second == 0) r_left.pop_front();
    }
    if (dut->m_axi_ARVALID) {
      r_left.emplace_back(dut->m_axi_ARADDR / 16, dut->m_axi_ARLEN + 1);
    }
    if (dut->m_axi_BVALID && dut->m_axi_BREADY) --responses;
    if (dut->write_resp_empty_n) acked += dut->write_resp_dout + 1;
    if (dut->read_data_empty_n) {
      if (rdata >= n || dut->read_data_dout[0] != elements[rdata]) {
        std::printf("read %d returns %u\n", rdata, (unsigned)dut->read_data_dout[0]);
        ok = false;
      }
      ++rdata;
    }
    if (push) {
      pushed.emplace(r.first, cycle);
      ++req;
      if (req < requests.size()) next_push = cycle + 1 + requests[req].gap;
    }
    if (push_d) ++wdata;

    dut->clk = 1; dut->eval();
    dut->clk = 0; dut->eval();
    ++cycle;
  }
  for (const auto* bursts : {&aw, &ar}) {
    if (*bursts != kBursts) {
      std::printf("%s bursts:", bursts == &aw ? "AW" : "AR");
      for (const auto& b : *bursts) std::printf(" {%u, %d}", b.first, b.second);
      std::printf("\n");
      ok = false;
    }
  }
  // A burst request does not wait for more requests.
  for (const uint32_t first : {0u, 5000u}) {
    const long aw_after = aw_at.count(first) ? aw_at[first] - pushed[first] : -1;
    const long ar_after = ar_at.count(first) ? ar_at[first] - pushed[first] : -1;
    if (aw_after < 0 || aw_after > 8 || ar_after < 0 || ar_after > 8) {
      std::printf("request at %u: AW %ld and AR %ld cycles after it\n", first,
                  aw_after, ar_after);
      ok = false;
    }
  }
  if (rdata != n || acked != n) {
    std::printf("%d reads and %d acknowledged writes of %d in %ld cycles\n",
                rdata, acked, n, cycle);
    ok = false;
  }
  delete dut;
  std::printf(ok ? "PASS\n" : "FAIL\n");
  return ok ? 0 : 1;
}
"#;

#[test]
fn burst_requests_go_out_at_once_and_merge_back_to_back() {
    run_cosim(
        "async_mmap",
        &[
            ("DataWidth", 128),
            ("DataWidthBytesLog", 4),
            ("WaitTimeWidth", 2),
            ("MaxWaitTime", 3),
            ("BurstLenWidth", 8),
            ("MaxBurstLen", 63),
            ("ReqLenWidth", 32),
        ],
        ASSETS,
        &[],
        BURST_TESTBENCH,
    );
}
