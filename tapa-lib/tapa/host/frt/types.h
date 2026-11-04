#pragma once

#include <any>
#include <array>
#include <cstddef>
#include <cstring>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>

#include <glog/logging.h>

#include "tapa/base/int.h"
#include "tapa/base/stream.h"

// Recognize vendor integer payloads without requiring vendor headers on hosts.
template <int W>
struct ap_uint;
template <int W>
struct ap_int;

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
struct StreamWidth : std::integral_constant<int, 0> {};
template <int W>
struct StreamWidth<tapa::u<W>> : std::integral_constant<int, W> {};
template <int W>
struct StreamWidth<tapa::i<W>> : std::integral_constant<int, W> {};
template <int W>
struct StreamWidth<ap_uint<W>> : std::integral_constant<int, W> {};
template <int W>
struct StreamWidth<ap_int<W>> : std::integral_constant<int, W> {};

// Stream transport only: object and mmap layouts remain unchanged.
template <typename T, typename = void>
struct StreamCodec {
  static constexpr size_t kBytes = sizeof(T);

  static std::array<char, kBytes> Encode(const T& val) {
    std::array<char, kBytes> bytes;
    memcpy(bytes.data(), &val, sizeof(val));
    return bytes;
  }

  template <typename Bytes>
  static T Decode(const Bytes& bytes) {
    CHECK_EQ(bytes.size(), kBytes);
    T val;
    memcpy(&val, bytes.data(), sizeof(val));
    return val;
  }
};

template <typename T>
struct StreamCodec<elem_t<T>, std::enable_if_t<(StreamWidth<T>::value > 0)>> {
  static constexpr int kBits = StreamWidth<T>::value;
  static constexpr size_t kDataBytes = (kBits + 7) / 8;
  static constexpr size_t kBytes = kDataBytes + 1;

  static std::array<char, kBytes> Encode(const elem_t<T>& val) {
    std::array<char, kBytes> bytes;
    memcpy(bytes.data(), &val.val, kDataBytes);
    if constexpr (kBits % 8 != 0) {
      bytes[kDataBytes - 1] &= (1u << (kBits % 8)) - 1;
    }
    bytes[kDataBytes] = val.eot;
    return bytes;
  }

  template <typename Bytes>
  static elem_t<T> Decode(const Bytes& bytes) {
    CHECK_EQ(bytes.size(), kBytes);
    elem_t<T> val{};
    memcpy(&val.val, bytes.data(), kDataBytes);
    if constexpr (kBits % 8 != 0) {
      reinterpret_cast<unsigned char*>(&val.val)[kDataBytes - 1] &=
          (1u << (kBits % 8)) - 1;
    }
    val.eot = bytes[kDataBytes] != 0;
    return val;
  }
};

template <typename T>
std::string ToBinaryString(const T& val) {
  const auto bytes = StreamCodec<T>::Encode(val);
  return {bytes.data(), bytes.size()};
}

template <typename T>
T FromBinaryString(std::string_view bytes) {
  return StreamCodec<T>::Decode(bytes);
}

}  // namespace frt
}  // namespace internal
}  // namespace tapa
