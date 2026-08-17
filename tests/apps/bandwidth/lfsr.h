// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <cstdint>

#include <tapa.h>

template <int width>
class LfsrTaps;

template <>
class LfsrTaps<16> {
 public:
  static tapa::u<16> Value() { return 0x2d; }
};

template <int width>
class Lfsr : public tapa::u<width> {
 public:
  using tapa::u<width>::u;

  Lfsr& operator++() {
    CHECK_NE(*this, 0);
    this->set_bit(0, (*this & LfsrTaps<width>::Value()).xor_reduce());
    this->rrotate(1);
    return *this;
  }

 private:
  Lfsr operator++(int);
  Lfsr operator--(int);
  Lfsr& operator--();
};
