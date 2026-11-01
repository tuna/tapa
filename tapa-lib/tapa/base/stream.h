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

// Keeps every stream or memory access after the fence behind every one
// before it. Synthesis orders accesses to different ports only by their
// data: HLS may issue a read in the same cycle as an earlier write, or ahead
// of it, and the AI Engine compiler may move a read above a write in an
// earlier loop. A task whose read waits on its own earlier write, such as
// one in a ring, then deadlocks in hardware, though software simulation,
// where the fence is a no-op, completes. Each target's `stream.h` defines it.
void fence();

}  // namespace tapa
