#pragma once

#include <cstdint>

#include <tapa.h>

// Elements an AI Engine request moves at once.
constexpr int kTile = 64;

void Top(tapa::mmap<float> src, tapa::mmap<float> dst, uint64_t n);
