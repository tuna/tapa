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

template <typename T>
class Stream {
 public:
  explicit Stream(uint64_t depth) {
    std::array<char, 4096> path_buf{};
    handle_ = frt_shmq_create(static_cast<uint32_t>(depth == 0 ? 1 : depth),
                              StreamCodec<T>::kBytes, path_buf.data(),
                              path_buf.size());
    CHECK(handle_ != nullptr) << "failed to create shared-memory stream";
    path_ = path_buf.data();
  }

  ~Stream() {
    if (handle_ != nullptr) frt_shmq_destroy(handle_);
  }

  Stream(const Stream&) = delete;
  Stream& operator=(const Stream&) = delete;

  // Shared-memory path used to hand this stream to the FRT C ABI.
  const std::string& path() const { return path_; }

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

  std::string path_;
  void* handle_ = nullptr;
};

}  // namespace frt
}  // namespace internal
}  // namespace tapa
