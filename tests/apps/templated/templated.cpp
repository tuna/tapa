// Conformance kernel: exercises template-specialization tasks where
// tapacc emits a demangled readable_name that differs from the task key.

#include <cstdint>

#include <tapa.h>

template <typename T>
void TemplatedPassthrough(tapa::istream<T>& a, tapa::ostream<T>& c,
                          uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    c << a.read();
  }
}

void Mmap2Stream(tapa::mmap<const float> mmap, uint64_t n,
                 tapa::ostream<float>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream << mmap[i];
  }
}

void Stream2Mmap(tapa::istream<float>& stream, tapa::mmap<float> mmap,
                 uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    stream >> mmap[i];
  }
}

void TemplatedTop(tapa::mmap<const float> a, tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a"), c_q("c");
  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(TemplatedPassthrough<float>, a_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
}
