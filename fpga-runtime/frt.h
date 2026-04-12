// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#ifndef FPGA_RUNTIME_H_
#define FPGA_RUNTIME_H_

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

#include "frt/arg_info.h"
#include "frt/buffer.h"
#include "frt/buffer_arg.h"
#include "frt/stream.h"
#include "frt/stream_arg.h"
#include "frt/stringify.h"  // IWYU pragma: export
#include "frt/tag.h"

namespace fpga {

template <typename T>
using ReadOnlyBuffer = internal::Buffer<T, internal::Tag::kReadOnly>;
template <typename T>
using WriteOnlyBuffer = internal::Buffer<T, internal::Tag::kWriteOnly>;
template <typename T>
using ReadWriteBuffer = internal::Buffer<T, internal::Tag::kReadWrite>;
template <typename T>
using PlaceholderBuffer = internal::Buffer<T, internal::Tag::kPlaceHolder>;

template <typename T>
ReadOnlyBuffer<T> ReadOnly(T* ptr, size_t n) {
  return ReadOnlyBuffer<T>(ptr, n);
}
template <typename T>
WriteOnlyBuffer<T> WriteOnly(T* ptr, size_t n) {
  return WriteOnlyBuffer<T>(ptr, n);
}
template <typename T>
ReadWriteBuffer<T> ReadWrite(T* ptr, size_t n) {
  return ReadWriteBuffer<T>(ptr, n);
}
template <typename T>
PlaceholderBuffer<T> Placeholder(T* ptr, size_t n) {
  return PlaceholderBuffer<T>(ptr, n);
}

template <typename T>
using Stream = internal::Stream<T, internal::Tag::kReadWrite>;

class Instance {
 public:
  explicit Instance(const std::string& bitstream);
  Instance(Instance&&) noexcept;
  Instance& operator=(Instance&&) noexcept;
  ~Instance();

  template <typename T>
  void SetArg(int index, T arg) {
    SetScalarArgRaw(index, &arg, sizeof(arg));
  }

  template <typename T, internal::Tag tag>
  void SetArg(int index, internal::Buffer<T, tag> arg) {
    SetBufferArgRaw(index, tag, internal::BufferArg(arg));
  }

  template <typename T, internal::Tag tag>
  void SetArg(int index, internal::Stream<T, tag>& arg) {
    SetStreamArgRaw(index, tag, arg);
  }

  template <typename... Args>
  void SetArgs(Args&&... args) {
    SetArgImpl(0, std::forward<Args>(args)...);
  }

  size_t SuspendBuf(int index);
  void WriteToDevice();
  void ReadFromDevice();
  void Exec();
  void Pause();
  void Resume();
  void Finish();
  void Kill();
  bool IsFinished() const;

  template <typename... Args>
  Instance& Invoke(Args&&... args) {
    SetArgs(std::forward<Args>(args)...);
    WriteToDevice();
    Exec();
    ReadFromDevice();
    bool has_stream =
        (... || std::is_base_of<internal::StreamArg,
                                std::remove_reference_t<Args>>::value);
    ConditionallyFinish(has_stream);
    return *this;
  }

  std::vector<ArgInfo> GetArgsInfo() const;
  int64_t LoadTimeNanoSeconds() const;
  int64_t ComputeTimeNanoSeconds() const;
  int64_t StoreTimeNanoSeconds() const;
  double LoadTimeSeconds() const;
  double ComputeTimeSeconds() const;
  double StoreTimeSeconds() const;
  double LoadThroughputGbps() const;
  double StoreThroughputGbps() const;

 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;

  void SetScalarArgRaw(int index, const void* arg, size_t size);
  void SetBufferArgRaw(int index, internal::Tag tag, internal::BufferArg arg);
  void SetStreamArgRaw(int index, internal::Tag tag, internal::StreamArg& arg);

  template <typename T, typename... Args>
  void SetArgImpl(int index, T&& arg, Args&&... rest) {
    SetArg(index, std::forward<T>(arg));
    if constexpr (sizeof...(rest) > 0) {
      SetArgImpl(index + 1, std::forward<Args>(rest)...);
    }
  }

  void ConditionallyFinish(bool has_stream);
};

template <typename Arg, typename... Args>
Instance Invoke(const std::string& bitstream, Arg&& arg, Args&&... args) {
  return std::move(Instance(bitstream).Invoke(std::forward<Arg>(arg),
                                              std::forward<Args>(args)...));
}

}  // namespace fpga

#endif  // FPGA_RUNTIME_H_
