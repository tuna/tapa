// Conformance kernel: exercises [[tapa::target("ignore")]] — a task
// with synth="ignore" that tapacc emits but does not synthesize.

#include <cstdint>

#include <tapa.h>

void Add(tapa::istream<float>& a, tapa::istream<float>& b,
         tapa::ostream<float>& c, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    c << (a.read() + b.read());
  }
}

[[tapa::target("ignore")]] void IgnoreUpper(tapa::istream<float>& a,
                                            tapa::istream<float>& b,
                                            tapa::ostream<float>& c,
                                            uint64_t n) {
  tapa::task().invoke(Add, a, b, c, n);
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

void IgnoreTop(tapa::mmap<const float> a, tapa::mmap<const float> b,
               tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a"), b_q("b"), c_q("c");
  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(IgnoreUpper, a_q, b_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
}
