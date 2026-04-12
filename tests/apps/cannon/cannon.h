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

// Keep the functional fixture small enough that each PE-to-PE exchange stays
// below the native 16-deep inter-PE FIFOs while still exercising the
// scatter/compute/communicate/gather flow across the whole PE grid.
constexpr int kN = p * 3;

void Cannon(tapa::mmap<const float> a_vec, tapa::mmap<const float> b_vec,
            tapa::mmap<float> c_vec, uint64_t n);
