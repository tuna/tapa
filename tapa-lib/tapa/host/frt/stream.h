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
class Stream {
 public:
  explicit Stream(uint64_t depth) : Stream(MakeInit(depth)) {}

  ~Stream() {
    if (handle_ != nullptr) {
      frt_shmq_destroy(handle_);
      handle_ = nullptr;
    }
  }

  Stream(const Stream&) = delete;
  Stream& operator=(const Stream&) = delete;

  // Shared-memory path used to hand this stream to the FRT C ABI.
  std::string path() const { return context_.path; }

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

  void push(const T& val) { push_bytes(StreamCodec<T>::Encode(val)); }
  T pop() { return StreamCodec<T>::Decode(pop_bytes()); }
  T front() const { return StreamCodec<T>::Decode(front_bytes()); }

 private:
  using Bytes = std::array<char, StreamCodec<T>::kBytes>;

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

  struct Init {
    StreamFfiContext context;
    void* handle = nullptr;
  };

  explicit Stream(const Init& init)
      : context_(init.context), handle_(init.handle) {}

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

  StreamFfiContext context_;
  void* handle_ = nullptr;
};

}  // namespace frt
}  // namespace internal
}  // namespace tapa
