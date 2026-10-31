#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>

#include "roundtrip.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);

  const int n = argc > 1 ? atoi(argv[1]) : 64;
  std::vector<vec4i> input(n);
  std::vector<vec4i> output(n);
  for (int i = 0; i < n; ++i) {
    for (int lane = 0; lane < vec4i::length; ++lane) {
      input[i].set(lane, static_cast<int32_t>(i * vec4i::length + lane) - 128);
    }
  }

  int errors = 0;
  for (int run = 0; run < 2 && errors == 0; ++run) {
    for (auto& element : output) element.set(-1);
    tapa::invoke(RoundTrip, FLAGS_bitstream, tapa::read_only_mmap<vec4i>(input),
                 tapa::write_only_mmap<vec4i>(output), n);
    for (int i = 0; i < n; ++i) {
      for (int lane = 0; lane < vec4i::length; ++lane) {
        const int32_t expected = 2 * (input[i].get(lane) + 1) + 3;
        if (output[i].get(lane) != expected) ++errors;
      }
    }
    std::clog << "run " << run << ": " << (errors == 0 ? "PASS!" : "FAIL!")
              << std::endl;
  }
  return errors == 0 ? 0 : 1;
}
