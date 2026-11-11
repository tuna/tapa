#pragma once

// The AI Engine target of `tapa.h`, selected by `TAPA_TARGET_AIE_`. It
// compiles one AI Engine task's variant of the rewritten tree: that task's
// body sees the stream and vector types below, while every other task is a
// signature stub, so the PL-only port types only need to be declared.

#include <cstddef>
#include <cstdint>

#include "tapa/base/logging_dummy.h"
#include "tapa/base/stream.h"
#include "tapa/base/task.h"
#include "tapa/base/util.h"

#include "tapa/aie/mmap.h"
#include "tapa/aie/stream.h"
#include "tapa/aie/vec.h"

namespace tapa {

template <typename T>
class mmap;
template <typename T, uint64_t S>
class mmaps;
template <typename T, int chan_count, int64_t chan_size>
class hmap;
template <typename T, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
class stream;
template <typename T, uint64_t S, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
class streams;

// The core has no PL clock cycle to yield and no pipeline register to add,
// so these keep only their software-simulation meaning.
inline void wait() noexcept {}
inline void wait(int) noexcept {}

template <typename T, size_t Depth = 1>
T reg(T x) {
  return x;
}

}  // namespace tapa
