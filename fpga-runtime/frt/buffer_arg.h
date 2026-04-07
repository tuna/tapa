// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#ifndef FPGA_RUNTIME_BUFFER_ARG_H_
#define FPGA_RUNTIME_BUFFER_ARG_H_

#include <cstddef>

#include "frt/buffer.h"
#include "frt/tag.h"

namespace fpga {
namespace internal {

class BufferArg {
 public:
  template <typename T, Tag tag>
  explicit BufferArg(Buffer<T, tag> buffer)
      : ptr_(const_cast<char*>(reinterpret_cast<const char*>(buffer.Get()))),
        elem_size_(sizeof(T)),
        n_(buffer.Size()) {}

  BufferArg() = default;

  char* Get() const { return ptr_; }
  size_t SizeInCount() const { return n_; }
  size_t SizeInBytes() const { return elem_size_ * n_; }

 private:
  char* ptr_ = nullptr;
  size_t elem_size_ = 0;
  size_t n_ = 0;
};

}  // namespace internal
}  // namespace fpga

#endif  // FPGA_RUNTIME_BUFFER_ARG_H_
