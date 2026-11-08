#include "gmio.h"

// A top with no PL task: both stages run on the AI Engine, and the top's
// stream arguments reach them as frames moved by GMIO, one input through a
// frame port and one output through a `[[tapa::frame(N)]]` stream port.

void AddOne(tapa::iframes<int, kFrame>& in, tapa::oframes<int, kFrame>& out,
            int n) {
  for (int f = 0; f < n; ++f) {
    auto src = in.acquire();
    auto dst = out.acquire();
    for (int i = 0; i < kFrame; ++i) dst[i] = src[i] + 1;
  }
}

void Scale([[tapa::frame(kFrame)]] tapa::istream<int>& in,
           [[tapa::frame(kFrame)]] tapa::ostream<int>& out, int factor, int n) {
  for (int i = 0; i < n * kFrame; ++i) out.write(in.read() * factor);
}

[[tapa::target("aie")]]
void Pipeline(tapa::istream<int>& src, tapa::ostream<int>& dst, int factor,
              int n) {
  tapa::stream<int, 2 * kFrame> mid("mid");
  tapa::task().invoke(AddOne, src, mid, n).invoke(Scale, mid, dst, factor, n);
}
