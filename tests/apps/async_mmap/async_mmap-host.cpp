#include <cstdlib>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>

#include "async_mmap.h"

using std::clog;
using std::endl;
using std::vector;

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  const uint64_t n = argc > 1 ? atoll(argv[1]) : 1024 * 1024;
  vector<float> mem(n);
  vector<float> dst(n);
  for (uint64_t i = 0; i < n; ++i) {
    mem[i] = static_cast<float>(i);
    dst[i] = 0.f;
  }

  // `mem` is read through an async_mmap and `dst` written through a plain
  // mmap, but both kernel ports are declared `tapa::mmap<float>`, so both
  // bind as read-write.
  int64_t kernel_time_ns =
      tapa::invoke(AsyncTop, FLAGS_bitstream, tapa::read_write_mmap<float>(mem),
                   tapa::read_write_mmap<float>(dst), n);
  clog << "kernel time: " << kernel_time_ns * 1e-9 << " s" << endl;

  uint64_t num_errors = 0;
  const uint64_t threshold = 10;  // only report up to these errors
  for (uint64_t i = 0; i < n; ++i) {
    auto expected = static_cast<uint64_t>(mem[i]);
    auto actual = static_cast<uint64_t>(dst[i]);
    if (actual != expected) {
      if (num_errors < threshold) {
        clog << "expected: " << expected << ", actual: " << actual << endl;
      } else if (num_errors == threshold) {
        clog << "...";
      }
      ++num_errors;
    }
  }
  if (num_errors == 0) {
    clog << "PASS!" << endl;
  } else {
    if (num_errors > threshold) {
      clog << " (+" << (num_errors - threshold) << " more errors)" << endl;
    }
    clog << "FAIL!" << endl;
  }
  return num_errors > 0 ? 1 : 0;
}
