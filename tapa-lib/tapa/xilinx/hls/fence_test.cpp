#include "tapa/xilinx/hls/fence.h"

#include <vector>

#include <gtest/gtest.h>

namespace tapa::internal {
namespace {

// Against a stand-in for the vendor's `hls_fence.h` (see
// `testdata/hls-fence`): a frame's release must wait for its bank's
// accesses, and only for those.
TEST(FenceOneWay, OrdersTheFirstPortBeforeTheSecondOnly) {
  int bank[4] = {};
  int release = 0;
  hls::fake::calls.clear();
  fence_one_way(bank, release, 0);
  ASSERT_EQ(hls::fake::calls.size(), 1u);
  EXPECT_EQ(hls::fake::calls[0].before, std::vector<const void*>{&bank});
  EXPECT_EQ(hls::fake::calls[0].after, std::vector<const void*>{&release});
}

}  // namespace
}  // namespace tapa::internal
