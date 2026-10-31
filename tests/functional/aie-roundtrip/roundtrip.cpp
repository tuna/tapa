#include "roundtrip.h"

void Load(tapa::mmap<vec4i> src, int n, tapa::ostream<vec4i>& dst) {
  for (int i = 0; i < n; ++i) {
    dst.write(src[i]);
  }
}

[[tapa::target("aie")]]
void AddOne(tapa::istream<vec4i>& src, tapa::ostream<vec4i>& dst, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) { dst.write(src.read() + 1); }
}

void Double(tapa::istream<vec4i>& src, tapa::ostream<vec4i>& dst, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) { dst.write(src.read() * 2); }
}

[[tapa::target("aie")]]
void AddThree(tapa::istream<vec4i>& src, tapa::ostream<vec4i>& dst, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i) { dst.write(src.read() + 3); }
}

void Store(tapa::istream<vec4i>& src, tapa::mmap<vec4i> dst, int n) {
  for (int i = 0; i < n; ++i) {
    dst[i] = src.read();
  }
}

void RoundTrip(tapa::mmap<vec4i> src, tapa::mmap<vec4i> dst, int n) {
  tapa::stream<vec4i> src_q("src_q");
  tapa::stream<vec4i> plus_one_q("plus_one_q");
  tapa::stream<vec4i> doubled_q("doubled_q");
  tapa::stream<vec4i> dst_q("dst_q");

  tapa::task()
      .invoke(Load, src, n, src_q)
      .invoke(AddOne, src_q, plus_one_q, n)
      .invoke(Double, plus_one_q, doubled_q, n)
      .invoke(AddThree, doubled_q, dst_q, n)
      .invoke(Store, dst_q, dst, n);
}
