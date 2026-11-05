// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "host_util.h"

using std::clog;
using std::endl;
using std::vector;

void VecAddShared(tapa::mmap<float> data, uint64_t n);

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  const uint64_t n = argc > 1 ? atoll(argv[1]) : 1024 * 1024;
  vector<float> data(n * 3);
  auto a = data.data();
  auto b = a + n;
  auto c = b + n;
  for (uint64_t i = 0; i < n; ++i) {
    a[i] = static_cast<float>(i);
    b[i] = static_cast<float>(i) * 2;
    c[i] = 0.f;
  }
  const tapa::read_write_mmap<float> memory(data);
  int64_t kernel_time_ns =
      tapa::invoke(VecAddShared, FLAGS_bitstream, memory, n);
  clog << "kernel time: " << kernel_time_ns * 1e-9 << " s" << endl;

  uint64_t num_errors = 0;
  for (uint64_t i = 0; i < n; ++i) {
    auto expected = i * 3;
    auto actual = static_cast<uint64_t>(c[i]);
    if (actual != expected) {
      TAPA_TEST_RECORD_MISMATCH(num_errors, clog, "", expected, actual);
    }
  }
  TAPA_TEST_RETURN_VERDICT(num_errors, clog, clog);
}
