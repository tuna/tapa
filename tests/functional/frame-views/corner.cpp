#include "corner.h"

// The three turns in a row move frames between an element stream and a frame
// port, between two frame ports, and back.

void Load(tapa::mmap<const float> src, tapa::ostream<float>& out, int n) {
  [[tapa::pipeline(1)]] for (int i = 0; i < n * kFrame; ++i) {
    out.write(src[i]);
  }
}

void Turn(tapa::iframes<float, kFrame>& in, tapa::oframes<float, kFrame>& out,
          int n) {
  for (int f = 0; f < n; ++f) {
    auto src = in.acquire();
    auto dst = out.acquire();
    for (int r = 0; r < kEdge; ++r) {
      [[tapa::pipeline(1)]] for (int c = 0; c < kEdge; ++c) {
        dst[c * kEdge + r] = src[r * kEdge + c];
      }
    }
  }
}

void Store(tapa::istream<float>& in, tapa::mmap<float> dst, int n) {
  [[tapa::pipeline(1)]] for (int i = 0; i < n * kFrame; ++i) {
    dst[i] = in.read();
  }
}

void CornerTurn(tapa::mmap<const float> src, tapa::mmap<float> dst, int n) {
  tapa::stream<float, 2 * kFrame> rows("rows");
  tapa::stream<float, 2 * kFrame> once("once");
  tapa::stream<float, 2 * kFrame> twice("twice");
  tapa::stream<float, 2 * kFrame> cols("cols");
  tapa::task()
      .invoke(Load, src, rows, n)
      .invoke(Turn, rows, once, n)
      .invoke(Turn, once, twice, n)
      .invoke(Turn, twice, cols, n)
      .invoke(Store, cols, dst, n);
}
