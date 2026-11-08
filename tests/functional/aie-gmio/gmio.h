#pragma once

#include <tapa.h>

// Elements per frame.
constexpr int kFrame = 64;

void Pipeline(tapa::istream<int>& src, tapa::ostream<int>& dst, int factor,
              int n);
