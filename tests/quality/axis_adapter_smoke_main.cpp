// Behavior smoke test for the AXIS <-> stream adapters. The clock is driven
// here rather than in SystemVerilog so the verilated model needs no delay
// controls, and therefore no C++20 coroutine support to build.

#include <cstdio>
#include <cstdlib>
#include <memory>

#include <verilated.h>

#include "Vtb.h"

namespace {

std::unique_ptr<Vtb> top;
int failures = 0;

void check(bool cond, const char* msg) {
  if (!cond) {
    std::fprintf(stderr, "FAIL: %s\n", msg);
    ++failures;
  }
}

// Settle combinational logic against the inputs driven so far.
void settle() { top->eval(); }

// One clock cycle, returning with the clock low so the next stimulus is
// applied well before the following rising edge.
void tick() {
  top->clk = 1;
  top->eval();
  top->clk = 0;
  top->eval();
}

}  // namespace

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  top = std::make_unique<Vtb>();

  top->clk = 0;
  top->reset = 1;
  top->s_axis_tdata = 0;
  top->s_axis_tvalid = 0;
  top->s_axis_tlast = 0;
  top->m_stream_read = 0;
  top->s_stream_din = 0;
  top->s_stream_write = 0;
  top->m_axis_tready = 0;
  settle();
  tick();
  tick();
  top->reset = 0;

  // stream -> AXIS: a write is visible on the AXIS side one cycle later.
  top->s_stream_din = 0x155;
  top->s_stream_write = 1;
  top->m_axis_tready = 0;
  settle();
  check(top->m_axis_tvalid == 0, "stream_to_axis is registered");

  tick();
  check(top->m_axis_tvalid == 1, "stream_to_axis first beat visible");
  check((top->m_axis_tlast << 8 | top->m_axis_tdata) == 0x155,
        "stream_to_axis first beat payload");
  top->s_stream_write = 0;

  top->m_axis_tready = 1;
  tick();
  check(top->m_axis_tvalid == 0, "stream_to_axis drained");

  // AXIS -> stream: a beat is visible on the stream side one cycle later.
  top->s_axis_tdata = 0x11;
  top->s_axis_tlast = 1;
  top->s_axis_tvalid = 1;
  top->m_stream_read = 0;
  settle();
  check(top->m_stream_empty_n == 0, "axis_to_stream is registered");

  tick();
  check(top->m_stream_empty_n == 1, "axis_to_stream first beat visible");
  check(top->m_stream_dout == 0x111, "axis_to_stream first beat payload");

  top->m_stream_read = 1;
  top->s_axis_tvalid = 0;
  tick();
  check(top->m_stream_empty_n == 0, "axis_to_stream drained");

  top->final();
  // Destroy the model here: at static-destruction time Verilator's global
  // context is already gone, and ~Vtb would abort reaching for it.
  top.reset();
  if (failures != 0) {
    std::fprintf(stderr, "%d check(s) failed\n", failures);
    return EXIT_FAILURE;
  }
  std::puts("PASS");
  return EXIT_SUCCESS;
}
