//! Verilator cosims for the RTL that moves frames.
//!
//! `frame_channel` hands its banks between a producer and a consumer in
//! order, whether each end is a frame port, which touches a frame's
//! elements in any order, or an element stream. `frame_ring` moves frames
//! between a host and a channel through the slots of a ring in device
//! memory: a slot is reused only once the other side has taken its frame,
//! a stale `host_frames` (XRT rewrites every register at a start) never
//! moves the ring backwards, and an output ring is `drained` only while
//! everything released into it is in memory. Both run under random stalls.

mod common;

use common::verilator::run_cosim;

const CHANNEL_TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vframe_channel.h"
#include <cstdint>
#include <cstdio>
#include <random>
#include <vector>

static std::mt19937_64 rng;
static int rnd(int n) { return (int)(rng() % (uint64_t)n); }
static bool chance(int p) { return rnd(100) < p; }
static uint32_t value(int frame, int i) { return (uint32_t)(frame * 1000 + i + 1); }

// A frame's element indices, in order or shuffled.
static std::vector<int> order() {
  std::vector<int> o;
  for (int i = 0; i < TB_FRAME; ++i) o.push_back(i);
  if (chance(50))
    for (int i = TB_FRAME - 1; i > 0; --i) std::swap(o[i], o[rnd(i + 1)]);
  return o;
}

static bool run(uint64_t seed) {
  rng.seed(seed);
  const int frames = 40, pw = 5 + rnd(96), pr = 5 + rnd(96);
  Vframe_channel* d = new Vframe_channel;
  auto tick = [&]() { d->clk = 1; d->eval(); d->clk = 0; d->eval(); };
  d->clk = 0; d->reset = 1;
  for (int i = 0; i < 3; ++i) tick();
  d->reset = 0;
  int wf = 0, wi = 0, rf = 0, ri = 0, errors = 0;
  bool w_have = false, r_have = false;
  int w_bank = 0, r_bank = 0, inflight = -1;
  std::vector<int> w_order, r_order;
  size_t w_pos = 0, r_pos = 0;
  std::vector<uint32_t> got(TB_FRAME);
  long cycle = 0;
  auto fail = [&](const char* what) {
    if (errors++ < 5) std::printf("seed %llu cycle %ld: %s\n", (unsigned long long)seed, cycle, what);
  };
  for (; cycle < 100000 && rf < frames; ++cycle) {
    d->w_acquire_read = d->w_buf_ce0 = d->w_buf_we0 = d->w_release_write = d->if_write = 0;
    d->r_acquire_read = d->r_buf_ce0 = d->r_release_write = d->if_read = 0;
    bool w_acq = false, w_put = false, w_rel = false, push = false;
    bool r_acq = false, r_get = false, r_rel = false, pop = false;
    if (wf < frames && chance(pw)) {
      if (!TB_W_VIEW) {
        d->if_write = 1; d->if_din = value(wf, wi); push = true;
      } else if (!w_have) {
        d->w_acquire_read = 1; w_acq = true;
      } else if (w_pos < w_order.size()) {
        const int i = w_order[w_pos];
        d->w_buf_ce0 = d->w_buf_we0 = 1;
        d->w_buf_address0 = w_bank * TB_FRAME + i;
        d->w_buf_d0 = value(wf, i);
        w_put = true;
      } else {
        d->w_release_write = 1; w_rel = true;
      }
    }
    int issued = -1;
    if (rf < frames && chance(pr)) {
      if (!TB_R_VIEW) {
        d->if_read = 1; pop = true;
      } else if (!r_have) {
        d->r_acquire_read = 1; r_acq = true;
      } else if (r_pos < r_order.size()) {
        issued = r_order[r_pos];
        d->r_buf_ce0 = 1;
        d->r_buf_address0 = r_bank * TB_FRAME + issued;
        r_get = true;
        // A release may go with the frame's last read.
        if (r_pos + 1 == r_order.size() && chance(50)) { d->r_release_write = 1; r_rel = true; }
      } else {
        d->r_release_write = 1; r_rel = true;
      }
    }
    d->eval();
    if (w_acq && d->w_acquire_empty_n) {
      w_have = true; w_bank = d->w_acquire_dout; w_order = order(); w_pos = 0;
      if (w_bank != wf % TB_BANKS) fail("producer acquired a bank out of turn");
    }
    if (w_put) ++w_pos;
    if (w_rel) { w_have = false; ++wf; }
    if (push && d->if_full_n && ++wi == TB_FRAME) { wi = 0; ++wf; }
    if (r_acq && d->r_acquire_empty_n) {
      r_have = true; r_bank = d->r_acquire_dout; r_order = order(); r_pos = 0;
    }
    if (r_get) { inflight = issued; ++r_pos; }
    if (pop && d->if_empty_n) {
      if ((d->if_dout >> 32) & 1) fail("end of transmission on the element stream");
      if ((uint32_t)d->if_dout != value(rf, ri)) fail("element out of place");
      if (++ri == TB_FRAME) { ri = 0; ++rf; }
    }
    tick();
    // A view's read data arrives a cycle after its address.
    if (inflight >= 0) { got[inflight] = d->r_buf_q0; inflight = -1; }
    if (r_rel) {
      r_have = false;
      for (int i = 0; i < TB_FRAME; ++i)
        if (got[i] != value(rf, i)) fail("frame element out of place");
      ++rf;
    }
  }
  if (rf < frames) fail("frames did not all arrive");
  delete d;
  return errors == 0;
}

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  bool ok = true;
  for (uint64_t seed = 1; seed <= 16; ++seed) ok = run(seed) && ok;
  std::printf(ok ? "PASS\n" : "FAIL\n");
  return ok ? 0 : 1;
}
"#;

