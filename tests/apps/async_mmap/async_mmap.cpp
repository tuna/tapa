// Conformance kernel: exercises tapa::async_mmap port category.
// Minimal design — tapacc only needs the type in the signature to
// classify the port; the body is preserved verbatim as `code`.

#include <cstdint>

#include <tapa.h>

void AsyncReader(tapa::async_mmap<float>& mem, uint64_t n,
                 tapa::ostream<float>& out) {
  for (uint64_t i = 0; i < n; ++i) {
    out << float(0);
  }
}

void AsyncTop(tapa::mmap<float> mem, uint64_t n, tapa::stream<float>& out) {
  tapa::task().invoke(AsyncReader, mem, n, out);
}
