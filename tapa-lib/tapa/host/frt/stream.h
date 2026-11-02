// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <string>

#include <glog/logging.h>

#include "tapa/host/frt/c_api.h"
#include "tapa/host/frt/types.h"

namespace tapa {
namespace internal {
namespace frt {

struct StreamFfiContext {
  std::string path;
};

template <typename T>
class StreamBase : public StreamArg {
 public:
  explicit StreamBase(uint64_t depth) : StreamBase(MakeInit(depth)) {}

  ~StreamBase() {
    if (handle_ != nullptr) {
      frt_shmq_destroy(handle_);
      handle_ = nullptr;
    }
  }

  StreamBase(const StreamBase&) = delete;
  StreamBase& operator=(const StreamBase&) = delete;

  // Shared-memory path used to hand this stream to the FRT C ABI.
  std::string path() const {
    return this->template get<StreamFfiContext>().path;
  }

 protected:
  using Bytes = std::array<char, StreamCodec<T>::kBytes>;

  bool empty() const {
    int ret = frt_shmq_empty(handle_);
    CHECK_GE(ret, 0) << "frt_shmq_empty failed";
    return ret != 0;
  }

  bool full() const {
    int ret = frt_shmq_full(handle_);
    CHECK_GE(ret, 0) << "frt_shmq_full failed";
    return ret != 0;
  }

  void push_bytes(const Bytes& bytes) {
    CHECK_EQ(
        frt_shmq_push(handle_, reinterpret_cast<const uint8_t*>(bytes.data()),
                      bytes.size()),
        0)
        << "frt_shmq_push failed";
  }

  Bytes front_bytes() const {
    Bytes bytes;
    CHECK_EQ(frt_shmq_front(handle_, reinterpret_cast<uint8_t*>(bytes.data()),
                            bytes.size()),
             0)
        << "frt_shmq_front failed";
    return bytes;
  }

  Bytes pop_bytes() {
    Bytes bytes;
    CHECK_EQ(frt_shmq_pop(handle_, reinterpret_cast<uint8_t*>(bytes.data()),
                          bytes.size()),
             0)
        << "frt_shmq_pop failed";
    return bytes;
  }

 private:
  struct Init {
    StreamFfiContext context;
    void* handle = nullptr;
  };

  explicit StreamBase(const Init& init)
      : StreamArg(init.context), handle_(init.handle) {}

  static Init MakeInit(uint64_t depth) {
    std::array<char, 4096> path_buf{};
    void* handle = frt_shmq_create(
        static_cast<uint32_t>(depth == 0 ? 1 : depth), StreamCodec<T>::kBytes,
        path_buf.data(), path_buf.size());
    CHECK(handle != nullptr) << "failed to create shared-memory stream";
    Init init;
    init.context.path = std::string(path_buf.data());
    init.handle = handle;
    return init;
  }

  void* handle_ = nullptr;
};

template <typename T>
class Stream : public StreamBase<T> {
 public:
  using StreamBase<T>::StreamBase;

  bool empty() const { return StreamBase<T>::empty(); }
  bool full() const { return StreamBase<T>::full(); }
  void push(const T& val) { this->push_bytes(StreamCodec<T>::Encode(val)); }
  T pop() {
    const auto bytes = this->pop_bytes();
    return FromBinaryString<T>({bytes.data(), bytes.size()});
  }
  T front() const {
    const auto bytes = this->front_bytes();
    return FromBinaryString<T>({bytes.data(), bytes.size()});
  }
};

}  // namespace frt
}  // namespace internal
}  // namespace tapa
