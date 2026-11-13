#include "scalar.h"

void Echo(tapa::mmap<uint64_t> result, tapa::u<80> a, int b, tapa::u<12> c,
          long long d, tapa::i<12> e, tapa::i<80> f, uint64_t g) {
  result[0] = a(63, 0);
  result[1] = a(79, 64);
  result[2] = b;
  result[3] = c;
  result[4] = d;
  result[5] = e;
  result[6] = f(63, 0);
  result[7] = f(79, 64);
  result[8] = g;
}

void ScalarWidth(tapa::mmap<uint64_t> result, tapa::u<80> a, int b,
                 tapa::u<12> c, long long d, tapa::i<12> e, tapa::i<80> f,
                 uint64_t g) {
  tapa::task().invoke(Echo, result, a, b, c, d, e, f, g);
}
