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
//!
//! Under random requests and random stalls on every channel, the bridge
//! keeps the AXI rules and puts every element where its request points.

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

/// Every request and stall pattern an AXI slave and a user may present: an
/// in-order memory model takes and returns beats with random stalls on all
/// five AXI channels (W beats ahead of their AW or not), the user pushes
/// element and burst requests, some empty, around 4 KiB pages and 4 GiB
/// boundaries, and pops data and responses with random stalls. Each beat
/// lands where its request points, no burst crosses 4 KiB or outgrows
/// AWLEN/ARLEN, WLAST ends each burst, VALID holds its payload until READY,
/// and the write responses acknowledge each element once its burst's B is in.
const STRESS_TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vasync_mmap.h"
#include <cstdint>
#include <cstdio>
#include <deque>
#include <map>
#include <random>
#include <type_traits>
#include <vector>

static const int kBytes = DW / 8;
static const int kWords = (DW + 31) / 32;
typedef std::vector<uint32_t> Val;

template <typename T>
static typename std::enable_if<std::is_integral<T>::value>::type set(T& p, const Val& v) {
  uint64_t x = v[0];
  if (v.size() > 1 && sizeof(T) > 4) x |= uint64_t(v[1]) << 32;
  p = (T)x;
}
template <std::size_t N>
static void set(VlWide<N>& p, const Val& v) {
  for (std::size_t i = 0; i < N; ++i) p[i] = i < v.size() ? v[i] : 0;
}
template <typename T>
static typename std::enable_if<std::is_integral<T>::value, Val>::type get(const T& p) {
  Val v(kWords);
  v[0] = (uint32_t)p;
  if (kWords > 1) v[1] = (uint32_t)((uint64_t)p >> 32);
  return v;
}
template <std::size_t N>
static Val get(const VlWide<N>& p) {
  Val v(kWords);
  for (int i = 0; i < kWords; ++i) v[i] = p[i];
  return v;
}

static std::mt19937_64 rng;
static int rnd(int n) { return (int)(rng() % (uint64_t)n); }
static bool chance(int pct) { return rnd(100) < pct; }
static Val masked(Val v) {
  if (DW % 32) v[kWords - 1] &= (1u << (DW % 32)) - 1;
  return v;
}
static Val random_val() {
  Val v(kWords);
  for (auto& w : v) w = (uint32_t)rng();
  return masked(v);
}
// What memory holds at `addr` before any write.
static Val initial(uint64_t addr) {
  std::mt19937_64 r(addr);
  Val v(kWords);
  for (auto& w : v) w = (uint32_t)r();
  return masked(v);
}

struct Req { uint64_t addr; uint32_t count; };
struct Burst { uint64_t addr; int beats; int done; };

// Runs of contiguous requests, requests ending just below a 4 KiB page,
// and scattered ones, from `base`.
static std::vector<Req> requests(uint64_t base, int n) {
  std::vector<Req> r;
  uint64_t next = base;
  for (int i = 0; i < n; ++i) {
    const int k = rnd(10);
    uint64_t addr = k < 5 ? next
                  : k < 7 ? (base & ~0xFFFULL) + 4096 * rnd(8) - kBytes * rnd(4)
                          : base + (uint64_t)kBytes * rnd(4096);
    addr &= ~(uint64_t)(kBytes - 1);
    uint32_t count = 1;
    if (RLW > 0 && chance(60)) {
      const int m = rnd(10);
      count = m == 0 ? 0 : m < 4 ? rnd(8) + 1 : m < 8 ? rnd(300) + 1 : rnd(1500) + 1;
    }
    r.push_back({addr, count});
    next = addr + (uint64_t)kBytes * count;
  }
  return r;
}

