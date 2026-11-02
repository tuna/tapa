#include <cstdint>

#include <tapa.h>

// Every stream element packs four floats, and every task moves elements only
// in bulk, eight floats at a time, so no task builds one itself: the bulk
// transfers pack and unpack them.
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
    // Each stream element spans two rows. Transfer the whole array so HLS
    // retains its banks while bulk reads and writes cross row boundaries.
    float a_floats[kFloats / 2][2];
    float b_floats[kFloats / 2][2];
    float c_floats[kFloats / 2][2];
    a.read(a_floats, kFloats / 2);
    b.read(b_floats, kFloats / 2);
    for (int row = 0; row < kFloats / 2; ++row) {
      for (int col = 0; col < 2; ++col) {
        c_floats[row][col] = a_floats[row][col] + b_floats[row][col];
      }
    }
    c.write(c_floats, kFloats / 2);
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
