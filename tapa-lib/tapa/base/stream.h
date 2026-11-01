// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstddef>
#include <cstdint>

#include <limits>

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

}  // namespace internal

// Keeps every stream access after the fence behind every one before it. The
// AI Engine compiler may move a read ahead of a write in an earlier loop
// (`tapa/aie/stream.h`); HLS and software simulation run separate loops in
// order, so there the fence is a no-op.
#if defined(TAPA_TARGET_AIE_)
void fence();
#else
inline void fence() {}
#endif

}  // namespace tapa
