// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstdint>

#include <tapa.h>

// What VecAdd writes to c[i], for the shared vadd host to check: Add<1>,
// Relay<0>'s Shift<2> and Shift<0>, then Stream2Mmap's -1, each rounding in
// float. It equals a + b only while the sums stay below 2^24.
#define VADD_EXPECTED(a, b) ((a) + static_cast<int>(b) + 1 + 2 - 2 + -1)

void VecAdd(tapa::mmap<const float> a, tapa::mmap<const float> b,
            tapa::mmap<float> c, uint64_t n);
