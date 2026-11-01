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

// Each PE's block edge; a multiple of the PE's vector width
// (`TAPA_CANNON_LANES`). Raise it with `-DTAPA_CANNON_B=32` to give AI Engine
// PEs enough work to measure.
#ifndef TAPA_CANNON_B
#define TAPA_CANNON_B 8
#endif

constexpr int kN = p * TAPA_CANNON_B;

void Cannon(tapa::mmap<const float> a_vec, tapa::mmap<const float> b_vec,
            tapa::mmap<float> c_vec, uint64_t n);
