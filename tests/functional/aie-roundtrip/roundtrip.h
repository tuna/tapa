#pragma once

#include <cstdint>

#include <tapa.h>

using vec4i = tapa::vec_t<int32_t, 4>;

void RoundTrip(tapa::mmap<vec4i> src, tapa::mmap<vec4i> dst, int n);