static bool run(uint64_t seed) {
  rng.seed(seed);
  const int p_aw = 20 + rnd(81), p_w = 20 + rnd(81), p_ar = 20 + rnd(81), p_r = 20 + rnd(81);
  const int p_req = 10 + rnd(91), p_data = 10 + rnd(91), p_pop = 10 + rnd(91), p_ack = 10 + rnd(91);
  const bool w_before_aw = chance(50);
  const bool coupled = RLW == 0 && chance(50);  // each address pushed with its data
  const int b_delay = rnd(20);
  const uint64_t rbase = chance(50) ? 0xFFFF0000ULL : 0x10000ULL;   // 4 GiB below
  const uint64_t wbase = chance(50) ? 0x1FFFF0000ULL : 0x7FFF8000ULL;
  const std::vector<Req> rreq = requests(rbase, 60 + rnd(100));
  const std::vector<Req> wreq = requests(wbase, 60 + rnd(100));

  std::deque<uint64_t> rexpect;
  for (const Req& q : rreq)
    for (uint32_t i = 0; i < q.count; ++i) rexpect.push_back(q.addr + (uint64_t)i * kBytes);
  std::vector<std::pair<uint64_t, Val>> wdata;
  std::map<uint64_t, Val> mem, golden;
  for (const Req& q : wreq)
    for (uint32_t i = 0; i < q.count; ++i) {
      wdata.push_back({q.addr + (uint64_t)i * kBytes, random_val()});
      golden[wdata.back().first] = wdata.back().second;
    }
  const size_t nr = rexpect.size(), nw = wdata.size();

  Vasync_mmap* dut = new Vasync_mmap;
  auto tick = [&]() { dut->clk = 1; dut->eval(); dut->clk = 0; dut->eval(); };
  dut->clk = 0; dut->rst = 1;
  for (int i = 0; i < 4; ++i) tick();
  dut->rst = 0;

  size_t ri = 0, wi = 0, wdi = 0, rgot = 0;
  uint64_t acked = 0, b_beats = 0;
  std::deque<Burst> aw, ar;
  std::deque<std::pair<Val, int>> w;  // accepted W beats and their WLAST
  std::deque<std::pair<long, int>> b; // when each B is due, and its beats
  bool rvalid = false, held_aw = false, held_ar = false, held_w = false;
  uint64_t aw_addr = 0, ar_addr = 0;
  int aw_len = 0, ar_len = 0, w_last = 0;
  Val w_data;
  int errors = 0, settle = 0;
  long cycle = 0;
  auto fail = [&](const char* what) {
    if (errors++ < 5) std::printf("seed %llu cycle %ld: %s\n", (unsigned long long)seed, cycle, what);
  };
  auto check_burst = [&](uint64_t addr, int len, int size, int burst) {
    if (size != __builtin_ctz(kBytes) || burst != 1 || addr % kBytes) fail("bad burst");
    if ((addr & 0xFFF) + (uint64_t)(len + 1) * kBytes > 4096) fail("burst crosses 4 KiB");
  };
  for (; cycle < 1000000 && settle < 100; ++cycle) {
    // User side.
    const bool push_ra = ri < rreq.size() && chance(p_req);
    bool push_wa, push_wd;
    if (coupled) {
      push_wa = push_wd = wi < wreq.size() && chance(p_req) && dut->write_addr_full_n &&
                          dut->write_data_full_n;
    } else {
      push_wa = wi < wreq.size() && chance(p_req);
      push_wd = wdi < nw && chance(p_data);
    }
    const Req& r = push_ra ? rreq[ri] : Req{0, 0};
    set(dut->read_addr_din, Val{(uint32_t)r.addr, (uint32_t)(r.addr >> 32), r.count});
    dut->read_addr_write = push_ra;
    const Req& q = push_wa ? wreq[wi] : Req{0, 0};
    set(dut->write_addr_din, Val{(uint32_t)q.addr, (uint32_t)(q.addr >> 32), q.count});
    dut->write_addr_write = push_wa;
    set(dut->write_data_din, push_wd ? wdata[wdi].second : Val(kWords, 0));
    dut->write_data_write = push_wd;
    const bool pop = chance(p_pop), ack = chance(p_ack);
    dut->read_data_read = pop;
    dut->write_resp_read = ack;
    // Slave side.
    const bool awready = chance(p_aw), arready = chance(p_ar);
    const bool wready = (w_before_aw || !aw.empty()) && chance(p_w);
    const bool bvalid = !b.empty() && b.front().first <= cycle;
    if (!rvalid && !ar.empty() && chance(p_r)) rvalid = true;
    dut->m_axi_AWREADY = awready;
    dut->m_axi_ARREADY = arready;
    dut->m_axi_WREADY = wready;
    dut->m_axi_BVALID = bvalid;
    dut->m_axi_BRESP = 0;
    dut->m_axi_BID = 0;
    dut->m_axi_RVALID = rvalid;
    dut->m_axi_RID = 0;
    dut->m_axi_RRESP = 0;
    if (rvalid) {
      const uint64_t a = ar.front().addr + (uint64_t)ar.front().done * kBytes;
      set(dut->m_axi_RDATA, mem.count(a) ? mem[a] : initial(a));
      dut->m_axi_RLAST = ar.front().done == ar.front().beats - 1;
    }
    dut->eval();

    // A VALID not yet taken holds itself and its payload.
    const Val wd = get(dut->m_axi_WDATA);
    if (held_aw && (!dut->m_axi_AWVALID || dut->m_axi_AWADDR != aw_addr || dut->m_axi_AWLEN != aw_len))
      fail("AW changed before AWREADY");
    if (held_ar && (!dut->m_axi_ARVALID || dut->m_axi_ARADDR != ar_addr || dut->m_axi_ARLEN != ar_len))
      fail("AR changed before ARREADY");
    if (held_w && (!dut->m_axi_WVALID || wd != w_data || dut->m_axi_WLAST != w_last))
      fail("W changed before WREADY");
    held_aw = dut->m_axi_AWVALID && !awready;
    held_ar = dut->m_axi_ARVALID && !arready;
    held_w = dut->m_axi_WVALID && !wready;
    aw_addr = dut->m_axi_AWADDR; aw_len = dut->m_axi_AWLEN;
    ar_addr = dut->m_axi_ARADDR; ar_len = dut->m_axi_ARLEN;
    w_data = wd; w_last = dut->m_axi_WLAST;

    if (dut->m_axi_AWVALID && awready) {
      check_burst(dut->m_axi_AWADDR, dut->m_axi_AWLEN, dut->m_axi_AWSIZE, dut->m_axi_AWBURST);
      aw.push_back({dut->m_axi_AWADDR, dut->m_axi_AWLEN + 1, 0});
    }
    if (dut->m_axi_ARVALID && arready) {
      check_burst(dut->m_axi_ARADDR, dut->m_axi_ARLEN, dut->m_axi_ARSIZE, dut->m_axi_ARBURST);
      ar.push_back({dut->m_axi_ARADDR, dut->m_axi_ARLEN + 1, 0});
    }
    if (dut->m_axi_WVALID && wready) w.push_back({wd, dut->m_axi_WLAST});
    while (!aw.empty() && !w.empty()) {
      Burst& burst = aw.front();
      mem[burst.addr + (uint64_t)burst.done * kBytes] = w.front().first;
      if (w.front().second != (burst.done == burst.beats - 1)) fail("WLAST does not end the burst");
      w.pop_front();
      if (++burst.done == burst.beats) {
        b.push_back({cycle + 1 + rnd(b_delay + 1), burst.beats});
        aw.pop_front();
      }
    }
    if (bvalid && dut->m_axi_BREADY) {
      b_beats += b.front().second;
      b.pop_front();
    }
    if (rvalid && dut->m_axi_RREADY) {
      if (++ar.front().done == ar.front().beats) ar.pop_front();
      rvalid = false;
    }
    if (dut->read_data_empty_n && pop) {
      if (rexpect.empty() || get(dut->read_data_dout) != initial(rexpect.front()))
        fail("read data out of place");
      if (!rexpect.empty()) rexpect.pop_front();
      ++rgot;
    }
    if (dut->write_resp_empty_n && ack) {
      acked += dut->write_resp_dout + 1;
      if (acked > b_beats) fail("write acknowledged before its B");
    }
    if (push_ra && dut->read_addr_full_n) ++ri;
    if (coupled) {
      if (push_wa) { ++wi; ++wdi; }
    } else {
      if (push_wa && dut->write_addr_full_n) ++wi;
      if (push_wd && dut->write_data_full_n) ++wdi;
    }
    tick();
    if (rgot >= nr && acked >= nw && ri == rreq.size() && wi == wreq.size() && wdi == nw) ++settle;
  }
  if (rgot != nr || acked != nw) fail("requests left unserved");
  if (mem.size() != golden.size()) fail("a write landed off its request");
  for (const auto& p : golden)
    if (mem[p.first] != p.second) { fail("memory differs from the writes"); break; }
  delete dut;
  return errors == 0;
}

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  bool ok = true;
  for (uint64_t seed = 1; seed <= 8; ++seed) ok = run(seed) && ok;
  std::printf(ok ? "PASS\n" : "FAIL\n");
  return ok ? 0 : 1;
}
"#;

#[test]
fn random_requests_and_stalls_keep_axi_rules_and_data() {
    for (data_width, req_len_width, max_burst_len) in [(32, 0, 255), (512, 32, 15)] {
        let testbench =
            format!("#define DW {data_width}\n#define RLW {req_len_width}\n{STRESS_TESTBENCH}");
        run_cosim(
            "async_mmap",
            &[
                ("DataWidth", data_width),
                ("DataWidthBytesLog", (data_width / 8).trailing_zeros()),
                ("WaitTimeWidth", 2),
                ("MaxWaitTime", 3),
                ("BurstLenWidth", 8),
                ("MaxBurstLen", max_burst_len),
                ("ReqLenWidth", req_len_width),
            ],
            ASSETS,
            &[],
            &testbench,
        );
    }
}
