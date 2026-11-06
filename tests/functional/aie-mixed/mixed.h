#pragma once

#include <tapa.h>

using vec4f = tapa::vec_t<float, 4>;

void Mixed(tapa::mmap<vec4f> a, tapa::mmap<vec4f> b, tapa::mmap<vec4f> c,
           int n);
