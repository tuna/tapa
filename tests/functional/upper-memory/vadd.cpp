// The top hands its whole mmaps and hmap to the upper task Mid, whose own
// tasks take a region of the mmaps each and share the hmap; a task of the
// top's shares the hmap with Mid.

#include "vadd.h"

void Load(tapa::mmap<float> mem, uint64_t n, tapa::ostream<float>& out) {
  for (uint64_t i = 0; i < n; ++i) out << mem[i];
}

// Channel `chan` of the hmap, from its first element: twice what came in.
void Store(int chan, tapa::istream<float>& in, tapa::mmap<float> mem,
           uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) mem[chan * kChanSize + i] = in.read() * 2;
}

void Mid(tapa::mmaps<float, M> regions, tapa::hmap<float, M, kChanSize> chans,
         uint64_t n) {
  tapa::streams<float, M> q("q");
  tapa::task()
      .invoke<tapa::join, M>(Load, regions, n, q)
      .invoke<tapa::join, M>(Store, tapa::seq(), q, chans, n);
}

// The second half of each channel: minus the element's index.
void Mark(tapa::mmap<float> mem, uint64_t n) {
  for (int chan = 0; chan < M; ++chan) {
    for (uint64_t i = 0; i < n; ++i) {
      const int64_t k = chan * kChanSize + kChanSize / 2 + i;
      mem[k] = -static_cast<float>(k);
    }
  }
}

void Top(tapa::mmaps<float, M> src, tapa::hmap<float, M, kChanSize> dst,
         uint64_t n) {
  tapa::task().invoke(Mid, src, dst, n).invoke(Mark, dst, n);
}
