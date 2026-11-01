// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <array>
#include <type_traits>

#include "cannon.h"

// Logical values per vector operation. Use 1 for the pre-AIE-style PL
// schedule and 8 for native AIE FP32 vectors; other widths are design-space
// exploration points.
#ifndef TAPA_CANNON_LANES
#define TAPA_CANNON_LANES 1
#endif
constexpr int kLanes = TAPA_CANNON_LANES;
constexpr int kBoundaryLanes = kLanes < 4 ? kLanes : 4;
using boundary_t = tapa::vec_t<float, kBoundaryLanes>;
using row4_t = std::array<float, kBoundaryLanes>;

// Scatter n*n matrix into p*p blocks, each block.
void Scatter(tapa::mmap<const float> matrix_ptr,
             tapa::ostreams<boundary_t, p * p>& block) {
  const uint64_t kNumElems = (kN / p) * (kN / p) / kBoundaryLanes;
  for (int j = 0; j < p * p; ++j) {
    for (uint64_t i = 0; i < kNumElems; ++i) {
      row4_t value;
      for (int lane = 0; lane < kBoundaryLanes; ++lane) {
        value[lane] = *matrix_ptr;
        ++matrix_ptr;
      }
      block[j].write(tapa::bit_cast<boundary_t>(value));
    }
  }
}

void Gather(tapa::mmap<float> matrix_ptr,
            tapa::istreams<boundary_t, p * p>& block) {
  const uint64_t kNumElems = (kN / p) * (kN / p) / kBoundaryLanes;
  for (int j = 0; j < p * p; ++j) {
    for (uint64_t i = 0; i < kNumElems; ++i) {
      const auto value = tapa::bit_cast<row4_t>(block[j].read());
      for (int lane = 0; lane < kBoundaryLanes; ++lane) {
        *matrix_ptr = value[lane];
        ++matrix_ptr;
      }
    }
  }
}

// Each PE holds one kB x kB block of A, B, and C: A as a flat scalar
// array and B and C as native-width vectors, in HLS and on the AI Engine
// alike (`--target ProcElem:aie`). An output vector accumulates in one
// register, one vector multiply-accumulate per k, using the loop schedule
// appropriate to the target. A and B ring as whole rows. Boundary frames use
// at most 128 bits on the AI Engine, leaving
// its two input and two output stream ports to the ring.
using vec_t = std::conditional_t<TAPA_CANNON_LANES == 1, float,
                                 tapa::vec_t<float, TAPA_CANNON_LANES>>;
using row_t = std::array<float, kLanes>;
constexpr int kB = kN / p;
constexpr int kRowVecs = kB / kLanes;
constexpr int kVecs = kB * kRowVecs;
static_assert(kB % kLanes == 0, "a block row must be whole vectors");

inline __attribute__((always_inline)) void ExchangeBlock(
    tapa::ostream<float>& i_prev, tapa::istream<float>& i_next,
    tapa::ostream<float>& j_prev, tapa::istream<float>& j_next,
    float (&a)[kB * kB], float (&b)[kVecs]) {
  [[tapa::pipeline(1)]] [[tapa::tripcount(
      kVecs, kVecs)]] [[tapa::dependence("a")]] [[tapa::dependence("b")]]
  for (uint64_t a_wr = 0, b_wr = 0, a_rd = 0, b_rd = 0;
       a_wr < kVecs || b_wr < kVecs || a_rd < kVecs || b_rd < kVecs;) {
    if (b_wr < kVecs && i_prev.try_write(b[b_wr])) ++b_wr;
    if (a_wr < kVecs && j_prev.try_write(a[a_wr])) ++a_wr;
    if (b_rd < b_wr && i_next.try_read(b[b_rd])) ++b_rd;
    if (a_rd < a_wr && j_next.try_read(a[a_rd])) ++a_rd;
  }
}

template <int N>
inline __attribute__((always_inline)) void ExchangeBlock(
    tapa::ostream<tapa::vec_t<float, N>>& i_prev,
    tapa::istream<tapa::vec_t<float, N>>& i_next,
    tapa::ostream<tapa::vec_t<float, N>>& j_prev,
    tapa::istream<tapa::vec_t<float, N>>& j_next, float (&a)[kB * kB],
    tapa::vec_t<float, N> (&b)[kVecs]) {
  for (int v = 0; v < kVecs; ++v) {
    i_prev.write(b[v]);
    row_t a_row;
    for (int lane = 0; lane < kLanes; ++lane) {
      a_row[lane] = a[v * kLanes + lane];
    }
    j_prev.write(tapa::bit_cast<tapa::vec_t<float, N>>(a_row));
  }
  tapa::fence();
  for (int v = 0; v < kVecs; ++v) {
    b[v] = i_next.read();
    const auto a_row = tapa::bit_cast<row_t>(j_next.read());
    for (int lane = 0; lane < kLanes; ++lane) {
      a[v * kLanes + lane] = a_row[lane];
    }
  }
}

