// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <chrono>
#include <functional>
#include <iostream>
#include <iterator>
#include <list>
#include <map>
#include <memory>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <vector>

#include "tapa/base/tapa.h"

#include "tapa/host/axis.h"
#include "tapa/host/fixed.h"
#include "tapa/host/coroutine.h"
#include "tapa/host/logging.h"
#include "tapa/host/mmap.h"
#include "tapa/host/stream.h"
#include "tapa/host/task.h"
#include "tapa/host/util.h"
#include "tapa/host/vec.h"

namespace tapa {

/// Invokes @p f; if @p bitstream is non-empty, programs the FPGA and returns
/// kernel time in nanoseconds. If empty, runs software simulation.
template <typename Func, typename... Args>
inline int64_t invoke(Func&& f, const std::string& bitstream, Args&&... args) {
  static_assert(std::is_function_v<std::remove_reference_t<Func>>,
                "the first argument for tapa::invoke() must be a function");
  return internal::invoker<Func>::invoke(std::forward<Func>(f), bitstream,
                                         std::forward<Args>(args)...);
}

template <typename T>
struct aligned_allocator {
  using value_type = T;
  using size_type = std::size_t;
  using difference_type = std::ptrdiff_t;
  using is_always_equal = std::true_type;

  template <typename U>
  void construct(U* ptr) {
    ::new (static_cast<void*>(ptr)) U;
  }
  template <class U, class... Args>
  void construct(U* ptr, Args&&... args) {
    ::new (static_cast<void*>(ptr)) U(std::forward<Args>(args)...);
  }
  T* allocate(size_t count) {
    return reinterpret_cast<T*>(internal::allocate(count * sizeof(T)));
  }
  void deallocate(T* ptr, std::size_t count) {
    internal::deallocate(ptr, count * sizeof(T));
  }
  template <typename U>
  constexpr bool operator==(const aligned_allocator<U>&) const noexcept {
    return true;
  }
  template <typename U>
  constexpr bool operator!=(const aligned_allocator<U>&) const noexcept {
    return false;
  }
};

}  // namespace tapa
