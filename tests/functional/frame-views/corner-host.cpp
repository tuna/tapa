#include <cstdlib>
#include <iostream>
#include <vector>

#include <gflags/gflags.h>
#include <tapa.h>

#include "corner.h"

DEFINE_string(bitstream, "", "path to bitstream file, run csim if empty");
DEFINE_bool(stream_top, false,
            "with --bitstream, the bitstream's top is TurnStream");

// Whether `dst` holds each frame of `src` transposed; logs the first errors.
bool Check(const std::vector<float>& src, const std::vector<float>& dst,
           int n) {
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
  return errors == 0;
}

bool RunCornerTurn(const std::vector<float>& src, int n) {
  std::vector<float> dst(n * kFrame, -1.f);
  tapa::invoke(CornerTurn, FLAGS_bitstream,
               tapa::read_only_mmap<const float>(src),
               tapa::write_only_mmap<float>(dst), n);
  return Check(src, dst, n);
}

bool RunTurnStream(const std::vector<float>& src, int n) {
  std::vector<float> dst(n * kFrame, -1.f);
  tapa::stream<float> rows("rows");
  tapa::stream<float> cols("cols");
  tapa::task()
      .invoke(
          [&](tapa::ostream<float>& rows) {
            for (float x : src) rows.write(x);
          },
          rows)
      .invoke(TurnStream, tapa::executable(FLAGS_bitstream), rows, cols, n)
      .invoke(
          [&](tapa::istream<float>& cols) {
            for (float& x : dst) x = cols.read();
          },
          cols);
  return Check(src, dst, n);
}

int main(int argc, char* argv[]) {
  gflags::ParseCommandLineFlags(&argc, &argv, /*remove_flags=*/true);
  const int n = argc > 1 ? atoi(argv[1]) : 8;

  std::vector<float> src(n * kFrame);
  for (int i = 0; i < n * kFrame; ++i) src[i] = static_cast<float>(i);
  // Software simulation runs both tops; a bitstream holds one.
  bool ok = true;
  if (FLAGS_bitstream.empty() || !FLAGS_stream_top) {
    ok = RunCornerTurn(src, n) && ok;
  }
  if (FLAGS_bitstream.empty() || FLAGS_stream_top) {
    // Twice: on XRT the second invoke's frames continue the counts of the
    // rings that carry TurnStream's arguments.
    for (int run = 0; run < 2; ++run) ok = RunTurnStream(src, n) && ok;
  }
  std::clog << (ok ? "PASS!" : "FAIL!") << std::endl;
  return ok ? 0 : 1;
}
