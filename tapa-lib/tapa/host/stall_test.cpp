// Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <chrono>
#include <thread>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "tapa/host/coroutine.h"
#include "tapa/scoped_log_sink_mock.h"

namespace {

using ::testing::_;
using ::testing::HasSubstr;

// `note_blocked_poll` only reads the clock every `kStallSampleInterval`
// polls; the constant is private, so drive well past it.
constexpr int kPollsPerSample = 512;

// This target sets TAPA_STALL_WARN_SECONDS in its environment, because the
// threshold is read once per process on the first blocked poll.
constexpr double kThresholdSeconds = 0.05;

void PollBlocked(int times, const char* channel = "ch",
                 const char* state = "empty") {
  for (int i = 0; i < times; ++i) {
    tapa::internal::note_blocked_poll(channel, state);
  }
}

TEST(ParseStallWarnSeconds, UnsetUsesTheTenSecondDefault) {
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds(nullptr), 10000000000ULL);
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds(""), 10000000000ULL);
}

TEST(ParseStallWarnSeconds, ZeroDisablesAndFractionsAreKept) {
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds("0"), 0ULL);
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds("0.5"), 500000000ULL);
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds("30"), 30000000000ULL);
}

TEST(ParseStallWarnSeconds, GarbageWarnsAndFallsBackToTheDefault) {
  tapa_testing::ScopedLogSinkMock log;
  EXPECT_CALL(log, Warning(HasSubstr("TAPA_STALL_WARN_SECONDS"))).Times(2);
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds("10s"), 10000000000ULL);
  EXPECT_EQ(tapa::internal::parse_stall_warn_seconds("-1"), 10000000000ULL);
}

TEST(NoteBlockedPoll, ChannelBlockedPastTheThresholdWarnsExactlyOnce) {
  tapa_testing::ScopedLogSinkMock log;
  EXPECT_CALL(log, Warning(HasSubstr("no stream progress"))).Times(0);

  tapa::internal::note_poll_progress();
  PollBlocked(kPollsPerSample, "fifo_A", "empty");  // arms the stall clock
  ::testing::Mock::VerifyAndClearExpectations(&log);

  std::this_thread::sleep_for(
      std::chrono::duration<double>(kThresholdSeconds * 2));

  // The warning names the channel and its state, and repeats do not re-warn
  // until progress resumes.
  EXPECT_CALL(
      log, Warning(::testing::AllOf(HasSubstr("no stream progress"),
                                    HasSubstr("fifo_A"), HasSubstr("empty"))))
      .Times(1);
  PollBlocked(kPollsPerSample * 4, "fifo_A", "empty");
}

TEST(NoteBlockedPoll, ProgressBeforeTheThresholdKeepsQuiet) {
  tapa_testing::ScopedLogSinkMock log;
  EXPECT_CALL(log, Warning(HasSubstr("no stream progress"))).Times(0);

  // A producer/consumer pair that keeps getting served blocks constantly but
  // never accumulates a stall: every success zeroes the poll count.
  for (int round = 0; round < 8; ++round) {
    tapa::internal::note_poll_progress();
    PollBlocked(kPollsPerSample * 2);
    std::this_thread::sleep_for(
        std::chrono::duration<double>(kThresholdSeconds / 4));
  }
  tapa::internal::note_poll_progress();
}

}  // namespace
