// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#ifndef FPGA_RUNTIME_BUFFER_H_
#define FPGA_RUNTIME_BUFFER_H_

#include <cstddef>
#include <type_traits>

#include <glog/logging.h>

#include "frt/tag.h"

namespace fpga {
namespace internal {

template <typename T, Tag tag>
class Buffer {
 public:
  Buffer(T* ptr, size_t n) : ptr_(ptr), n_(n) {}
  T* Get() const { return ptr_; }
  size_t Size() const { return n_; }
  size_t SizeInBytes() const { return n_ * sizeof(T); }

  template <typename U>
  Buffer<U, tag> Reinterpret() const {
    static_assert(std::is_standard_layout<T>::value,
                  "T must have standard layout");
    static_assert(std::is_standard_layout<U>::value,
                  "U must have standard layout");
    if constexpr (sizeof(U) > sizeof(T)) {
      CHECK_EQ(sizeof(U) % sizeof(T), 0);
      constexpr auto N = sizeof(U) / sizeof(T);
      CHECK_EQ(Size() % N, 0);
    } else if constexpr (sizeof(U) < sizeof(T)) {
      CHECK_EQ(sizeof(T) % sizeof(U), 0);
    }
    CHECK_EQ(reinterpret_cast<size_t>(Get()) % alignof(U), 0);
    return Buffer<U, tag>(reinterpret_cast<U*>(Get()),
                          Size() * sizeof(T) / sizeof(U));
  }

 private:
  T* const ptr_;
  const size_t n_;
};

}  // namespace internal
}  // namespace fpga

#endif  // FPGA_RUNTIME_BUFFER_H_
