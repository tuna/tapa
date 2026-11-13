#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "vadd.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  const uint64_t n = argc > 1 ? atoll(argv[1]) : 1024;
  std::vector<float> mem(2 * n, -1.f);
  for (uint64_t i = 0; i < n; ++i) mem[i] = i;
  tapa::invoke(Top, FLAGS_bitstream, tapa::read_write_mmap<float>(mem), n);

  uint64_t errors = 0;
  for (uint64_t i = 0; i < 2 * n; ++i) {
    const float expected = i < n ? i : 2.f * (i - n);
    if (mem[i] != expected && errors++ < 10) {
      std::clog << "mem[" << i << "]: expected " << expected << ", got "
                << mem[i] << std::endl;
    }
  }
  std::clog << (errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return errors == 0 ? 0 : 1;
}
