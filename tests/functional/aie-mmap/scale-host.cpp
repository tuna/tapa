#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "scale.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);
  const uint64_t n = argc > 1 ? atoll(argv[1]) : 16 * kTile;
  if (n == 0 || n % (4 * kTile) != 0) {
    std::clog << "n = " << n << " is not a positive multiple of " << 4 * kTile
              << ": each quarter is whole tiles" << std::endl;
    return EXIT_FAILURE;
  }

  std::vector<float, tapa::aligned_allocator<float>> src(n);
  std::vector<float, tapa::aligned_allocator<float>> dst(n, -1);
  for (uint64_t i = 0; i < n; ++i) src[i] = static_cast<float>(i);

  tapa::invoke(Top, FLAGS_bitstream, tapa::read_only_mmap<float>(src),
               tapa::write_only_mmap<float>(dst), n);

  uint64_t errors = 0;
  for (uint64_t i = 0; i < n; ++i) {
    if (dst[i] != 2 * src[i]) {
      if (errors < 5) {
        std::clog << "dst[" << i << "] = " << dst[i] << ", want " << 2 * src[i]
                  << std::endl;
      }
      ++errors;
    }
  }
  std::clog << (errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return errors == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
