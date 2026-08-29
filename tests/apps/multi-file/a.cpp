// TU 1: the top task, one leaf task, and the cross-TU helper definition.

#include "multi-file.h"

#include "helper.h"  // out-of-root, reached via -I

// User macro wrapping a task invocation, the selective-expansion case: the
// frontend must see through it to the Consume task it invokes.
#define INVOKE_CONSUME(s, m, n) .invoke(Consume, s, m, n)

void Produce(tapa::mmap<const float> a, tapa::mmap<const float> b, uint64_t n,
             tapa::ostream<float>& a_q, tapa::ostream<float>& b_q) {
#ifdef __SYNTHESIS__
  // Both arms emit the same token order per stream; only the traversal
  // width differs, and host g++ compiles without __SYNTHESIS__.
  constexpr uint64_t kLanes = 4;
#else
  constexpr uint64_t kLanes = 1;
#endif
  const uint64_t tiles = CeilDiv(n, kLanes);
  for (uint64_t tile = 0; tile < tiles; ++tile) {
    for (uint64_t lane = 0; lane < kLanes; ++lane) {
      const uint64_t i = tile * kLanes + lane;
      if (i < n) {
        a_q << a[i];
        b_q << b[i];
      }
    }
  }
}

constexpr float kResultScale = 2.f;

// Defined in this TU, called from Consume in b.cpp.
float ScaleValue(float value) { return value * kResultScale; }

void MultiFileTop(tapa::mmap<const float> a, tapa::mmap<const float> b,
                  tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a"), b_q("b"), c_q("c");
  tapa::task()
      .invoke(Produce, a, b, n, a_q, b_q)
      .invoke(Combine<float>, a_q, b_q, c_q, n) INVOKE_CONSUME(c_q, c, n);
}
