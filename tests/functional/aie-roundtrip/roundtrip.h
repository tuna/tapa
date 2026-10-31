#pragma once

#include <cstdint>

#include <tapa.h>

using vec4i = tapa::vec_t<int32_t, 4>;

void RoundTrip(tapa::mmap<vec4i> input, tapa::mmap<vec4i> output, int n);
