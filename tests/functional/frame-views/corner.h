#pragma once

#include <tapa.h>

// Corner turns of 32 x 32 matrices: each frame of a stream is written
// transposed.
constexpr int kEdge = 32;
constexpr int kFrame = kEdge * kEdge;

void CornerTurn(tapa::mmap<const float> src, tapa::mmap<float> dst, int n);
