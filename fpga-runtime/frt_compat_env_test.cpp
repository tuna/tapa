// Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.

#include <cstdlib>
#include <string>

#include <gflags/gflags.h>
#include <gtest/gtest.h>

#include "tapa/scoped_set_env.h"

DECLARE_bool(xsim_save_waveform);
DECLARE_string(cosim_work_dir);

namespace fpga::internal {
void ForwardFlagsToEnvForTest(const std::string& bitstream);
}  // namespace fpga::internal

namespace {

TEST(FrtCompatEnvTest, DefaultFlagsDoNotClearExistingCosimEnv) {
  tapa_testing::ScopedSetEnv save_waveform("FRT_XSIM_SAVE_WAVEFORM", "1");
  tapa_testing::ScopedSetEnv work_dir("FRT_COSIM_WORK_DIR", "/tmp/existing");

  FLAGS_xsim_save_waveform = false;
  FLAGS_cosim_work_dir.clear();

  fpga::internal::ForwardFlagsToEnvForTest("kernel.xo");

  ASSERT_NE(std::getenv("FRT_XSIM_SAVE_WAVEFORM"), nullptr);
  EXPECT_STREQ(std::getenv("FRT_XSIM_SAVE_WAVEFORM"), "1");
  ASSERT_NE(std::getenv("FRT_COSIM_WORK_DIR"), nullptr);
  EXPECT_STREQ(std::getenv("FRT_COSIM_WORK_DIR"), "/tmp/existing");
}

}  // namespace