#[test]
fn a_frame_channel_hands_its_banks_over_in_order() {
    // Every pairing of view and element-stream ends, with one to three
    // banks between them.
    for (w_view, r_view) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let banks = 1 + w_view + r_view;
        let testbench = format!(
            "#define TB_FRAME 3\n#define TB_BANKS {banks}\n#define TB_W_VIEW {w_view}\n\
             #define TB_R_VIEW {r_view}\n{CHANNEL_TESTBENCH}"
        );
        run_cosim(
            "frame_channel",
            &[
                ("DATA_WIDTH", 32),
                ("FRAME", 3),
                ("BANKS", banks),
                ("ADDR_WIDTH", 10),
                ("BANK_WIDTH", 8),
                ("W_VIEW", w_view),
                ("R_VIEW", r_view),
            ],
            &["frame_channel.v"],
            &[],
            &testbench,
        );
    }
}

/// A `frame_ring` of 32-bit elements and the `async_mmap` it moves frames
/// through.
const RING_TOP: &str = "
`default_nettype none
module ring_top #(
  parameter FRAME = 5, parameter SLOTS = 3, parameter INPUT = 1, parameter ELEMENTS = 0
) (
  input wire clk, input wire rst,
  input wire [63:0] base, input wire [31:0] host_frames, output wire [31:0] kernel_frames,
  output wire [32:0] out_din, input wire out_full_n, output wire out_write,
  input wire [32:0] in_dout, input wire in_empty_n, output wire in_read,
  input wire released, output wire drained,
  output wire m_axi_AWVALID, input wire m_axi_AWREADY, output wire [63:0] m_axi_AWADDR,
  output wire [7:0] m_axi_AWLEN,
  output wire m_axi_WVALID, input wire m_axi_WREADY, output wire [31:0] m_axi_WDATA,
  output wire m_axi_WLAST,
  input wire m_axi_BVALID, output wire m_axi_BREADY,
  output wire m_axi_ARVALID, input wire m_axi_ARREADY, output wire [63:0] m_axi_ARADDR,
  output wire [7:0] m_axi_ARLEN,
  input wire m_axi_RVALID, output wire m_axi_RREADY, input wire [31:0] m_axi_RDATA,
  input wire m_axi_RLAST
);
  wire [95:0] ra_din, wa_din;
  wire ra_full_n, ra_write, wa_full_n, wa_write, rd_empty_n, rd_read;
  wire [31:0] rd_dout;
  wire [32:0] wd_din;
  wire wd_full_n, wd_write, wr_empty_n, wr_read;
  wire [7:0] wr_dout;
  frame_ring #(
    .DATA_WIDTH(32), .FRAME(FRAME), .SLOTS(SLOTS), .INPUT(INPUT), .ADDR_WIDTH(64),
    .LEN_WIDTH(32), .RELEASED_ELEMENTS(ELEMENTS)
  ) ring (
    .clk(clk), .rst(rst), .base(base), .host_frames(host_frames),
    .kernel_frames(kernel_frames), .shape(),
    .out_din(out_din), .out_full_n(out_full_n), .out_write(out_write),
    .in_dout(in_dout), .in_empty_n(in_empty_n), .in_read(in_read),
    .released(released), .drained(drained),
    .read_addr_din(ra_din), .read_addr_full_n(ra_full_n), .read_addr_write(ra_write),
    .read_data_dout(rd_dout), .read_data_empty_n(rd_empty_n), .read_data_read(rd_read),
    .write_addr_din(wa_din), .write_addr_full_n(wa_full_n), .write_addr_write(wa_write),
    .write_data_din(wd_din), .write_data_full_n(wd_full_n), .write_data_write(wd_write),
    .write_resp_dout(wr_dout), .write_resp_empty_n(wr_empty_n), .write_resp_read(wr_read)
  );
  async_mmap #(
    .DataWidth(32), .DataWidthBytesLog(2), .AddrWidth(64), .WaitTimeWidth(2),
    .MaxWaitTime(3), .BurstLenWidth(8), .MaxBurstLen(255), .ReqLenWidth(32),
    .EnableReadChannel(INPUT), .EnableWriteChannel(!INPUT)
  ) mmap (
    .clk(clk), .rst(rst),
    .m_axi_AWVALID(m_axi_AWVALID), .m_axi_AWREADY(m_axi_AWREADY), .m_axi_AWADDR(m_axi_AWADDR),
    .m_axi_AWID(), .m_axi_AWLEN(m_axi_AWLEN), .m_axi_AWSIZE(), .m_axi_AWBURST(),
    .m_axi_AWLOCK(), .m_axi_AWCACHE(), .m_axi_AWPROT(), .m_axi_AWQOS(),
    .m_axi_WVALID(m_axi_WVALID), .m_axi_WREADY(m_axi_WREADY), .m_axi_WDATA(m_axi_WDATA),
    .m_axi_WSTRB(), .m_axi_WLAST(m_axi_WLAST),
    .m_axi_BVALID(m_axi_BVALID), .m_axi_BREADY(m_axi_BREADY), .m_axi_BRESP(2'b0),
    .m_axi_BID(1'b0),
    .m_axi_ARVALID(m_axi_ARVALID), .m_axi_ARREADY(m_axi_ARREADY), .m_axi_ARADDR(m_axi_ARADDR),
    .m_axi_ARID(), .m_axi_ARLEN(m_axi_ARLEN), .m_axi_ARSIZE(), .m_axi_ARBURST(),
    .m_axi_ARLOCK(), .m_axi_ARCACHE(), .m_axi_ARPROT(), .m_axi_ARQOS(),
    .m_axi_RVALID(m_axi_RVALID), .m_axi_RREADY(m_axi_RREADY), .m_axi_RDATA(m_axi_RDATA),
    .m_axi_RLAST(m_axi_RLAST), .m_axi_RID(1'b0), .m_axi_RRESP(2'b0),
    .read_addr_din(ra_din), .read_addr_write(ra_write), .read_addr_full_n(ra_full_n),
    .read_data_dout(rd_dout), .read_data_read(rd_read), .read_data_empty_n(rd_empty_n),
    .write_addr_din(wa_din), .write_addr_write(wa_write), .write_addr_full_n(wa_full_n),
    .write_data_din(wd_din[31:0]), .write_data_write(wd_write), .write_data_full_n(wd_full_n),
    .write_resp_dout(wr_dout), .write_resp_read(wr_read), .write_resp_empty_n(wr_empty_n)
  );
endmodule
`default_nettype wire
";

