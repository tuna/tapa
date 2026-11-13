#pragma once

#include <cstdint>

#include <tapa.h>

constexpr int M = 2;
// Elements in a channel of the hmap: 8 KiB of float.
constexpr int64_t kChanSize = 2048;

void Top(tapa::mmaps<float, M> src, tapa::hmap<float, M, kChanSize> dst,
         uint64_t n);
