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

  const uint64_t n = argc > 1 ? atoll(argv[1]) : kChanSize / 2;
  if (n > kChanSize / 2) {
    std::clog << "n must be at most " << kChanSize / 2 << std::endl;
    return 1;
  }
  std::vector<float> src[M];
  for (int r = 0; r < M; ++r) {
    for (uint64_t i = 0; i < n; ++i) src[r].push_back(r * 10000 + i);
  }
  std::vector<float> dst(M * kChanSize, 1.f);
  tapa::invoke(Top, FLAGS_bitstream, tapa::read_only_mmaps<float, M>(src),
               tapa::read_write_mmap<float>(dst), n);

  uint64_t errors = 0;
  for (int chan = 0; chan < M; ++chan) {
    for (int64_t i = 0; i < kChanSize; ++i) {
      const int64_t k = chan * kChanSize + i;
      const float expected =
          i < static_cast<int64_t>(n) ? 2.f * (chan * 10000 + i)
          : i >= kChanSize / 2 && i < kChanSize / 2 + static_cast<int64_t>(n)
              ? -static_cast<float>(k)
              : 1.f;
      if (dst[k] != expected && errors++ < 10) {
        std::clog << "dst[" << k << "]: expected " << expected << ", got "
                  << dst[k] << std::endl;
      }
    }
  }
  std::clog << (errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return errors == 0 ? 0 : 1;
}
