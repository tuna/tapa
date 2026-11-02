#include <cstdint>

#include <tapa.h>

// Every stream element packs four floats, and every task moves elements only
// in bulk, eight floats at a time, so no task builds one itself: the bulk
// transfers pack and unpack them. `Add` computes where `acquire` and
// `reserve` put the values, which off the AI Engine is the storage it passes.
using packed_t = tapa::vec_t<float, 4>;
constexpr int kFloats = 8;  // per bulk transfer; divides every tested n

void Mmap2Stream(tapa::mmap<const float> mmap, uint64_t n,
                 tapa::ostream<packed_t>& stream) {
  for (uint64_t i = 0; i < n; i += kFloats) {
    float floats[kFloats];
    for (int k = 0; k < kFloats; ++k) floats[k] = mmap[i + k];
    stream.write(floats, kFloats);
  }
}

void Add(tapa::istream<packed_t>& a, tapa::istream<packed_t>& b,
         tapa::ostream<packed_t>& c, uint64_t n) {
  for (uint64_t i = 0; i < n; i += kFloats) {
    float a_floats[kFloats];
    float b_floats[kFloats];
    float c_floats[kFloats];
    const float* a_in = a.acquire(a_floats, kFloats);
    const float* b_in = b.acquire(b_floats, kFloats);
    float* c_out = c.reserve(c_floats, kFloats);
    for (int k = 0; k < kFloats; ++k) c_out[k] = a_in[k] + b_in[k];
    a.release();
    b.release();
    c.commit(c_out, kFloats);
  }
}

void Stream2Mmap(tapa::istream<packed_t>& stream, tapa::mmap<float> mmap,
                 uint64_t n) {
  for (uint64_t i = 0; i < n; i += kFloats) {
    float floats[kFloats];
    stream.read(floats, kFloats);
    for (int k = 0; k < kFloats; ++k) mmap[i + k] = floats[k];
  }
}

void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n) {
  tapa::stream<packed_t> a_q("a");
  tapa::stream<packed_t> b_q("b");
  tapa::stream<packed_t> c_q("c");

  tapa::task()
      .invoke(Mmap2Stream, a, n, a_q)
      .invoke(Mmap2Stream, b, n, b_q)
      .invoke(Add, a_q, b_q, c_q, n)
      .invoke(Stream2Mmap, c_q, c, n);
}
