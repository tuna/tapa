// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <iostream>
#include <vector>

#include <gflags/gflags.h>

#include "bandwidth.h"
#include "host_util.h"

template <typename T>
using vector = std::vector<T, tapa::aligned_allocator<T>>;

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, true);

  const uint64_t n = argc > 1 ? atoll(argv[1]) : 1024 * 1024;
  const uint64_t flags = argc > 2 ? atoll(argv[2]) : 6LL;

  vector<float> chan[kBankCount];
  for (uint64_t i = 0; i < kBankCount; ++i) {
    chan[i].resize(n * Elem::length);
    for (uint64_t j = 0; j < n * Elem::length; ++j) {
      chan[i][j] = i ^ j;
    }
  }

  tapa::invoke(Bandwidth, FLAGS_bitstream,
               tapa::read_write_mmaps<float, kBankCount>(chan)
                   .vectorized<Elem::length>(),
               n, flags);

  if (!((flags & kRead) && (flags & kWrite))) return 0;

  uint64_t num_errors = 0;
  for (uint64_t i = 0; i < kBankCount; ++i) {
    for (uint64_t j = 0; j < n * Elem::length; ++j) {
      int64_t expected = i ^ j;
      int64_t actual = chan[i][j];
      if (actual != expected) {
        TAPA_TEST_RECORD_MISMATCH(num_errors, LOG(ERROR), "", expected, actual);
      }
    }
  }
  TAPA_TEST_RETURN_VERDICT(num_errors, std::clog, LOG(WARNING));
}
