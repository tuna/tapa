#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "gmio.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");

// The streams hold every frame of a run, so the host fills the input
// before the invoke and drains the output after it.
constexpr int kMaxFrames = 16;

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);
  const int n = argc > 1 ? atoi(argv[1]) : 6;
  const int factor = 3;
  if (n > kMaxFrames) {
    std::clog << "at most " << kMaxFrames << " frames" << std::endl;
    return 1;
  }

  tapa::stream<int, kMaxFrames * kFrame> in("in");
  tapa::stream<int, kMaxFrames * kFrame> out("out");
  for (int i = 0; i < n * kFrame; ++i) in.write(i);
  tapa::invoke(Pipeline, FLAGS_bitstream, in, out, factor, n);

  int errors = 0;
  for (int i = 0; i < n * kFrame; ++i) {
    const int expected = (i + 1) * factor;
    const int actual = out.read();
    if (actual != expected && errors++ < 10) {
      std::clog << "element " << i << ": expected " << expected << ", actual "
                << actual << std::endl;
    }
  }
  std::clog << (errors == 0 ? "PASS!" : "FAIL!") << std::endl;
  return errors == 0 ? 0 : 1;
}
