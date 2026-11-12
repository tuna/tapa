#include "tapa/host/private_util.h"

#include <sched.h>

#include <sstream>
#include <string>

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

// Core ids repeat across packages, so a core is a (package, core id) pair:
// two packages of two cores, two hardware threads each, are four, and the
// processors of one package are two.
TEST(PhysicalCoreCount, CountsCoresPerPackageAmongAllowedProcessors) {
  std::string cpuinfo;
  int processor = 0;
  for (int package = 0; package < 2; ++package) {
    for (int core = 0; core < 2; ++core) {
      for (int thread = 0; thread < 2; ++thread) {
        cpuinfo += "processor\t: " + std::to_string(processor++) +
                   "\nphysical id\t: " + std::to_string(package) +
                   "\ncore id\t\t: " + std::to_string(core) + "\n\n";
      }
    }
  }
  std::istringstream every(cpuinfo);
  EXPECT_EQ(count_physical_cores(every, [](int) { return true; }), 4);
  std::istringstream second_package(cpuinfo);
  EXPECT_EQ(count_physical_cores(second_package, [](int p) { return p >= 4; }),
            2);
  std::istringstream unnamed("processor\t: 0\n");
  EXPECT_EQ(count_physical_cores(unnamed, [](int) { return true; }), 0);
}

}  // namespace
}  // namespace tapa::internal
