// Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "vadd.h"

// TEST 1: integer template parameters
template <int offset>
void Add(tapa::istream<float>& a, tapa::istream<int>& b,
         tapa::ostream<float>& c, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    c << (a.read() + b.read() + offset);
  }
}

// TEST 2: type template parameters
template <typename mmap_type, typename stream_type>
void Mmap2Stream(tapa::mmap<mmap_type> mmap, uint64_t n,
                 tapa::ostream<stream_type>& stream) {
  for (uint64_t i = 0; i < n; ++i) {
    stream << mmap[i];
  }
}

// TEST 3: mixed template parameters
template <typename tapa_mmap_type, typename stream_type, int offset>
void Stream2Mmap(tapa::istream<stream_type>& stream, tapa_mmap_type mmap,
                 uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    stream >> mmap[i];
    mmap[i] += offset;
  }
}

// TEST 5: an explicit specialization beside the primary's implicit ones
template <int delta>
void Shift(tapa::istream<float>& in, tapa::ostream<float>& out, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    out << (in.read() + delta);
  }
}

// Undoes Shift<2>, where the primary's Shift<0> would pass values through.
template <>
void Shift<0>(tapa::istream<float>& in, tapa::ostream<float>& out, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    out << (in.read() - 2);
  }
}

// TEST 6: an explicitly specialized upper task, whose body differs from
// what its primary template would invoke.
template <int k>
void Relay(tapa::istream<float>& src, tapa::ostream<float>& dst, uint64_t n) {
  tapa::task().invoke(Shift<k + 1>, src, dst, n);
}

template <>
void Relay<0>(tapa::istream<float>& src, tapa::ostream<float>& dst,
              uint64_t n) {
  tapa::stream<float> mid("mid");
  tapa::task().invoke(Shift<2>, src, mid, n).invoke(Shift<0>, mid, dst, n);
}

void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n) {
  tapa::stream<float> a_q("a");
  tapa::stream<int> b_q("b");
  tapa::stream<float> c_q("c");
  tapa::stream<float> d_q("d");

  tapa::task()
      .invoke(Mmap2Stream<const float, float>, a, n, a_q)
      .invoke(Mmap2Stream<const float, int>, b, n, b_q)
      .invoke(Add<1>, a_q, b_q, c_q, n)
      .invoke(Relay<0>, c_q, d_q, n)
      // TEST 4: tapa::mmap as a template parameter
      .invoke(Stream2Mmap<tapa::mmap<float>, float, -1>, d_q, c, n);
}
