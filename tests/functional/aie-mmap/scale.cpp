#include "scale.h"

// Doubles quarters [first, first + count) of the `n` floats of `src` into
// `dst`, a tile at a time, requesting the next tile before computing this
// one so the round trip to memory hides behind the computation.
[[tapa::target("aie")]] void Scale(tapa::async_mmap<float>& src,
                                   tapa::async_mmap<float>& dst, uint64_t n,
                                   int first, int count) {
  const uint64_t offset = first * (n / 4);
  const uint64_t tiles = count * (n / 4) / kTile;
  float tile[kTile];
  src.read_addr.write({offset, kTile});
  for (uint64_t t = 0; t < tiles; ++t) {
    if (t + 1 < tiles) src.read_addr.write({offset + (t + 1) * kTile, kTile});
    src.read_data.read(tile, kTile);
    for (int i = 0; i < kTile; ++i) tile[i] *= 2;
    dst.write_addr.write({offset + t * kTile, kTile});
    dst.write_data.write(tile, kTile);
  }
  for (uint64_t acked = 0; acked < tiles * kTile;) {
    acked += uint64_t{dst.write_resp.read()} + 1;
  }
}

// An AI Engine upper task handing on its async_mmap, to one task.
[[tapa::target("aie")]] void Forward(tapa::async_mmap<float>& src,
                                     tapa::async_mmap<float>& dst, uint64_t n) {
  tapa::task().invoke(Scale, src, dst, n, 3, 1);
}

// An AI Engine upper task handing on an mmap: each of its tasks takes it as
// its own async_mmap.
[[tapa::target("aie")]] void Halves(tapa::mmap<float> src,
                                    tapa::mmap<float> dst, uint64_t n) {
  tapa::task().invoke(Scale, src, dst, n, 2, 1).invoke(Forward, src, dst, n);
}

// The first half from the top, the rest through AI Engine upper tasks.
void Top(tapa::mmap<float> src, tapa::mmap<float> dst, uint64_t n) {
  tapa::task().invoke(Scale, src, dst, n, 0, 2).invoke(Halves, src, dst, n);
}
