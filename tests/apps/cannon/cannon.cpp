// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

// Each PE fences its own ring traffic (`tapa::fence()` below), so an
// AI Engine build need not also fence every stream write.
#define TAPA_AIE_PHASE_ORDERED_STREAMS

#include <array>

#include "cannon.h"

// The one design-space knob: floats per vector operation. Every value runs
// the same source. 1 suits PL, where HLS unrolls a block row into kB
// multiply-accumulates per cycle; 8 fills an AI Engine's fp32 vector unit
// (`--target ProcElem:aie`).
#ifndef TAPA_CANNON_LANES
#define TAPA_CANNON_LANES 1
#endif

constexpr int kLanes = TAPA_CANNON_LANES;
constexpr int kB = kN / p;             // edge of a PE's block
constexpr int kRowVecs = kB / kLanes;  // vectors in a block row
constexpr int kVecs = kB * kRowVecs;   // vectors in a block
static_assert(kB % kLanes == 0, "a block row must be whole vectors");
using vec_t = tapa::vec_t<float, kLanes>;

// A stream between the PL and the AI Engine carries at most 128 bits, so a
// vector crosses a PE's boundary as kParts parts of at most four floats.
constexpr int kIoLanes = kLanes < 4 ? kLanes : 4;
constexpr int kParts = kLanes / kIoLanes;
constexpr int kIoVecs = kVecs * kParts;
using io_t = tapa::vec_t<float, kIoLanes>;
using parts_t = std::array<io_t, kParts>;

// Scatter n*n matrix into p*p blocks, each block.
void Scatter(tapa::mmap<const float> matrix_ptr,
             tapa::ostreams<io_t, p * p>& block) {
  for (int j = 0; j < p * p; ++j) {
    for (int i = 0; i < kIoVecs; ++i) {
      io_t value;
      for (int lane = 0; lane < kIoLanes; ++lane) {
        value[lane] = *matrix_ptr;
        ++matrix_ptr;
      }
      block[j].write(value);
    }
  }
}

void Gather(tapa::mmap<float> matrix_ptr, tapa::istreams<io_t, p * p>& block) {
  for (int j = 0; j < p * p; ++j) {
    for (int i = 0; i < kIoVecs; ++i) {
      const io_t value = block[j].read();
      for (int lane = 0; lane < kIoLanes; ++lane) {
        *matrix_ptr = value[lane];
        ++matrix_ptr;
      }
    }
  }
}

// Each PE holds one kB x kB block of A, B and C, row-major in vectors. Per
// round it adds A * B to C, one vector of C at a time: lane k of A's row i
// times vector j of B's row k. While computing it sends each vector of A
// and B on to its neighbors; it receives theirs once it is done, so a ring
// stream holds a whole block. On the AI Engine the blocks arrive as frames
// (`aie_frame`, ignored on PL), leaving the core's stream ports to the ring.
void ProcElem([[tapa::aie_frame(kIoVecs)]] tapa::istream<io_t>& a_fifo,
              [[tapa::aie_frame(kIoVecs)]] tapa::istream<io_t>& b_fifo,
              [[tapa::aie_frame(kIoVecs)]] tapa::ostream<io_t>& c_fifo,
              tapa::ostream<vec_t>& i_prev, tapa::istream<vec_t>& i_next,
              tapa::ostream<vec_t>& j_prev, tapa::istream<vec_t>& j_next) {
  [[tapa::partition("cyclic", kRowVecs)]] vec_t a[kVecs];
  [[tapa::partition("block", kB)]] vec_t b[kVecs];
  [[tapa::partition("cyclic", kRowVecs)]] vec_t c[kVecs];

init:
  [[tapa::pipeline(1)]] for (int v = 0; v < kVecs; ++v) {
    parts_t a_parts, b_parts;
    for (int part = 0; part < kParts; ++part) {
      a_parts[part] = a_fifo.read();
      b_parts[part] = b_fifo.read();
    }
    a[v] = tapa::bit_cast<vec_t>(a_parts);
    b[v] = tapa::bit_cast<vec_t>(b_parts);
    c[v] = 0.f;
  }

outer:
  for (int l = 0; l < p; ++l) {
  compute:
    [[tapa::pipeline(1)]] [[tapa::dependence("c")]]
    for (int ij = 0; ij < kVecs; ++ij) {
      i_prev.write(b[ij]);
      j_prev.write(a[ij]);
      const int i = ij / kRowVecs;
      const int j = ij % kRowVecs;
      vec_t acc = c[ij];
      for (int kv = 0; kv < kRowVecs; ++kv) {
        vec_t a_ik = a[i * kRowVecs + kv];
        for (int lane = 0; lane < kLanes; ++lane) {
          const int k = kv * kLanes + lane;
          acc += a_ik[lane] * b[k * kRowVecs + j];
        }
      }
      c[ij] = acc;
    }
    tapa::fence();  // every send of the round before any receive
  receive:
    [[tapa::pipeline(1)]] for (int v = 0; v < kVecs; ++v) {
      b[v] = i_next.read();
      a[v] = j_next.read();
    }
  }

finalize:
  [[tapa::pipeline(1)]] for (int v = 0; v < kVecs; ++v) {
    const auto parts = tapa::bit_cast<parts_t>(c[v]);
    for (int part = 0; part < kParts; ++part) c_fifo.write(parts[part]);
  }
}

void Cannon(tapa::mmap<const float> a_vec, tapa::mmap<const float> b_vec,
            tapa::mmap<float> c_vec, uint64_t n) {
  assert(kN % p == 0);
  assert(n <= kN);

  tapa::streams<io_t, p * p> a("a");
  tapa::streams<io_t, p * p> b("b");
  tapa::streams<io_t, p * p> c("c");
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
