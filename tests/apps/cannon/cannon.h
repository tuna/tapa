// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cassert>
#include <cstdint>

#include <tapa.h>

#ifndef TAPA_CANNON_P
#define TAPA_CANNON_P 2
#endif

// p x p PEs
constexpr int p = TAPA_CANNON_P;

// Each PE's block edge; a multiple of four of the PE's vectors
// (`TAPA_CANNON_LANES` floats each). An AI Engine build with eight lanes needs
// `-DTAPA_CANNON_B=32`, which also gives its PEs enough work to measure.
#ifndef TAPA_CANNON_B
#define TAPA_CANNON_B 8
#endif

constexpr int kN = p * TAPA_CANNON_B;

// The matrices move to and from memory in 512-bit beats, as wide as a memory
// port. Aligning a beat to its size lets HLS make each one a single bus word;
// it realigns every access to a `tapa::vec_t<float, 16>`, which is aligned
// only as a float.
struct alignas(64) beat_t {
  float floats[16];
};
constexpr int kBeatFloats = sizeof(beat_t) / sizeof(float);
static_assert((kN / p) * (kN / p) % kBeatFloats == 0,
              "a block must be whole 512-bit beats");

void Cannon(tapa::mmap<const beat_t> a_vec, tapa::mmap<const beat_t> b_vec,
            tapa::mmap<beat_t> c_vec, uint64_t n);
