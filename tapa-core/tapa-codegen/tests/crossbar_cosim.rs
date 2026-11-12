//! Verilator cosim of the M-AXI crossbar codegen puts behind a memory that
//! several tasks share.
//!
//! Tasks sharing a memory reach it through a generated `axi_crossbar_SxM`
//! wrapper; an hmap's channels are its masters, each addressed from its own
//! offset. Each task here writes bursts into its own region and reads them
//! back, with random lengths and stalls, while the memory stalls every
//! channel at random and returns responses of different IDs out of order.
//! Every task reads what it wrote, and every burst reaches the channel its
//! address names, inside that channel and a 4 KiB page. The memory returns
//! each read burst whole: the crossbar does not take read data interleaved
//! across IDs.

mod common;

use common::run_manifest::{generate_manifest, rtl_file};
use common::verilator::run_cosim;
use common::{design, parse_module, task};

/// A task that writes 60 bursts of 1 to 16 words into its region (`REGION`
/// bytes from `BASE` in each of `CHANS` channels `CHAN_BYTES` apart), and
/// reads each back, stalling at random; it stops the simulation on a wrong
/// beat.
const MASTER: &str = "
module axi_master_model #(
  parameter [31:0] SEED = 1, parameter [63:0] BASE = 0, parameter [63:0] REGION = 4096,
  parameter [63:0] CHANS = 1, parameter [63:0] CHAN_BYTES = 0
) (
  input wire ap_clk, input wire ap_rst_n, input wire ap_start,
  output wire ap_done, output wire ap_idle, output wire ap_ready, input wire [63:0] mem_offset,
  output reg [63:0] m_axi_mem_AWADDR, output reg [7:0] m_axi_mem_AWLEN,
  input wire m_axi_mem_AWREADY, output reg m_axi_mem_AWVALID,
  output reg [31:0] m_axi_mem_WDATA, output reg m_axi_mem_WLAST, input wire m_axi_mem_WREADY,
  output reg m_axi_mem_WVALID,
  input wire m_axi_mem_BID, output reg m_axi_mem_BREADY, input wire m_axi_mem_BVALID,
  output reg [63:0] m_axi_mem_ARADDR, output reg [7:0] m_axi_mem_ARLEN,
  input wire m_axi_mem_ARREADY, output reg m_axi_mem_ARVALID,
  input wire [31:0] m_axi_mem_RDATA, input wire m_axi_mem_RID, input wire m_axi_mem_RLAST,
  output reg m_axi_mem_RREADY, input wire m_axi_mem_RVALID
);
  localparam [31:0] BURSTS = 60;
  reg [31:0] lfsr = SEED;
  wire [31:0] next = {lfsr[30:0], lfsr[31] ^ lfsr[21] ^ lfsr[1] ^ lfsr[0]};
  reg [2:0] state = 0;
  reg [31:0] pass = 0, beat = 0, len = 0;
  reg [63:0] addr = 0;
  reg finished = 0, done = 0;
  assign ap_done = done;
  assign ap_ready = done;
  assign ap_idle = state == 0;
  function [31:0] value(input [63:0] a, input [31:0] p);
    value = a[31:0] * 32'h9E3779B1 ^ p * 32'h85EBCA6B ^ SEED;
  endfunction
  always @(posedge ap_clk) begin
    if (!ap_rst_n) begin
      state <= 0; done <= 0; m_axi_mem_AWVALID <= 0; m_axi_mem_WVALID <= 0;
      m_axi_mem_BREADY <= 0; m_axi_mem_ARVALID <= 0; m_axi_mem_RREADY <= 0;
    end else begin
      lfsr <= next;
      done <= 0;
      case (state)
        0: if (ap_start && !finished) state <= 1;
        1: begin  // at most 64 bytes from a 64-byte boundary: in one page
          len <= next[3:0] + 1;
          addr <= mem_offset + (next[20] % CHANS) * CHAN_BYTES + BASE
                  + (next[15:6] % (REGION / 64)) * 64;
          state <= 2;
        end
        2: if (next[1:0] != 0) begin
          m_axi_mem_AWADDR <= addr; m_axi_mem_AWLEN <= len - 1; m_axi_mem_AWVALID <= 1;
          beat <= 0; state <= 3;
        end
        3: begin
          if (m_axi_mem_AWREADY) m_axi_mem_AWVALID <= 0;
          if (m_axi_mem_WVALID && m_axi_mem_WREADY) begin
            m_axi_mem_WVALID <= 0;
            beat <= beat + 1;
          end else if (!m_axi_mem_WVALID && beat < len && next[2]) begin
            m_axi_mem_WVALID <= 1; m_axi_mem_WDATA <= value(addr + beat * 4, pass);
            m_axi_mem_WLAST <= beat == len - 1;
          end
          if (beat == len && !m_axi_mem_AWVALID) state <= 4;
        end
        4: begin
          m_axi_mem_BREADY <= next[3];
          if (m_axi_mem_BREADY && m_axi_mem_BVALID) begin
            m_axi_mem_BREADY <= 0;
            if (m_axi_mem_BID != 0) begin $display(\"FAIL: BID %0d\", m_axi_mem_BID); $fatal; end
            state <= 5;
          end
        end
        5: if (next[4]) begin
          m_axi_mem_ARADDR <= addr; m_axi_mem_ARLEN <= len - 1; m_axi_mem_ARVALID <= 1;
          beat <= 0; state <= 6;
        end
        6: begin
          if (m_axi_mem_ARREADY) m_axi_mem_ARVALID <= 0;
          m_axi_mem_RREADY <= next[5] || next[6];
          if (m_axi_mem_RREADY && m_axi_mem_RVALID) begin
            if (m_axi_mem_RDATA != value(addr + beat * 4, pass) || m_axi_mem_RID != 0
                || m_axi_mem_RLAST != (beat == len - 1)) begin
              $display(\"FAIL: SEED %0h read %h at %h, beat %0d of %0d\", SEED, m_axi_mem_RDATA,
                       addr + beat * 4, beat, len);
              $fatal;
            end
            beat <= beat + 1;
            if (beat == len - 1) begin
              m_axi_mem_RREADY <= 0;
              pass <= pass + 1;
              if (pass + 1 == BURSTS) begin finished <= 1; done <= 1; state <= 0; end
              else state <= 1;
            end
          end
        end
        default: state <= 0;
      endcase
    end
  end
endmodule
";

/// The memory: per channel, with random stalls; it returns write responses
/// and read bursts of different IDs out of order, each ID's in order.
const TESTBENCH: &str = r#"
#include "Vtop.h"
#include <cstdint>
#include <cstdio>
#include <deque>
#include <map>
#include <random>
#include <vector>

static std::mt19937_64 rng;
static int rnd(int n) { return (int)(rng() % (uint64_t)n); }
struct Burst { uint64_t addr; int beats, done; unsigned id; long ready; };
struct Channel {
  uint64_t base, bytes;
  std::map<uint64_t, uint32_t> mem;
  std::deque<Burst> aw, ar;
  std::deque<std::pair<uint32_t, int>> w;
  std::vector<Burst> b;
  int r = -1, bv = -1, errors = 0;
  void check(uint64_t a, int beats) {
    if (a < base || a - base + 4ULL * beats > bytes || (a & 0xfff) + 4ULL * beats > 4096) {
      if (errors++ < 5) std::printf("FAIL: a burst of %d at 0x%llx\n", beats, (unsigned long long)a);
    }
  }
  // The oldest burst of a random ID among those ready.
  template <typename C> int pick(const C& q, long cycle) {
    std::vector<int> ok;
    for (size_t i = 0; i < q.size(); ++i) {
      bool oldest = true;
      for (size_t j = 0; j < i; ++j) oldest = oldest && q[j].id != q[i].id;
      if (oldest && q[i].ready <= cycle) ok.push_back(i);
    }
    return ok.empty() ? -1 : ok[rnd(ok.size())];
  }
};
#define DRIVE(P, C)                                                            \
  d->P##AWREADY = rnd(100) < p[0];                                             \
  d->P##WREADY = rnd(100) < p[1];                                              \
  d->P##ARREADY = rnd(100) < p[2];                                             \
  if (C.bv < 0 && rnd(100) < p[3]) C.bv = C.pick(C.b, cycle);                  \
  d->P##BVALID = C.bv >= 0;                                                    \
  d->P##BID = C.bv >= 0 ? C.b[C.bv].id : 0;                                    \
  d->P##BRESP = 0;                                                             \
  if (C.r < 0 && rnd(100) < p[4]) C.r = C.pick(C.ar, cycle);                   \
  d->P##RVALID = C.r >= 0;                                                     \
  if (C.r >= 0) {                                                              \
    const Burst& r = C.ar[C.r];                                                \
    const uint64_t a = r.addr + 4ULL * r.done;                                 \
    d->P##RDATA = C.mem.count(a) ? C.mem[a] : 0;                               \
    d->P##RID = r.id;                                                          \
    d->P##RLAST = r.done == r.beats - 1;                                       \
  }                                                                            \
  d->P##RRESP = 0;
#define TAKE(P, C)                                                             \
  if (d->P##AWVALID && d->P##AWREADY) {                                        \
    C.check(d->P##AWADDR, d->P##AWLEN + 1);                                    \
    C.aw.push_back({d->P##AWADDR, d->P##AWLEN + 1, 0, (unsigned)d->P##AWID, 0}); \
  }                                                                            \
  if (d->P##ARVALID && d->P##ARREADY) {                                        \
    C.check(d->P##ARADDR, d->P##ARLEN + 1);                                    \
    C.ar.push_back({d->P##ARADDR, d->P##ARLEN + 1, 0, (unsigned)d->P##ARID,    \
                    cycle + 2 + rnd(10)});                                     \
  }                                                                            \
  if (d->P##WVALID && d->P##WREADY) C.w.push_back({d->P##WDATA, d->P##WLAST});  \
  while (!C.aw.empty() && !C.w.empty()) {                                      \
    Burst& a = C.aw.front();                                                   \
    C.mem[a.addr + 4ULL * a.done] = C.w.front().first;                         \
    if (C.w.front().second != (a.done == a.beats - 1) && C.errors++ < 5)       \
      std::printf("FAIL: WLAST\n");                                            \
    C.w.pop_front();                                                           \
    if (++a.done == a.beats) {                                                 \
      a.ready = cycle + 1 + rnd(10);                                           \
      C.b.push_back(a);                                                        \
      C.aw.pop_front();                                                        \
    }                                                                          \
  }                                                                            \
  if (C.bv >= 0 && d->P##BREADY) {                                             \
    C.b.erase(C.b.begin() + C.bv);                                             \
    C.bv = -1;                                                                 \
  }                                                                            \
  if (C.r >= 0 && d->P##RREADY) {                                              \
    if (++C.ar[C.r].done == C.ar[C.r].beats) {                                 \
      C.ar.erase(C.ar.begin() + C.r);                                          \
      C.r = -1;                                                                \
    }                                                                          \
  }

static bool run(uint64_t seed) {
  rng.seed(seed);
  int p[5];
  for (int& x : p) x = 20 + rnd(81);
  Vtop* d = new Vtop;
  auto tick = [&]() { d->ap_clk = 1; d->eval(); d->ap_clk = 0; d->eval(); };
#ifdef HMAP
  Channel c0{0x100000, 8192}, c1{0x300000, 8192};
  d->mem_0_offset = c0.base;
  d->mem_1_offset = c1.base;
#else
  Channel c0{0x40000000, 3 * 4096};
  d->mem_offset = c0.base;
#endif
  d->ap_clk = 0; d->ap_rst_n = 0; d->ap_start = 0;
  for (int i = 0; i < 8; ++i) tick();
  d->ap_rst_n = 1;
  tick();
  d->ap_start = 1;
  long cycle = 0;
  for (; cycle < 100000 && !d->ap_done; ++cycle) {
#ifdef HMAP
    DRIVE(m_axi_mem_0_, c0)
    DRIVE(m_axi_mem_1_, c1)
    d->eval();
    TAKE(m_axi_mem_0_, c0)
    TAKE(m_axi_mem_1_, c1)
    const int errors = c0.errors + c1.errors;
#else
    DRIVE(m_axi_mem_, c0)
    d->eval();
    TAKE(m_axi_mem_, c0)
    const int errors = c0.errors;
#endif
    if (errors) break;
    tick();
  }
  const bool ok = d->ap_done;
  if (!ok) std::printf("FAIL: seed %llu stopped at cycle %ld\n", (unsigned long long)seed, cycle);
  delete d;
  return ok;
}

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  bool ok = true;
  for (uint64_t seed = 1; seed <= 6; ++seed) ok = run(seed) && ok;
  std::printf(ok ? "PASS\n" : "FAIL\n");
  return ok ? 0 : 1;
}
"#;

/// A task named `name` driving the memory through `axi_master_model`.
fn master_task(name: &str, seed: u32, base: u64, chans: u64, chan_bytes: u64) -> String {
    format!(
        "module {name}(
  input wire ap_clk, input wire ap_rst_n, input wire ap_start,
  output wire ap_done, output wire ap_idle, output wire ap_ready, input wire [63:0] mem_offset,
  output wire [63:0] m_axi_mem_AWADDR, output wire [1:0] m_axi_mem_AWBURST,
  output wire m_axi_mem_AWID, output wire [7:0] m_axi_mem_AWLEN, input wire m_axi_mem_AWREADY,
  output wire [2:0] m_axi_mem_AWSIZE, output wire m_axi_mem_AWVALID,
  output wire [31:0] m_axi_mem_WDATA, output wire m_axi_mem_WLAST, input wire m_axi_mem_WREADY,
  output wire [3:0] m_axi_mem_WSTRB, output wire m_axi_mem_WVALID,
  input wire m_axi_mem_BID, output wire m_axi_mem_BREADY, input wire [1:0] m_axi_mem_BRESP,
  input wire m_axi_mem_BVALID,
  output wire [63:0] m_axi_mem_ARADDR, output wire [1:0] m_axi_mem_ARBURST,
  output wire m_axi_mem_ARID, output wire [7:0] m_axi_mem_ARLEN, input wire m_axi_mem_ARREADY,
  output wire [2:0] m_axi_mem_ARSIZE, output wire m_axi_mem_ARVALID,
  input wire [31:0] m_axi_mem_RDATA, input wire m_axi_mem_RID, input wire m_axi_mem_RLAST,
  output wire m_axi_mem_RREADY, input wire [1:0] m_axi_mem_RRESP, input wire m_axi_mem_RVALID
);
  assign m_axi_mem_AWBURST = 2'd1;
  assign m_axi_mem_ARBURST = 2'd1;
  assign m_axi_mem_AWSIZE = 3'd2;
  assign m_axi_mem_ARSIZE = 3'd2;
  assign m_axi_mem_AWID = 1'b0;
  assign m_axi_mem_ARID = 1'b0;
  assign m_axi_mem_WSTRB = 4'hf;
  axi_master_model #(.SEED({seed}), .BASE({base}), .REGION(4096), .CHANS({chans}),
                     .CHAN_BYTES({chan_bytes})) model (
    .ap_clk(ap_clk), .ap_rst_n(ap_rst_n), .ap_start(ap_start), .ap_done(ap_done),
    .ap_idle(ap_idle), .ap_ready(ap_ready), .mem_offset(mem_offset),
    .m_axi_mem_AWADDR(m_axi_mem_AWADDR), .m_axi_mem_AWLEN(m_axi_mem_AWLEN),
    .m_axi_mem_AWREADY(m_axi_mem_AWREADY), .m_axi_mem_AWVALID(m_axi_mem_AWVALID),
    .m_axi_mem_WDATA(m_axi_mem_WDATA), .m_axi_mem_WLAST(m_axi_mem_WLAST),
    .m_axi_mem_WREADY(m_axi_mem_WREADY), .m_axi_mem_WVALID(m_axi_mem_WVALID),
    .m_axi_mem_BID(m_axi_mem_BID), .m_axi_mem_BREADY(m_axi_mem_BREADY),
    .m_axi_mem_BVALID(m_axi_mem_BVALID),
    .m_axi_mem_ARADDR(m_axi_mem_ARADDR), .m_axi_mem_ARLEN(m_axi_mem_ARLEN),
    .m_axi_mem_ARREADY(m_axi_mem_ARREADY), .m_axi_mem_ARVALID(m_axi_mem_ARVALID),
    .m_axi_mem_RDATA(m_axi_mem_RDATA), .m_axi_mem_RID(m_axi_mem_RID),
    .m_axi_mem_RLAST(m_axi_mem_RLAST), .m_axi_mem_RREADY(m_axi_mem_RREADY),
    .m_axi_mem_RVALID(m_axi_mem_RVALID)
  );
endmodule
"
    )
}

/// `masters` tasks sharing the top's memory `mem`, an hmap of two 8 KiB
/// channels (`chan_size` 2048 words) at offsets when `hmap`, and each
/// task's RTL source by file name.
fn shared_memory_state(
    masters: u64,
    hmap: bool,
) -> (
    tapa_codegen::rtl_state::TopologyWithRtl,
    Vec<(String, String)>,
) {
    let mut mem = serde_json::json!({"cat": "mmap", "name": "mem", "type": "int*", "width": 32});
    if hmap {
        mem["chan_count"] = 2.into();
        mem["chan_size"] = 2048.into();
    }
    let names: Vec<String> = (0..masters).map(|i| format!("m{i}")).collect();
    let top = task("top", "upper", |t| {
        t["ports"] = serde_json::json!([mem]);
        t["tasks"] = names
            .iter()
            .map(|name| {
                let args = serde_json::json!([{"args": {"mem": {"arg": "mem", "cat": "mmap"}}}]);
                (name.clone(), args)
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
    });
    let leaf = |name: &str| {
        task(name, "lower", |t| {
            t["ports"] =
                serde_json::json!([{"cat": "mmap", "name": "mem", "type": "int*", "width": 32}]);
        })
    };
    let mut tasks = vec![("top", top)];
    tasks.extend(names.iter().map(|name| (name.as_str(), leaf(name))));
    let mut state =
        tapa_codegen::rtl_state::TopologyWithRtl::new(design("top", "xilinx-hls", &tasks));
    let offsets = if hmap {
        "input wire [63:0] mem_0_offset, input wire [63:0] mem_1_offset"
    } else {
        "input wire [63:0] mem_offset"
    };
    state
        .attach_module(
            "top",
            parse_module(&format!(
                "module top(input wire ap_clk, input wire ap_rst_n, input wire ap_start,\n\
                 output wire ap_done, output wire ap_idle, output wire ap_ready,\n\
                 {offsets});\nendmodule"
            )),
        )
        .unwrap();
    let (chans, chan_bytes) = if hmap { (2, 8192) } else { (1, 0) };
    let mut sources = Vec::new();
    for (name, i) in names.iter().zip(0u32..) {
        let source = master_task(
            name,
            0x0012_D687 + 0x457 * i,
            u64::from(i) * 4096,
            chans,
            chan_bytes,
        );
        state.attach_module(name, parse_module(&source)).unwrap();
        sources.push((format!("{name}.v"), source));
    }
    (state, sources)
}

#[test]
fn tasks_sharing_a_memory_through_the_crossbar_read_what_they_wrote() {
    // Three tasks on one memory, and two on a two-channel hmap.
    for (masters, hmap) in [(3, false), (2, true)] {
        let (mut state, sources) = shared_memory_state(masters, hmap);
        let manifest = generate_manifest(&mut state);
        let crossbar = format!("axi_crossbar_{masters}x{}.v", if hmap { 2 } else { 1 });
        let mut extra: Vec<(&str, &str)> = vec![
            ("top.v", rtl_file(&manifest, "top.v")),
            ("top_fsm.v", rtl_file(&manifest, "top_fsm.v")),
            (crossbar.as_str(), rtl_file(&manifest, &crossbar)),
            ("master.v", MASTER),
        ];
        extra.extend(
            sources
                .iter()
                .map(|(file, source)| (file.as_str(), source.as_str())),
        );
        let testbench = if hmap {
            format!("#define HMAP\n{TESTBENCH}")
        } else {
            TESTBENCH.to_owned()
        };
        run_cosim(
            "top",
            &[],
            &[
                "axi_crossbar.v",
                "axi_crossbar_addr.v",
                "axi_crossbar_rd.v",
                "axi_crossbar_wr.v",
                "axi_register_rd.v",
                "axi_register_wr.v",
                "arbiter.v",
                "priority_encoder.v",
            ],
            &extra,
            &testbench,
        );
    }
}
