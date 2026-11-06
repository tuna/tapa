// A mixed hierarchy: the top makes the AI Engine the default, its memory
// movers are PL, and the upper task Stage mixes an AI Engine Add with a PL
// Double, so the stream between them crosses inside Stage (Stage_0__mid).
#include "mixed.h"

[[tapa::target("hls")]]
void Load(tapa::mmap<vec4f> mem, int n, tapa::ostream<vec4f>& out) {
  for (int i = 0; i < n; ++i) out.write(mem[i]);
}

void Add(tapa::istream<vec4f>& a, tapa::istream<vec4f>& b,
         tapa::ostream<vec4f>& c, int n) {
  [[tapa::pipeline]] for (int i = 0; i < n; ++i)
    c.write(a.read() + b.read());
}

[[tapa::target("hls")]]
void Double(tapa::istream<vec4f>& in, tapa::ostream<vec4f>& out, int n) {
  for (int i = 0; i < n; ++i) {
    const vec4f x = in.read();
    out.write(x + x);
  }
}

void Stage(tapa::istream<vec4f>& a, tapa::istream<vec4f>& b,
           tapa::ostream<vec4f>& c, int n) {
  tapa::stream<vec4f> mid("mid");
  tapa::task().invoke(Add, a, b, mid, n).invoke(Double, mid, c, n);
}

[[tapa::target("hls")]]
void Store(tapa::istream<vec4f>& in, tapa::mmap<vec4f> mem, int n) {
  for (int i = 0; i < n; ++i) mem[i] = in.read();
}

[[tapa::target("aie")]]
void Mixed(tapa::mmap<vec4f> a, tapa::mmap<vec4f> b, tapa::mmap<vec4f> c,
           int n) {
  tapa::stream<vec4f> a_q("a_q");
  tapa::stream<vec4f> b_q("b_q");
  tapa::stream<vec4f> c_q("c_q");
  tapa::task()
      .invoke(Load, a, n, a_q)
      .invoke(Load, b, n, b_q)
      .invoke(Stage, a_q, b_q, c_q, n)
      .invoke(Store, c_q, c, n);
}
