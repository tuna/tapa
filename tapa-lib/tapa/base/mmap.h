#pragma once

#include <cstdint>

namespace tapa {

/// A request for `n` consecutive elements of an `async_mmap`, starting at
/// element `offset`: `mem.read_addr.write({offset, n})` reads them in order
/// on `read_data`, `mem.write_addr.write({offset, n})` takes them in order
/// from `write_data`. An element index on its own requests one element. A
/// burst holds fewer than 2^32 elements.
struct burst {
  burst() = default;
  // A template, so `{i, n}` takes any integer types without narrowing.
  template <typename Offset, typename Count>
  constexpr burst(Offset offset, Count n) : offset(offset), n(n) {}

  int64_t offset = 0;
  int64_t n = 0;
};

}  // namespace tapa