const RING_TESTBENCH: &str = r#"
#include <verilated.h>
#include "Vring_top.h"
#include <cstdint>
#include <cstdio>
#include <deque>
#include <map>
#include <random>
#include <vector>

static std::mt19937_64 rng;
static int rnd(int n) { return (int)(rng() % (uint64_t)n); }
static bool chance(int p) { return rnd(100) < p; }
static uint32_t value(uint32_t frame, int i) { return frame * 7919 + i * 13 + 1; }
struct Burst { uint64_t addr; int beats; int done; };

static bool run(uint64_t seed) {
  rng.seed(seed);
  const uint32_t frames = 30;
  // Slots straddling 4 GiB, or not.
  const uint64_t base = chance(50) ? 0x10000 : 0xFFFFFFF0ULL - 4ULL * TB_FRAME * rnd(3);
  const int p_host = 5 + rnd(96), p_kernel = 5 + rnd(96), p_aw = 20 + rnd(81),
            p_w = 20 + rnd(81), p_ar = 20 + rnd(81), p_r = 20 + rnd(81), p_b = 20 + rnd(81);
  auto slot = [&](uint32_t frame, int i) {
    return base + 4ULL * ((uint64_t)(frame % TB_SLOTS) * TB_FRAME + i);
  };
  std::map<uint64_t, uint32_t> mem;
  Vring_top* d = new Vring_top;
  auto tick = [&]() { d->clk = 1; d->eval(); d->clk = 0; d->eval(); };
  d->base = base; d->host_frames = 0; d->clk = 0; d->rst = 1;
  for (int i = 0; i < 3; ++i) tick();
  d->rst = 0;
  uint32_t host = 0;        // the host's frame count
  int stale = 0;            // cycles left of a stale host_frames register
  uint32_t taken = 0;       // TB_INPUT: frames the kernel took
  int index = 0;            // of the element moving next
  uint32_t produced = 0;    // !TB_INPUT: frames the kernel produced
  std::deque<uint32_t> channel;  // !TB_INPUT: elements released into the channel
  std::vector<uint32_t> frame_buffer;
  long released = 0;        // frames, or elements with TB_ELEMENTS
  uint64_t acked = 0;       // beats memory acknowledged
  std::deque<Burst> aw, ar;
  std::deque<std::pair<uint32_t, int>> w;
  std::deque<int> b;
  bool rvalid = false;
  int errors = 0;
  long cycle = 0;
  auto fail = [&](const char* what) {
    if (errors++ < 5) std::printf("seed %llu cycle %ld: %s\n", (unsigned long long)seed, cycle, what);
  };
  for (; cycle < 2000000 && (TB_INPUT ? taken : host) < frames; ++cycle) {
    // The host fills a free slot (input) or takes a written frame (output).
    const uint32_t kernel = d->kernel_frames;
    if (TB_INPUT && host < frames && host - kernel < TB_SLOTS && chance(p_host)) {
      for (int i = 0; i < TB_FRAME; ++i) mem[slot(host, i)] = value(host, i);
      ++host;
    }
    if (!TB_INPUT && host < kernel && chance(p_host)) {
      for (int i = 0; i < TB_FRAME; ++i)
        if (!mem.count(slot(host, i)) || mem[slot(host, i)] != value(host, i)) {
          fail("the host read a frame out of place");
          break;
        }
      ++host;
    }
    if (stale == 0 && host > 2 && chance(2)) stale = 1 + rnd(3);
    d->host_frames = stale ? host - 1 - rnd(2) : host;
    if (stale) --stale;
    // The kernel takes elements (input) or releases them (output).
    d->out_full_n = TB_INPUT && chance(p_kernel);
    bool release = false;
    if (!TB_INPUT && produced < frames && chance(p_kernel)) {
      const uint32_t v = value(produced, index);
      if (TB_ELEMENTS) {
        channel.push_back(v);
        release = true;
      } else {
        frame_buffer.push_back(v);
      }
      if (++index == TB_FRAME) {
        index = 0;
        ++produced;
        if (!TB_ELEMENTS) {
          for (uint32_t e : frame_buffer) channel.push_back(e);
          frame_buffer.clear();
          release = true;
        }
      }
    }
    const long released_before = released;
    if (release) ++released;
    d->released = release;
    d->in_empty_n = !channel.empty();
    d->in_dout = channel.empty() ? 0 : channel.front();
    // Memory.
    const bool awready = chance(p_aw), wready = chance(p_w);
    d->m_axi_AWREADY = awready;
    d->m_axi_WREADY = wready;
    d->m_axi_ARREADY = chance(p_ar);
    d->m_axi_BVALID = !b.empty() && chance(p_b);
    if (!rvalid && !ar.empty() && chance(p_r)) rvalid = true;
    d->m_axi_RVALID = rvalid;
    if (rvalid) {
      const uint64_t a = ar.front().addr + 4ULL * ar.front().done;
      d->m_axi_RDATA = mem.count(a) ? mem[a] : 0;
      d->m_axi_RLAST = ar.front().done == ar.front().beats - 1;
    }
    d->eval();
    // Everything released before this cycle is in memory while drained.
    if (!TB_INPUT && d->drained && acked != (uint64_t)released_before * (TB_ELEMENTS ? 1 : TB_FRAME))
      fail("drained before memory took everything released");
    if (d->m_axi_AWVALID && awready) aw.push_back({d->m_axi_AWADDR, d->m_axi_AWLEN + 1, 0});
    if (d->m_axi_ARVALID && d->m_axi_ARREADY)
      ar.push_back({d->m_axi_ARADDR, d->m_axi_ARLEN + 1, 0});
    if (d->m_axi_WVALID && wready) w.push_back({d->m_axi_WDATA, d->m_axi_WLAST});
    while (!aw.empty() && !w.empty()) {
      Burst& burst = aw.front();
      mem[burst.addr + 4ULL * burst.done] = w.front().first;
      if (w.front().second != (burst.done == burst.beats - 1)) fail("WLAST does not end the burst");
      w.pop_front();
      if (++burst.done == burst.beats) { b.push_back(burst.beats); aw.pop_front(); }
    }
    if (d->m_axi_BVALID && d->m_axi_BREADY) { acked += b.front(); b.pop_front(); }
    if (rvalid && d->m_axi_RREADY) {
      if (++ar.front().done == ar.front().beats) ar.pop_front();
      rvalid = false;
    }
    if (TB_INPUT && d->out_write && d->out_full_n) {
      if (taken >= frames || (uint32_t)d->out_din != value(taken, index))
        fail("the kernel took an element out of place");
      if (++index == TB_FRAME) { index = 0; ++taken; }
    }
    if (!TB_INPUT && d->in_read && d->in_empty_n) channel.pop_front();
    tick();
  }
  if ((TB_INPUT ? taken : host) < frames) fail("frames did not all arrive");
  if (d->kernel_frames != frames) fail("kernel_frames does not count every frame");
  delete d;
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
fn a_frame_ring_moves_frames_through_its_slots() {
    // An input ring, and an output ring counting released frames or, before
    // a plain FIFO, released elements.
    for (input, elements) in [(1, 0), (0, 0), (0, 1)] {
        let testbench = format!(
            "#define TB_FRAME 5\n#define TB_SLOTS 3\n#define TB_INPUT {input}\n\
             #define TB_ELEMENTS {elements}\n{RING_TESTBENCH}"
        );
        run_cosim(
            "ring_top",
            &[
                ("FRAME", 5),
                ("SLOTS", 3),
                ("INPUT", input),
                ("ELEMENTS", elements),
            ],
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
            ],
            &[("ring_top.v", RING_TOP)],
            &testbench,
        );
    }
}
