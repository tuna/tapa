#pragma once

// The shape of `hls::fence` in Vitis HLS 2024.1 and later: a one-way fence
// takes two port groups. Records each call in `hls::fake::calls` instead of
// fencing.

#include <vector>

namespace hls {
namespace fake {

struct Call {
  std::vector<const void*> before;
  std::vector<const void*> after;
};

inline std::vector<Call> calls;

}  // namespace fake

namespace internal {

struct fence_group {
  template <class... Args>
  fence_group(Args&&... args) : ports{&args...} {}
  std::vector<const void*> ports;
};

}  // namespace internal

inline void fence(const internal::fence_group& before,
                  const internal::fence_group& after) {
  fake::calls.push_back({before.ports, after.ports});
}

template <class... Args>
void fence(Args&&... args) {
  const std::vector<const void*> ports{&args...};
  fake::calls.push_back({ports, ports});
}

}  // namespace hls
