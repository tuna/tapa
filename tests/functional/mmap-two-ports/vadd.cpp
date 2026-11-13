// One task takes one buffer through two ports: it reads the first half
// through one and writes the second half through the other.

#include "vadd.h"

void Copy(tapa::mmap<float> src, tapa::mmap<float> dst, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) dst[n + i] = src[i] * 2;
}

void Top(tapa::mmap<float> mem, uint64_t n) {
  tapa::task().invoke(Copy, mem, mem, n);
}
