#include "scalar.h"

#include <cstdint>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>

DEFINE_string(bitstream, "", "XO/ZIP path; empty runs software simulation");

int main(int argc, char** argv) {
  gflags::ParseCommandLineFlags(&argc, &argv, true);
  // Every value sets its high bits, so a register written one word off, or a
  // word short, shows up in the result.
  tapa::u<80> a = 0x1234;
  a = (a << 64) | 0x89abcdef01234567ULL;
  const int b = -123456;
  const tapa::u<12> c = 0xabc;
  const long long d = -0x123456789abcdefLL;
  const tapa::i<12> e = -5;
  tapa::i<80> f = -1;
  f = (f << 70) + 7;
  const uint64_t g = 0xfedcba9876543210ULL;

  std::vector<uint64_t> result(9);
  tapa::invoke(ScalarWidth, FLAGS_bitstream,
               tapa::write_only_mmap<uint64_t>(result), a, b, c, d, e, f, g);

  const uint64_t expected[] = {
      0x89abcdef01234567ULL,
      0x1234,
      static_cast<uint64_t>(int64_t{b}),
      0xabc,
      static_cast<uint64_t>(d),
      static_cast<uint64_t>(int64_t{-5}),
      7,
      0xffc0,
      g,
  };
  bool failed = false;
  for (size_t i = 0; i < result.size(); ++i) {
    if (result[i] != expected[i]) {
      std::cerr << "result[" << i << "] = 0x" << std::hex << result[i]
                << ", expected 0x" << expected[i] << std::dec << "\n";
      failed = true;
    }
  }
  std::clog << (failed ? "FAIL!" : "PASS!") << std::endl;
  return failed ? 1 : 0;
}
