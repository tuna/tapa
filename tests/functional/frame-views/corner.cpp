#include "corner.h"

// Three turns in a row: the first between two frame ports, fed by an
// element stream; the second from a frame port to an element stream; the
// third from an element stream into a frame port, read as elements again.

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

void TurnOut(tapa::iframes<float, kFrame>& in, tapa::ostream<float>& out,
             int n) {
  for (int f = 0; f < n; ++f) {
    auto src = in.acquire();
    for (int c = 0; c < kEdge; ++c) {
      [[tapa::pipeline(1)]] for (int r = 0; r < kEdge; ++r) {
        out.write(src[r * kEdge + c]);
      }
    }
  }
}

void TurnIn(tapa::istream<float>& in, tapa::oframes<float, kFrame>& out,
            int n) {
  for (int f = 0; f < n; ++f) {
    auto dst = out.acquire();
    for (int r = 0; r < kEdge; ++r) {
      [[tapa::pipeline(1)]] for (int c = 0; c < kEdge; ++c) {
        dst[c * kEdge + r] = in.read();
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
  tapa::stream<float, 2> twice("twice");
  tapa::stream<float, 2 * kFrame> cols("cols");
  tapa::task()
      .invoke(Load, src, rows, n)
      .invoke(Turn, rows, once, n)
      .invoke(TurnOut, once, twice, n)
      .invoke(TurnIn, twice, cols, n)
      .invoke(Store, cols, dst, n);
}
