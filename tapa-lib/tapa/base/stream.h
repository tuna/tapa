// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

#include <limits>
#include <type_traits>

#include "tapa/base/util.h"

// The pragmas of the bulk transfers below; like `base/vec.h`'s, they expand
// to nothing on every target but Xilinx HLS.
#if defined(TAPA_TARGET_XILINX_HLS_)
#define TAPA_STREAM_INLINE _Pragma("HLS inline")
#define TAPA_STREAM_PIPELINE _Pragma("HLS pipeline II = 1")
#define TAPA_STREAM_UNROLL _Pragma("HLS unroll")
#else
#define TAPA_STREAM_INLINE
#define TAPA_STREAM_PIPELINE
#define TAPA_STREAM_UNROLL
#endif

namespace tapa {

inline constexpr uint64_t kStreamDefaultDepth = 2;
inline constexpr uint64_t kStreamInfiniteDepth =
    std::numeric_limits<uint64_t>::max();

namespace internal {

template <typename T>
struct [[gnu::packed]] elem_t {
  T val;
  bool eot;
};

// How a bulk transfer lines values of U up with elements of T: byte for byte,
// as `bit_cast` does, so a value spans kPer elements when U is wider and an
// element holds kPer values when T is wider.
template <typename T, typename U>
struct bulk_layout {
  static_assert(sizeof(U) % sizeof(T) == 0 || sizeof(T) % sizeof(U) == 0,
                "a bulk transfer needs the value size and the element size to "
                "divide one another");
  static constexpr bool kWide = sizeof(U) >= sizeof(T);
  static constexpr int kPer =
      kWide ? sizeof(U) / sizeof(T) : sizeof(T) / sizeof(U);
};

// Copy through array indices so HLS preserves the banks of every dimension.
template <typename U>
void bulk_copy(U& dst, const U& src) {
  TAPA_STREAM_INLINE
  if constexpr (std::is_array<U>::value) {
    for (size_t i = 0; i < std::extent<U>::value; ++i) {
      TAPA_STREAM_UNROLL
      bulk_copy(dst[i], src[i]);
    }
  } else {
    dst = src;
  }
}

// `istream<T>::read(dst, n)` on every target with a stream port.
template <typename T, typename U, typename Stream>
void bulk_read(Stream& s, U* dst, int n) {
  TAPA_STREAM_INLINE
  using layout = bulk_layout<T, U>;
  if constexpr (std::is_same<T, U>::value) {
    for (int i = 0; i < n; ++i) {
      TAPA_STREAM_PIPELINE
      dst[i] = s.read();
    }
  } else if constexpr (layout::kWide) {
    for (int i = 0; i < n; ++i) {
      TAPA_STREAM_PIPELINE
      std::array<T, layout::kPer> parts;
      for (int k = 0; k < layout::kPer; ++k) {
        TAPA_STREAM_UNROLL
        parts[k] = s.read();
      }
      // Wrapping U also permits bit_cast to return a C array value.
      const auto value = bit_cast<std::array<U, 1>>(parts);
      bulk_copy(dst[i], value[0]);
    }
  } else {
    for (int i = 0; i < n; i += layout::kPer) {
      TAPA_STREAM_PIPELINE
      const auto values = bit_cast<std::array<U, layout::kPer>>(s.read());
      for (int k = 0; k < layout::kPer; ++k) {
        TAPA_STREAM_UNROLL
        bulk_copy(dst[i + k], values[k]);
      }
    }
  }
}

// `ostream<T>::write(src, n)` on every target with a stream port.
template <typename T, typename U, typename Stream>
void bulk_write(Stream& s, const U* src, int n) {
  TAPA_STREAM_INLINE
  using layout = bulk_layout<T, U>;
  if constexpr (std::is_same<T, U>::value) {
    for (int i = 0; i < n; ++i) {
      TAPA_STREAM_PIPELINE
      s.write(src[i]);
    }
  } else if constexpr (layout::kWide) {
    for (int i = 0; i < n; ++i) {
      TAPA_STREAM_PIPELINE
      std::array<U, 1> value;
      bulk_copy(value[0], src[i]);
      const auto parts = bit_cast<std::array<T, layout::kPer>>(value);
      for (int k = 0; k < layout::kPer; ++k) {
        TAPA_STREAM_UNROLL
        s.write(parts[k]);
      }
    }
  } else {
    for (int i = 0; i < n; i += layout::kPer) {
      TAPA_STREAM_PIPELINE
      std::array<U, layout::kPer> values;
      for (int k = 0; k < layout::kPer; ++k) {
        TAPA_STREAM_UNROLL
        bulk_copy(values[k], src[i + k]);
      }
      s.write(bit_cast<T>(values));
    }
  }
}

}  // namespace internal

// Keeps every stream or memory access after the fence behind every one
// before it. Synthesis orders accesses to different ports only by their
// data: HLS may issue a read in the same cycle as an earlier write, or ahead
// of it, and the AI Engine compiler may move a read above a write in an
// earlier loop. A task whose read waits on its own earlier write, such as
// one in a ring, then deadlocks in hardware, though software simulation,
// where the fence is a no-op, completes. Each target's `stream.h` defines it.
void fence();

}  // namespace tapa
