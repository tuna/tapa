#pragma once

#include <tapa.h>

// 2-D FFTs of 32 x 32 complex matrices. A matrix travels as 512 vectors of 4
// floats (128 bits, the widest element a PLIO carries): row by row, each row
// as the real then the imaginary parts of its 8 groups of 4 columns. The result
// comes out in bit-reversed order on both axes, column group by column group
// (see fft2d-host.cpp).
constexpr int kEdge = 32;
constexpr int kLanes = 4;
constexpr int kGroups = kEdge / kLanes;
constexpr int kFrame = kEdge * 2 * kGroups;

using vec = tapa::vec_t<float, kLanes>;

void Fft2d(tapa::mmap<const vec> src, tapa::mmap<vec> dst, int n);
