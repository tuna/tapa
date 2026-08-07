#include <cstdlib>
#include <string>

#include <gflags/gflags.h>
#include <gtest/gtest.h>

#include "tapa/scoped_set_env.h"

DECLARE_bool(xsim_save_waveform);
DECLARE_string(cosim_work_dir);
DECLARE_string(cosim_simulator);

namespace tapa::internal::frt {
void ForwardFlagsToEnv();
const char* SimulatorFlag();
}  // namespace tapa::internal::frt

namespace {

TEST(FrtFlagsTest, DefaultFlagsDoNotClearExistingCosimEnv) {
  tapa_testing::ScopedSetEnv save_waveform("FRT_XSIM_SAVE_WAVEFORM", "1");
  tapa_testing::ScopedSetEnv work_dir("FRT_COSIM_WORK_DIR", "/tmp/existing");

  FLAGS_xsim_save_waveform = false;
  FLAGS_cosim_work_dir.clear();

  tapa::internal::frt::ForwardFlagsToEnv();

  ASSERT_NE(std::getenv("FRT_XSIM_SAVE_WAVEFORM"), nullptr);
  EXPECT_STREQ(std::getenv("FRT_XSIM_SAVE_WAVEFORM"), "1");
  ASSERT_NE(std::getenv("FRT_COSIM_WORK_DIR"), nullptr);
  EXPECT_STREQ(std::getenv("FRT_COSIM_WORK_DIR"), "/tmp/existing");
}

TEST(FrtFlagsTest, SimulatorFlagIsTheUsersChoiceOrNothing) {
  // Which backend runs a bitstream is Rust's decision now; this flag only
  // says which simulator to use if one is used at all.
  FLAGS_cosim_simulator.clear();
  EXPECT_EQ(tapa::internal::frt::SimulatorFlag(), nullptr);

  FLAGS_cosim_simulator = "verilator";
  EXPECT_STREQ(tapa::internal::frt::SimulatorFlag(), "verilator");
  FLAGS_cosim_simulator.clear();
}

}  // namespace
