// TU 2: the leaf task that a.cpp invokes across the TU boundary.

#include "multi-file.h"

#include "helper.h"  // out-of-root, reached via -I

void Consume(tapa::istream<float>& c_q, tapa::mmap<float> c, uint64_t n) {
  constexpr uint64_t kLanes = 2;
  const uint64_t tiles = CeilDiv(n, kLanes);
  for (uint64_t tile = 0; tile < tiles; ++tile) {
    for (uint64_t lane = 0; lane < kLanes; ++lane) {
      const uint64_t i = tile * kLanes + lane;
      if (i < n) {
        c[i] = ScaleValue(c_q.read());
      }
    }
  }
}