void ProcElem(
    [[tapa::aie_frame(kB * kB /
                      kBoundaryLanes)]] tapa::istream<boundary_t>& a_fifo,
    [[tapa::aie_frame(kB * kB /
                      kBoundaryLanes)]] tapa::istream<boundary_t>& b_fifo,
    [[tapa::aie_frame(kB * kB /
                      kBoundaryLanes)]] tapa::ostream<boundary_t>& c_fifo,
    tapa::ostream<vec_t>& i_prev, tapa::istream<vec_t>& i_next,
    tapa::ostream<vec_t>& j_prev, tapa::istream<vec_t>& j_next) {
  [[tapa::partition("cyclic", 32)]] float a[kB * kB];
  [[tapa::partition("block", 32)]] vec_t b[kVecs];
  [[tapa::partition("cyclic", 32)]] vec_t c[kVecs];

init:
  for (int v = 0; v < kVecs; ++v) {
    row_t b_row;
    for (int half = 0; half < kLanes / kBoundaryLanes; ++half) {
      const auto a_row = tapa::bit_cast<row4_t>(a_fifo.read());
      [[tapa::unroll]] for (int lane = 0; lane < kBoundaryLanes; ++lane) {
        a[v * kLanes + half * kBoundaryLanes + lane] = a_row[lane];
      }
      const auto b_row4 = tapa::bit_cast<row4_t>(b_fifo.read());
      for (int lane = 0; lane < kBoundaryLanes; ++lane) {
        b_row[half * kBoundaryLanes + lane] = b_row4[lane];
      }
    }
    b[v] = tapa::bit_cast<vec_t>(b_row);
    c[v] = 0.f;
  }

outer:
  for (int l = 0; l < p; ++l) {
    [[tapa::pipeline(1)]] [[tapa::dependence("c")]]
    for (int ij = 0; ij < kVecs; ++ij) {
      const int i = ij / kRowVecs;
      const int jv = ij % kRowVecs;
      vec_t acc = c[ij];
      for (int k = 0; k < kB; ++k) {
        acc += tapa::make_vec<kLanes>(a[i * kB + k]) * b[k * kRowVecs + jv];
      }
      c[ij] = acc;
    }
    ExchangeBlock(i_prev, i_next, j_prev, j_next, a, b);
  }

finalize:
  for (int v = 0; v < kVecs; ++v) {
    const auto c_row = tapa::bit_cast<row_t>(c[v]);
    for (int half = 0; half < kLanes / kBoundaryLanes; ++half) {
      row4_t out;
      [[tapa::unroll]] for (int lane = 0; lane < kBoundaryLanes; ++lane) {
        out[lane] = c_row[half * kBoundaryLanes + lane];
      }
      c_fifo.write(tapa::bit_cast<boundary_t>(out));
    }
  }
}

void Cannon(tapa::mmap<const float> a_vec, tapa::mmap<const float> b_vec,
            tapa::mmap<float> c_vec, uint64_t n) {
  assert(kN % p == 0);
  assert(n <= kN);

  tapa::streams<boundary_t, p * p> a("a");
  tapa::streams<boundary_t, p * p> b("b");
  tapa::streams<boundary_t, p * p> c("c");
  tapa::stream<vec_t, kVecs> fifo_00_01("PE00->PE01");
  tapa::stream<vec_t, kVecs> fifo_01_00("PE01->PE00");
  tapa::stream<vec_t, kVecs> fifo_10_11("PE10->PE11");
  tapa::stream<vec_t, kVecs> fifo_11_10("PE11->PE10");
  tapa::stream<vec_t, kVecs> fifo_00_10("PE00->PE10");
  tapa::stream<vec_t, kVecs> fifo_10_00("PE10->PE00");
  tapa::stream<vec_t, kVecs> fifo_01_11("PE01->PE11");
  tapa::stream<vec_t, kVecs> fifo_11_01("PE11->PE01");

  tapa::task()
      .invoke(Scatter, a_vec, a)
      .invoke(Scatter, b_vec, b)
      .invoke(ProcElem, a, b, c, fifo_00_10, fifo_10_00, fifo_00_01, fifo_01_00)
      .invoke(ProcElem, a, b, c, fifo_01_11, fifo_11_01, fifo_01_00, fifo_00_01)
      .invoke(ProcElem, a, b, c, fifo_10_00, fifo_00_10, fifo_10_11, fifo_11_10)
      .invoke(ProcElem, a, b, c, fifo_11_01, fifo_01_11, fifo_11_10, fifo_10_11)
      .invoke(Gather, c_vec, c);
}
