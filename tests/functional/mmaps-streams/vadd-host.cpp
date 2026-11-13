// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <iostream>
#include <limits>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

using std::clog;
using std::endl;
using std::vector;

void VecAdd(tapa::mmaps<float, 4> a, tapa::mmaps<float, 4> b,
            tapa::mmaps<float, 4> c, uint64_t n);

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  const uint64_t n = argc > 1 ? atoll(argv[1]) : 1024 * 1024;
  vector<float> a[4];
  vector<float> b[4];
  vector<float> c[4];
  for (uint16_t i = 0; i < 4; ++i) {
    a[i] = vector<float>(n, 0);
    b[i] = vector<float>(n, 0);
    c[i] = vector<float>(n, 0);
    for (uint64_t j = 0; j < n; ++j) {
      a[i][j] = static_cast<float>(i * j);
      b[i][j] = static_cast<float>(i * j) * 2;
      c[i][j] = 0.f;
    }
  }
  tapa::read_only_mmaps<float, 4> a_memory(a);
  const tapa::read_only_mmaps<float, 4> b_memory(b);
  tapa::write_only_mmaps<float, 4> c_memory(c);
  int64_t kernel_time_ns =
      tapa::invoke(VecAdd, FLAGS_bitstream, a_memory, b_memory, c_memory, n);
  clog << "kernel time: " << kernel_time_ns * 1e-9 << " s" << endl;

  clog.precision(std::numeric_limits<float>::max_digits10);
  uint64_t num_errors = 0;
  const uint64_t threshold = 10;  // only report up to these errors
  for (uint16_t i = 0; i < 4; ++i) {
    for (uint64_t j = 0; j < n; ++j) {
      auto expected = a[i][j] + b[i][j];
      auto actual = c[i][j];
      if (actual != expected) {
        if (num_errors < threshold) {
          clog << "expected: " << expected << ", actual: " << actual << endl;
        } else if (num_errors == threshold) {
          clog << "...";
        }
        ++num_errors;
      }
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
