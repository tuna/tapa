// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

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
constexpr int kCols = 4;               // vectors of C an iteration computes
static_assert(kRowVecs % kCols == 0,
              "a block row must be a multiple of four vectors");
using vec_t = tapa::vec_t<float, kLanes>;
// The ring moves blocks in chunks of an iteration's kCols vectors of a row.
using chunk_t = tapa::vec_t<float, kCols * kLanes>;
constexpr int kChunks = kVecs / kCols;  // chunks in a block

// A block crosses a PE's boundary in 128-bit parts of four floats, the
// widest a stream between the PL and the AI Engine carries.
using io_t = tapa::vec_t<float, 4>;
constexpr int kBeats = kB * kB / kBeatFloats;  // memory beats in a block
// A block crosses the boundary as two halves on two streams at once, which
// on the AI Engine are two PLIO channels: a block arrives or leaves in half
// the time one channel takes.
constexpr int kHalfParts = kB * kB / 8;  // io_t parts in half a block

// Scatter the matrix, stored block after block, into its p*p blocks, the
// two halves of each side by side.
void Scatter(tapa::mmap<const beat_t> matrix, tapa::ostreams<io_t, p * p>& lo,
             tapa::ostreams<io_t, p * p>& hi) {
  for (int j = 0; j < p * p; ++j) {
    [[tapa::pipeline(kBeatFloats / 4)]] for (int i = 0; i < kBeats / 2; ++i) {
      const beat_t first = matrix[j * kBeats + i];
      const beat_t second = matrix[j * kBeats + kBeats / 2 + i];
      lo[j].write(&first, 1);
      hi[j].write(&second, 1);
    }
  }
}

void Gather(tapa::mmap<beat_t> matrix, tapa::istreams<io_t, p * p>& lo,
            tapa::istreams<io_t, p * p>& hi) {
  for (int j = 0; j < p * p; ++j) {
    [[tapa::pipeline(kBeatFloats / 4)]] for (int i = 0; i < kBeats / 2; ++i) {
      beat_t first;
      beat_t second;
      lo[j].read(&first, 1);
      hi[j].read(&second, 1);
      matrix[j * kBeats + i] = first;
      matrix[j * kBeats + kBeats / 2 + i] = second;
    }
  }
}

// Each PE holds one kB x kB block of A, B and C, row-major in vectors. Per
// round it adds A * B to C, kCols vectors of a C row at a time, each with its
// own accumulator so that their multiply-accumulates overlap: lane k of A's
// row i times vector j of B's row k. Every round but the last it passes its
// blocks of A and B on to its neighbors and receives theirs once it is done,
// so a ring stream holds a whole block. On the AI Engine every port is a frame
// (`aie_frame`, ignored on PL): the blocks move by DMA, so the PE hands both
// over whole before it computes. On PL it sends the chunks of A and B an
// iteration computes with, and B has a bank per row and per vector of a chunk,
// so the iteration's kCols * kB multiply-accumulates issue in one cycle.
void ProcElem([[tapa::aie_frame(kHalfParts)]] tapa::istream<io_t>& a_lo,
              [[tapa::aie_frame(kHalfParts)]] tapa::istream<io_t>& a_hi,
              [[tapa::aie_frame(kHalfParts)]] tapa::istream<io_t>& b_lo,
              [[tapa::aie_frame(kHalfParts)]] tapa::istream<io_t>& b_hi,
              [[tapa::aie_frame(kHalfParts)]] tapa::ostream<io_t>& c_lo,
              [[tapa::aie_frame(kHalfParts)]] tapa::ostream<io_t>& c_hi,
              [[tapa::aie_frame(kChunks)]] tapa::ostream<chunk_t>& i_prev,
              [[tapa::aie_frame(kChunks)]] tapa::istream<chunk_t>& i_next,
              [[tapa::aie_frame(kChunks)]] tapa::ostream<chunk_t>& j_prev,
              [[tapa::aie_frame(kChunks)]] tapa::istream<chunk_t>& j_next) {
  [[tapa::partition("cyclic", kRowVecs)]] vec_t a[kVecs];
  [[tapa::partition("complete", -1, 1)]] [[tapa::partition("cyclic", kCols, 2)]]
  vec_t b[kB][kRowVecs];
  [[tapa::partition("cyclic", kRowVecs)]] vec_t c[kVecs];

  a_lo.read(a, kVecs / 2);
  a_hi.read(a + kVecs / 2, kVecs / 2);
  b_lo.read(b, kB / 2);
  b_hi.read(b + kB / 2, kB / 2);
  [[tapa::pipeline(1)]] for (int v = 0; v < kVecs; v += kRowVecs) {
    for (int t = 0; t < kRowVecs; ++t) c[v + t] = 0.f;
  }

outer:
  for (int l = 0; l < p; ++l) {
    const bool pass = l + 1 < p;  // the last round's blocks go nowhere
#if defined(TAPA_TARGET_AIE_)
    if (pass) {
      i_prev.write(b, kB);
      j_prev.write(a, kVecs);
    }
#endif
  compute:
    [[tapa::pipeline(1)]] [[tapa::dependence("c")]]
    for (int ij = 0; ij < kVecs; ij += kCols) {
      const int i = ij / kRowVecs;
      const int j = ij % kRowVecs;
#if !defined(TAPA_TARGET_AIE_)
      if (pass) {
        i_prev.write(&b[i][j], kCols);
        j_prev.write(&a[ij], kCols);
      }
#endif
      vec_t c0 = c[ij];
      vec_t c1 = c[ij + 1];
      vec_t c2 = c[ij + 2];
      vec_t c3 = c[ij + 3];
      int k = 0;  // B's row, walked in order
      for (int kv = 0; kv < kRowVecs; ++kv) {
        vec_t a_ik = a[i * kRowVecs + kv];
        for (int lane = 0; lane < kLanes; ++lane) {
          c0 += a_ik[lane] * b[k][j];
          c1 += a_ik[lane] * b[k][j + 1];
          c2 += a_ik[lane] * b[k][j + 2];
          c3 += a_ik[lane] * b[k][j + 3];
          ++k;
        }
      }
      c[ij] = c0;
      c[ij + 1] = c1;
      c[ij + 2] = c2;
      c[ij + 3] = c3;
    }
    if (pass) {
      tapa::fence();  // every send of the round before any receive
      i_next.read(b, kB);
      j_next.read(a, kVecs);
    }
  }

  c_lo.write(c, kVecs / 2);
  c_hi.write(c + kVecs / 2, kVecs / 2);
}

