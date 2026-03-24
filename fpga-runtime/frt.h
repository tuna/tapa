// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

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
#include "frt/device.h"
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
  Instance(const std::string& bitstream);

  Instance(Instance&&) = default;
  Instance& operator=(Instance&&) = default;

  ~Instance() {
    if (device_ && !device_->IsFinished()) device_->Kill();
  }

  template <typename T>
  void SetArg(int index, T arg) {
    device_->SetScalarArg(index, &arg, sizeof(arg));
  }

  template <typename T, internal::Tag tag>
  void SetArg(int index, internal::Buffer<T, tag> arg) {
    device_->SetBufferArg(index, tag, arg);
  }

  template <typename T, internal::Tag tag>
  void SetArg(int index, internal::Stream<T, tag>& arg) {
    device_->SetStreamArg(index, tag, arg);
  }

  template <typename... Args>
  void SetArgs(Args&&... args) {
    SetArg(0, std::forward<Args>(args)...);
  }

  // Suspends a buffer transfer; returns number of operations suspended.
  size_t SuspendBuf(int index);

  void WriteToDevice();
  void ReadFromDevice();
  void Exec();
  void Finish();
  void Kill();
  bool IsFinished() const;

  // Shortcut for SetArgs + WriteToDevice + Exec + ReadFromDevice + Finish
  // (Finish is skipped if any stream argument is present).
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
  template <typename T, typename... Args>
  void SetArg(int index, T&& arg, Args&&... other_args) {
    SetArg(index, std::forward<T>(arg));
    SetArg(index + 1, std::forward<Args>(other_args)...);
  }

  void ConditionallyFinish(bool has_stream);

  std::unique_ptr<internal::Device> device_;
};

template <typename Arg, typename... Args>
Instance Invoke(const std::string& bitstream, Arg&& arg, Args&&... args) {
  return std::move(Instance(bitstream).Invoke(std::forward<Arg>(arg),
                                              std::forward<Args>(args)...));
}

}  // namespace fpga

#endif  // FPGA_RUNTIME_H_
