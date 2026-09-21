// Shared header for the multi-file test app, included by both translation
// units. It declares every task, DEFINES the template task that the top
// instantiates, and declares the plain helper that a.cpp defines and b.cpp
// calls across the TU boundary.

#pragma once

#include <cstdint>

#include <tapa.h>

// Leaf task defined in a.cpp; emits both operands as streams.
void Produce(tapa::mmap<const float> a, tapa::mmap<const float> b, uint64_t n,
             tapa::ostream<float>& a_q, tapa::ostream<float>& b_q);

// Leaf task defined in b.cpp. Only this declaration is visible where a.cpp
// invokes it, so the top's graph spans both translation units.
void Consume(tapa::istream<float>& c_q, tapa::mmap<float> c, uint64_t n);

// Top task defined in a.cpp.
void MultiFileTop(tapa::mmap<const float> a, tapa::mmap<const float> b,
                  tapa::mmap<float> c, uint64_t n);

// Template task DEFINED in this header and instantiated by the top.
template <typename T>
void Combine(tapa::istream<T>& a_q, tapa::istream<T>& b_q,
             tapa::ostream<T>& c_q, uint64_t n) {
  for (uint64_t i = 0; i < n; ++i) {
    c_q << (a_q.read() + b_q.read());
  }
}

// Plain helper DEFINED in a.cpp and CALLED from Consume in b.cpp.
float ScaleValue(float value);
