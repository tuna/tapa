#pragma once

#include <cstdint>

#include <tapa.h>

// Scalars whose declared width is not their object's: HLS gives a W-bit
// scalar ceil(W/32) control words, so `a` takes three and pushes every later
// argument's register past where its object size would put it.
void ScalarWidth(tapa::mmap<uint64_t> result, tapa::u<80> a, int b,
                 tapa::u<12> c, long long d, tapa::i<12> e, tapa::i<80> f,
                 uint64_t g);
