#include "scale.h"

// Doubles `n` floats of `src` into `dst`, a tile at a time, requesting the
// next tile before computing this one so the round trip to memory hides
// behind the computation.
[[tapa::target("aie")]] void Scale(tapa::async_mmap<float>& src,
                                   tapa::async_mmap<float>& dst, uint64_t n) {
  const uint64_t tiles = n / kTile;
  float tile[kTile];
  src.read_addr.write({0, kTile});
  for (uint64_t t = 0; t < tiles; ++t) {
    if (t + 1 < tiles) src.read_addr.write({(t + 1) * kTile, kTile});
    src.read_data.read(tile, kTile);
    for (int i = 0; i < kTile; ++i) tile[i] *= 2;
    dst.write_addr.write({t * kTile, kTile});
    dst.write_data.write(tile, kTile);
  }
  for (uint64_t acked = 0; acked < tiles * kTile;) {
    acked += uint64_t{dst.write_resp.read()} + 1;
  }
}

void Top(tapa::mmap<float> src, tapa::mmap<float> dst, uint64_t n) {
  tapa::task().invoke(Scale, src, dst, n);
}
