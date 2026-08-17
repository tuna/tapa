#include "tapa/host/util.h"

#include <gtest/gtest.h>

namespace {

// CPU simulation has no clock: wait() must exist, link, and return.
TEST(UtilTest, WaitIsCallableInSimulation) { tapa::wait(); }

// The n-cycle form exists for the same reason as the nullary one: without it
// a program that needs `ap_wait_n(N)` has no portable spelling.
TEST(UtilTest, NCycleWaitIsCallableInSimulation) { tapa::wait(80); }

}  // namespace
