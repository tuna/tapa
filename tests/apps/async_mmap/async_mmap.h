#pragma once

#include <cstdint>

#include <tapa.h>

void AsyncTop(tapa::mmap<float> mem, tapa::mmap<float> dst, uint64_t n);
