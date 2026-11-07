#include <cstdlib>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "corner.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);
  const int n = argc > 1 ? atoi(argv[1]) : 8;

  std::vector<float> src(n * kFrame);
  std::vector<float> dst(n * kFrame, -1.f);
  for (int i = 0; i < n * kFrame; ++i) src[i] = static_cast<float>(i);
  tapa::invoke(CornerTurn, FLAGS_bitstream,
               tapa::read_only_mmap<const float>(src),
               tapa::write_only_mmap<float>(dst), n);

  int errors = 0;
  for (int f = 0; f < n; ++f) {
    for (int r = 0; r < kEdge; ++r) {
      for (int c = 0; c < kEdge; ++c) {
        const float expected = src[f * kFrame + r * kEdge + c];
        const float actual = dst[f * kFrame + c * kEdge + r];
        if (actual != expected && errors++ < 10) {
          std::clog << "frame " << f << " (" << r << ", " << c << "): expected "
                    << expected << ", actual " << actual << std::endl;
        }
      }
    }
  }
  std::clog << (errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return errors == 0 ? 0 : 1;
}
