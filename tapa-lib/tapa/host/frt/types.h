#pragma once

#include <any>
#include <cstring>
#include <string>
#include <string_view>
#include <utility>

#include <glog/logging.h>

namespace tapa {
namespace internal {
namespace frt {

// Type-erased carrier for a stream's FFI context (its shared-memory path).
class StreamArg {
 public:
  explicit StreamArg(std::any context) : context_(std::move(context)) {}
  StreamArg() = default;
  StreamArg(const StreamArg&) = delete;
  StreamArg& operator=(const StreamArg&) = delete;

  template <typename Context>
  Context get() const {
    return std::any_cast<Context>(context_);
  }

 private:
  std::any context_;
};

template <typename T>
std::string ToBinaryString(const T& val) {
  std::string bytes(sizeof(val), '\0');
  memcpy(bytes.data(), &val, sizeof(val));
  return bytes;
}

template <typename T>
T FromBinaryString(std::string_view bytes) {
  T val;
  CHECK_EQ(bytes.size(), sizeof(val));
  memcpy(&val, bytes.data(), sizeof(val));
  return val;
}

}  // namespace frt
}  // namespace internal
}  // namespace tapa