void Cannon(tapa::mmap<const beat_t> a_vec, tapa::mmap<const beat_t> b_vec,
            tapa::mmap<beat_t> c_vec, uint64_t n) {
  assert(kN % p == 0);
  assert(n <= kN);

  // A PE moves a block's halves one after the other while Scatter and Gather
  // move them side by side, so each boundary stream holds a whole half.
  tapa::streams<io_t, p * p, kHalfParts> a_lo("a_lo");
  tapa::streams<io_t, p * p, kHalfParts> a_hi("a_hi");
  tapa::streams<io_t, p * p, kHalfParts> b_lo("b_lo");
  tapa::streams<io_t, p * p, kHalfParts> b_hi("b_hi");
  tapa::streams<io_t, p * p, kHalfParts> c_lo("c_lo");
  tapa::streams<io_t, p * p, kHalfParts> c_hi("c_hi");
  tapa::stream<chunk_t, kChunks> fifo_00_01("PE00->PE01");
  tapa::stream<chunk_t, kChunks> fifo_01_00("PE01->PE00");
  tapa::stream<chunk_t, kChunks> fifo_10_11("PE10->PE11");
  tapa::stream<chunk_t, kChunks> fifo_11_10("PE11->PE10");
  tapa::stream<chunk_t, kChunks> fifo_00_10("PE00->PE10");
  tapa::stream<chunk_t, kChunks> fifo_10_00("PE10->PE00");
  tapa::stream<chunk_t, kChunks> fifo_01_11("PE01->PE11");
  tapa::stream<chunk_t, kChunks> fifo_11_01("PE11->PE01");

  tapa::task()
      .invoke(Scatter, a_vec, a_lo, a_hi)
      .invoke(Scatter, b_vec, b_lo, b_hi)
      .invoke(ProcElem, a_lo, a_hi, b_lo, b_hi, c_lo, c_hi, fifo_00_10,
              fifo_10_00, fifo_00_01, fifo_01_00)
      .invoke(ProcElem, a_lo, a_hi, b_lo, b_hi, c_lo, c_hi, fifo_01_11,
              fifo_11_01, fifo_01_00, fifo_00_01)
      .invoke(ProcElem, a_lo, a_hi, b_lo, b_hi, c_lo, c_hi, fifo_10_00,
              fifo_00_10, fifo_10_11, fifo_11_10)
      .invoke(ProcElem, a_lo, a_hi, b_lo, b_hi, c_lo, c_hi, fifo_11_01,
              fifo_01_11, fifo_11_10, fifo_10_11)
      .invoke(Gather, c_vec, c_lo, c_hi);
}
