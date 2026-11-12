#pragma once

#include <hls_fence.h>

namespace tapa {
namespace internal {

// Orders every access to `before` ahead of every access to `after`, with
// the port-group fence of Vitis HLS 2024.1 and later. Call it with a
// trailing `0`. Earlier releases only declare
// `hls::fence<BEFORE, AFTER>(ports...)`, which does not hold a frame's
// release behind its bank accesses (2023.2 schedules the release with the
// loop's start), so a frame port is rejected there instead.
template <typename Before, typename After>
auto fence_one_way(Before& before, After& after, int)
    -> decltype(hls::fence({before}, {after})) {
#pragma HLS inline
  hls::fence({before}, {after});
}
template <typename Before, typename After>
void fence_one_way(Before&, After&, long) {
  static_assert(sizeof(Before) == 0,
                "TAPA frame ports need Vitis HLS 2024.1 or newer");
}

}  // namespace internal
}  // namespace tapa
