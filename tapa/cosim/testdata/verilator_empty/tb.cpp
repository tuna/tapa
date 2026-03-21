#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <map>
#include <vector>
#include "Vtop.h"
#include "verilated.h"

static std::map<uint64_t, uint8_t> memory;

// Byte-oriented memory read: copies nbytes from memory into buf
static void mem_read(uint64_t addr, void* buf, size_t nbytes) {
  auto* dst = reinterpret_cast<uint8_t*>(buf);
  for (size_t i = 0; i < nbytes; i++) dst[i] = memory[addr + i];
}

// Byte-oriented memory write: copies nbytes from buf into memory
// strb is a per-byte write enable bitmask
static void mem_write(uint64_t addr, const void* buf, uint64_t strb,
                      size_t nbytes) {
  auto* src = reinterpret_cast<const uint8_t*>(buf);
  for (size_t i = 0; i < nbytes; i++)
    if (strb & (1ULL << i)) memory[addr + i] = src[i];
}

static void load_binary(const char* path, uint64_t base, size_t size) {
  std::ifstream f(path, std::ios::binary);
  if (!f) {
    fprintf(stderr, "Cannot open %s\n", path);
    return;
  }
  std::vector<char> buf(size);
  f.read(buf.data(), size);
  size_t n = f.gcount();
  for (size_t i = 0; i < n; i++) memory[base + i] = (uint8_t)buf[i];
}

static void dump_binary(const char* path, uint64_t base, size_t size) {
  std::ofstream f(path, std::ios::binary);
  for (size_t i = 0; i < size; i++) f.put((char)memory[base + i]);
}

struct AxiReadPort {
  bool busy = false;
  uint64_t addr = 0;
  uint8_t len = 0, beat = 0, id = 0;
};

struct AxiWritePort {
  bool aw_got = false, b_pending = false;
  uint64_t addr = 0;
  uint8_t beat = 0, id = 0;
};

static Vtop* dut;

static void tick() {
  dut->ap_clk = 1;
  dut->eval();
  dut->ap_clk = 0;
  dut->eval();
}

static void service_all_axi();

int main(int argc, char** argv) {
  Verilated::commandArgs(argc, argv);
  dut = new Vtop;

  // Initialize
  dut->ap_clk = 0;
  dut->ap_rst_n = 0;
  dut->ap_start = 0;

  // Reset
  for (int i = 0; i < 10; i++) tick();
  dut->ap_rst_n = 1;
  for (int i = 0; i < 5; i++) tick();

  // Start kernel
  dut->ap_start = 1;
  service_all_axi();
  tick();
  printf("Kernel started, running simulation...\n");

  bool done = false;
  int timeout = 50000000;
  for (int cycle = 0; cycle < timeout; cycle++) {
    service_all_axi();
    tick();
    if (dut->ap_done) {
      printf("Kernel done after %d cycles\n", cycle);
      done = true;
      break;
    }
    if (dut->ap_ready) {
      dut->ap_start = 0;
    }
  }

  if (!done) {
    printf("TIMEOUT: kernel did not complete in %d cycles\n", timeout);
    delete dut;
    return 1;
  }

  printf("Simulation completed successfully\n");
  delete dut;
  return 0;
}

static void service_all_axi() {}
