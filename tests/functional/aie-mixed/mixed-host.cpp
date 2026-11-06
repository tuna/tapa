#include <iostream>
#include <vector>

#include <gflags/gflags.h>

#include "mixed.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  const int n = argc > 1 ? atoi(argv[1]) : 256;  // elements of 4 floats
  std::vector<float> a(n * 4);
  std::vector<float> b(n * 4);
  std::vector<float> c(n * 4);
  for (int i = 0; i < n * 4; ++i) {
    a[i] = static_cast<float>(i);
    b[i] = static_cast<float>(i) * 2;
  }
  tapa::invoke(Mixed, FLAGS_bitstream,
               tapa::read_only_mmap<float>(a).vectorized<4>(),
               tapa::read_only_mmap<float>(b).vectorized<4>(),
               tapa::write_only_mmap<float>(c).vectorized<4>(), n);
  int num_errors = 0;
  for (int i = 0; i < n * 4; ++i) {
    if (c[i] != static_cast<float>(i) * 6) ++num_errors;
  }
  std::clog << (num_errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return num_errors > 0 ? 1 : 0;
}
