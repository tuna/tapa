#include "tapa/host/private_util.h"

#include <sched.h>

#include <gtest/gtest.h>

namespace tapa::internal {
namespace {

// Software simulation runs a coroutine worker per core by default; confined
// to one CPU (as a batch scheduler's allocation confines it), it must not
// start one per core of the whole machine.
TEST(PhysicalCoreCount, CountsOnlyTheCoresThisThreadMayRunOn) {
  cpu_set_t saved;
  ASSERT_EQ(sched_getaffinity(0, sizeof(saved), &saved), 0);
  int cpu = 0;
  while (!CPU_ISSET(cpu, &saved)) ++cpu;
  cpu_set_t one;
  CPU_ZERO(&one);
  CPU_SET(cpu, &one);
  ASSERT_EQ(sched_setaffinity(0, sizeof(one), &one), 0);
  const int count = get_physical_core_count();
  ASSERT_EQ(sched_setaffinity(0, sizeof(saved), &saved), 0);
  EXPECT_EQ(count, 1);
}

}  // namespace
}  // namespace tapa::internal
