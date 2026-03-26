// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.

#ifndef FPGA_RUNTIME_STREAM_H_
#define FPGA_RUNTIME_STREAM_H_

#include <array>
#include <cstddef>
#include <cstdint>
#include <string>

#include <glog/logging.h>

#include "frt/stream_arg.h"
#include "frt/stringify.h"
#include "frt/tag.h"

extern "C" {
void* frt_shmq_create(uint32_t depth, uint32_t width, char* out_path,
                      size_t out_path_len);
void frt_shmq_destroy(void* handle);
int frt_shmq_empty(const void* handle);
int frt_shmq_full(const void* handle);
int frt_shmq_push(void* handle, const uint8_t* data, size_t len);
int frt_shmq_front(const void* handle, uint8_t* out, size_t len);
int frt_shmq_pop(void* handle, uint8_t* out, size_t len);
}

namespace fpga {
namespace internal {

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

 protected:
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

  void push_bytes(const std::string& bytes) {
    CHECK_EQ(bytes.size(), sizeof(T));
    CHECK_EQ(
        frt_shmq_push(handle_, reinterpret_cast<const uint8_t*>(bytes.data()),
                      bytes.size()),
        0)
        << "frt_shmq_push failed";
  }

  std::string front_bytes() const {
    std::string bytes(sizeof(T), '\0');
    CHECK_EQ(frt_shmq_front(handle_, reinterpret_cast<uint8_t*>(bytes.data()),
                            bytes.size()),
             0)
        << "frt_shmq_front failed";
    return bytes;
  }

  std::string pop_bytes() {
    std::string bytes(sizeof(T), '\0');
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
    void* handle =
        frt_shmq_create(static_cast<uint32_t>(depth == 0 ? 1 : depth),
                        sizeof(T), path_buf.data(), path_buf.size());
    CHECK(handle != nullptr) << "failed to create shared-memory stream";
    Init init;
    init.context.path = std::string(path_buf.data());
    init.handle = handle;
    return init;
  }

  void* handle_ = nullptr;
};

template <typename T, Tag tag>
class Stream;

template <typename T>
class Stream<T, Tag::kReadWrite> : public StreamBase<T> {
 public:
  using StreamBase<T>::StreamBase;

  bool empty() const { return StreamBase<T>::empty(); }
  bool full() const { return StreamBase<T>::full(); }
  void push(const T& val) { this->push_bytes(ToBinaryString(val)); }
  T pop() { return FromBinaryString<T>(this->pop_bytes()); }
  T front() const { return FromBinaryString<T>(this->front_bytes()); }
};

}  // namespace internal
}  // namespace fpga

#endif  // FPGA_RUNTIME_STREAM_H_
