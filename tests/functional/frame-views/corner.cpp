#include "corner.h"

// Three turns in a row: the first between two frame ports, fed by an
// element stream; the second from a frame port to an element stream, the
// frame port inside an upper task; the third from an element stream into a
// frame port, read as elements again inside an upper task, which declares
// that frame channel itself. TurnStream turns between the top's own stream
// arguments, and TurnElemStream does so through `[[tapa::frame(N)]]` stream
// ports.

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

void Drain(tapa::istream<float>& turned, tapa::mmap<float> dst, int n) {
  tapa::stream<float, 2 * kFrame> cols("cols");
  tapa::task().invoke(TurnIn, turned, cols, n).invoke(Store, cols, dst, n);
}

// Hands the stream it takes to TurnOut's frame port: the frame channel is
// CornerTurn's, and its view reaches TurnOut through this task.
void Emit(tapa::istream<float>& once, tapa::ostream<float>& twice, int n) {
  tapa::task().invoke(TurnOut, once, twice, n);
}

void CornerTurn(tapa::mmap<const float> src, tapa::mmap<float> dst, int n) {
  tapa::stream<float, 2 * kFrame> rows("rows");
  tapa::stream<float, 2 * kFrame> once("once");
  tapa::stream<float, 2> twice("twice");
  tapa::task()
      .invoke(Load, src, rows, n)
      .invoke(Turn, rows, once, n)
      .invoke(Emit, once, twice, n)
      .invoke(Drain, twice, dst, n);
}

// The frame channels of Turn's ports are this top's: each takes its
// elements from or gives them to the stream argument.
void TurnStream(tapa::istream<float>& rows, tapa::ostream<float>& cols, int n) {
  tapa::task().invoke(Turn, rows, cols, n);
}

// Frames as elements: on PL these frame ports are ordinary streams, so the
// task buffers each frame to turn it.
void TurnElems([[tapa::frame(kFrame)]] tapa::istream<float>& in,
               [[tapa::frame(kFrame)]] tapa::ostream<float>& out, int n) {
  float frame[kFrame];
  for (int f = 0; f < n; ++f) {
    [[tapa::pipeline(1)]] for (int i = 0; i < kFrame; ++i) {
      frame[i] = in.read();
    }
    for (int c = 0; c < kEdge; ++c) {
      [[tapa::pipeline(1)]] for (int r = 0; r < kEdge; ++r) {
        out.write(frame[r * kEdge + c]);
      }
    }
  }
}

// The stream arguments reach TurnElems's ordinary stream ports: on Vitis
// each ring moves its frames through a FIFO rather than a frame channel.
void TurnElemStream(tapa::istream<float>& rows, tapa::ostream<float>& cols,
                    int n) {
  tapa::task().invoke(TurnElems, rows, cols, n);
}
