// Vector add with `Add` on the AI Engine: PL tasks move memory to and from
// the boundary streams, which become PLIOs. The streams are named after
// their variables, so `tapa aiesim` finds their software-simulation logs.

#include "vadd.h"

// The default test keeps ordinary stream ports. The manual frame test sets
// this to 3, forcing a partial frame at the default element count.
#ifndef TAPA_AIE_TEST_FRAME_SIZE
#define TAPA_AIE_TEST_FRAME_SIZE 0
#endif
#if TAPA_AIE_TEST_FRAME_SIZE > 0
#define TAPA_AIE_TEST_FRAME_ATTR [[tapa::aie_frame(TAPA_AIE_TEST_FRAME_SIZE)]]
#else
#define TAPA_AIE_TEST_FRAME_ATTR
#endif

void Mmap2Stream(tapa::mmap<vec4f> mmap, int n, tapa::ostream<vec4f>& stream) {
  for (int i = 0; i < n; ++i) {
    stream << mmap[i];
  }
}

[[tapa::target("aie")]]
void Add(TAPA_AIE_TEST_FRAME_ATTR tapa::istream<vec4f>& a,
         TAPA_AIE_TEST_FRAME_ATTR tapa::istream<vec4f>& b,
         TAPA_AIE_TEST_FRAME_ATTR tapa::ostream<vec4f>& c, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) {
    c.write(a.read() + b.read());
  }
}

void Stream2Mmap(tapa::istream<vec4f>& stream, tapa::mmap<vec4f> mmap, int n) {
  for (int i = 0; i < n; ++i) {
    stream >> mmap[i];
  }
}

void VecAdd(tapa::mmap<vec4f> a, tapa::mmap<vec4f> b, tapa::mmap<vec4f> c,
            int n) {
  tapa::stream<vec4f> a_q("a_q");
  tapa::stream<vec4f> b_q("b_q");
  tapa::stream<vec4f> c_q("c_q");

  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Add, a_q, b_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
}
