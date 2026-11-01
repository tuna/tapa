// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/stream.h"

#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ostream>
#include <string>
#include <string_view>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include <tapa/host/private_util.h>
#include <tapa/scoped_log_sink_mock.h>
#include <tapa/scoped_set_env.h>

#if __has_include(<filesystem>)
#include <filesystem>
namespace fs = std::filesystem;
#else
#include <experimental/filesystem>
namespace fs = std::experimental::filesystem;
#endif

namespace tapa {
namespace {

using namespace std::string_view_literals;

using ::tapa::internal::StrCat;
using ::tapa_testing::ScopedLogSinkMock;
using ::tapa_testing::ScopedSetEnv;
using ::testing::HasSubstr;
using ::testing::InSequence;
using ::testing::NiceMock;

constexpr char kEnvVarName[] = "TAPA_STREAM_LOG_DIR";

class StreamLogTest : public testing::Test {
 protected:
  void SetUp() override {
    fs::create_directory(temp_dir_);
    ASSERT_TRUE(fs::is_directory(temp_dir_));
    ASSERT_EQ(setenv(kEnvVarName, temp_dir_.c_str(), /*replace=*/1), 0)
        << std::strerror(errno);
  }

  void TearDown() override {
    EXPECT_EQ(unsetenv(kEnvVarName), 0) << std::strerror(errno);
    fs::remove_all(temp_dir_);
  }

  std::string GetLogContent(std::string_view name) {
    std::ifstream ifs(temp_dir_ / StrCat({name, ".txt"}));
    return std::string((std::istreambuf_iterator<char>(ifs)),
                       std::istreambuf_iterator<char>());
  }

  const testing::TestInfo* const test_info_ =
      testing::UnitTest::GetInstance()->current_test_info();
  const fs::path temp_dir_ =
      fs::temp_directory_path() /
      StrCat({test_info_->test_suite_name(), ".", test_info_->name()});
};

// Primitive types are logged in text format.
TEST_F(StreamLogTest, LoggingPrimitiveTypeSucceeds) {
  tapa::stream<int> data_q("data");
  data_q.write(2333);
  data_q.read();

  EXPECT_EQ(GetLogContent(data_q.get_name()), "2333\n");
}

// If a type does not overload `operator<<`, values are logged in hex format.
struct Foo {
  int foo;
};
TEST_F(StreamLogTest, LoggingCustomTypeInHexFormatSucceeds) {
  tapa::stream<Foo> data_q("data");
  data_q.write(Foo{0x2333});
  data_q.read();

  EXPECT_EQ(GetLogContent(data_q.get_name()), "0x33230000\n");  // Little-endian
}

// If the value type overloads `operator<<`, values are logged in text format.
struct Bar {
  int bar;
};
std::ostream& operator<<(std::ostream& os, const Bar& bar) {
  return os << bar.bar;
}
TEST_F(StreamLogTest, LoggingCustomTypeInTextFormatSucceeds) {
  tapa::stream<Bar> data_q("data");
  data_q.write(Bar{2333});
  data_q.read();

  EXPECT_EQ(GetLogContent(data_q.get_name()), "2333\n");
}

TEST_F(StreamLogTest, LoggingEotSucceeds) {
  tapa::stream<int> data_q("data");
  data_q.write(233);
  data_q.read();
  data_q.close();
  data_q.open();
  data_q.write(2333);
  data_q.read();

  EXPECT_EQ(GetLogContent(data_q.get_name()), "233\n\n2333\n");
}

// The `.words` log holds each element's raw 32-bit words (AI Engine PLIO
// stimulus); EoT tokens and elements that are not whole words are skipped.
struct FloatPair {
  float x, y;
};
TEST_F(StreamLogTest, LoggingRawWordsSkipsEotAndPartialWords) {
  tapa::stream<FloatPair> vec_q("vec");
  vec_q.write(FloatPair{1.f, -2.f});
  vec_q.read();
  vec_q.close();
  vec_q.open();
  tapa::stream<short> short_q("short");
  short_q.write(7);
  short_q.read();

  std::ifstream vec_words(temp_dir_ / "vec.words");
  EXPECT_EQ(std::string(std::istreambuf_iterator<char>(vec_words), {}),
            "1065353216 3221225472\n");
  std::ifstream short_words(temp_dir_ / "short.words");
  EXPECT_EQ(std::string(std::istreambuf_iterator<char>(short_words), {}), "");
}

TEST_F(StreamLogTest, LoggingToNonexistentDirFails) {
  ASSERT_EQ(setenv(kEnvVarName, "/nonexistent", /*replace=*/1), 0)
      << std::strerror(errno);

  tapa::stream<int> data_q("data");
  data_q.write(233);
  data_q.read();

  EXPECT_EQ(GetLogContent(data_q.get_name()), "");
}

TEST(StringifyTest, TapaInternalElemToBinaryString) {
  const internal::elem_t<float> val = {.val = 1.f, .eot = true};
  EXPECT_EQ(internal::frt::ToBinaryString(val), "\0\0\x80?\x1"sv);
}

TEST(StringifyTest, TapaInternalElemFromBinaryString) {
  const internal::elem_t<float> val =
      internal::frt::FromBinaryString<internal::elem_t<float>>(
          "\0\0\x80?\x1"sv);
  EXPECT_TRUE(val.eot);
  EXPECT_EQ(val.val, 1.f);
}

template <typename T>
void CheckStreamFrame() {
  constexpr int kBits = T::width;
  constexpr size_t kDataBytes = (kBits + 7) / 8;
  internal::elem_t<T> token{};
  token.val[0] = true;
  token.val[kBits - 1] = true;
  token.eot = true;
  const std::string bytes = internal::frt::ToBinaryString(token);
  ASSERT_EQ(bytes.size(), kDataBytes + 1);
  EXPECT_EQ(static_cast<unsigned char>(bytes[0]) & 1u, 1u);
  EXPECT_EQ(static_cast<unsigned char>(bytes[kDataBytes - 1]),
            (1u << ((kBits - 1) % 8)) | (kDataBytes == 1 ? 1u : 0u));
  EXPECT_EQ(bytes[kDataBytes], 1);
  const auto decoded =
      internal::frt::FromBinaryString<internal::elem_t<T>>(bytes);
  EXPECT_EQ(decoded.val, token.val);
  EXPECT_TRUE(decoded.eot);

  // Exercise the queue's allocation size, peek and both transfer directions.
  internal::frt::Stream<internal::elem_t<T>> queue(2);
  token.eot = false;
  queue.push(token);
  queue.push({T{}, true});
  EXPECT_EQ(queue.front().val, token.val);
  EXPECT_FALSE(queue.pop().eot);
  EXPECT_TRUE(queue.pop().eot);
  EXPECT_TRUE(queue.empty());
}

TEST(StringifyTest, StreamFramesKeepPayloadAndEotSeparate) {
  CheckStreamFrame<u<7>>();
  CheckStreamFrame<u<17>>();
  CheckStreamFrame<i<17>>();
  CheckStreamFrame<u<544>>();
  CheckStreamFrame<i<544>>();
  CheckStreamFrame<u<512>>();
}

TEST(StreamNameTest, StreamsHaveIndexedNames) {
  tapa::streams<int, 3> data_q("data");

  EXPECT_EQ(data_q[0].get_name(), "data[0]");
  EXPECT_EQ(data_q[1].get_name(), "data[1]");
  EXPECT_EQ(data_q[2].get_name(), "data[2]");
}

template <typename T>
void CheckEmptyStreamReads(const T& value) {
  tapa::stream<T, 1> data_q("empty_reads");
  EXPECT_EQ(data_q.peek(nullptr), T{});
  EXPECT_EQ(data_q.read(nullptr), T{});

  data_q.write(value);
  EXPECT_EQ(data_q.peek(nullptr), value);
  EXPECT_FALSE(data_q.empty());
  EXPECT_EQ(data_q.read(nullptr), value);
  EXPECT_TRUE(data_q.empty());
  EXPECT_EQ(data_q.peek(nullptr), T{});
  EXPECT_EQ(data_q.read(nullptr), T{});

  data_q.close();
  data_q.open();
  EXPECT_EQ(data_q.peek(nullptr), T{});
  EXPECT_EQ(data_q.read(nullptr), T{});
}

TEST(StreamReadTest, EmptyReadsReturnValueInitializedElements) {
  CheckEmptyStreamReads(2333);
  CheckEmptyStreamReads(std::array<int, 3>{1, 2, 3});
}

// A bulk transfer lines values up with elements byte for byte, whether a value
// spans elements, an element holds values, or the two are the same type.
TEST(StreamBulkTest, ValuesLineUpWithElementsByteForByte) {
  using pair_t = std::array<int, 2>;
  const int ints[] = {1, 2, 3, 4};
  const pair_t pairs[] = {{1, 2}, {3, 4}};

  tapa::stream<int, 4> narrow("narrow");
  narrow.write(ints, 4);
  pair_t read_pairs[2];
  narrow.read(read_pairs, 2);
  EXPECT_EQ(read_pairs[0], pairs[0]);
  EXPECT_EQ(read_pairs[1], pairs[1]);

  narrow.write(pairs, 2);
  int read_ints[4];
  narrow.read(read_ints, 4);
  EXPECT_THAT(read_ints, testing::ElementsAreArray(ints));

  tapa::stream<pair_t, 2> wide("wide");
  wide.write(ints, 4);
  EXPECT_EQ(wide.read(), pairs[0]);
  EXPECT_EQ(wide.read(), pairs[1]);

  wide.write(pairs[0]);
  wide.write(pairs[1]);
  wide.read(read_ints, 4);
  EXPECT_THAT(read_ints, testing::ElementsAreArray(ints));
}

TEST(StreamCompletionDeathTest, FinalTransferAfterBlockedPollRemainsUsable) {
  EXPECT_EXIT(
      {
        internal::frt_queue<int> queue(1, "completed_output");
        // The queue is mapped locally; unlink it before the child exits.
        std::remove(queue.get_frt_stream().path().c_str());
        internal::note_frt_instance_scheduled();
        // The blocked poll yielded before the final output arrived and the
        // kernel finished. The guard must recheck current readiness.
        queue.push(42);
        internal::note_frt_instance_finished();
        internal::check_kernel_can_still_serve(
            queue, "empty", &internal::type_erased_queue::empty);
        std::exit(queue.pop() == 42 ? 0 : 1);
      },
      testing::ExitedWithCode(0), "");

  EXPECT_EXIT(
      {
        internal::frt_queue<int> queue(1, "completed_input");
        std::remove(queue.get_frt_stream().path().c_str());
        internal::note_frt_instance_scheduled();
        queue.push(41);
        // The formerly full queue has room after the kernel consumed its
        // last input while the producer was yielded.
        queue.pop();
        internal::note_frt_instance_finished();
        internal::check_kernel_can_still_serve(
            queue, "full", &internal::type_erased_queue::full);
        queue.push(42);
        std::exit(queue.pop() == 42 ? 0 : 1);
      },
      testing::ExitedWithCode(0), "");
}

TEST(StreamCompletionDeathTest, HybridForwarderMarksDestinationDone) {
  EXPECT_DEATH(
      {
        tapa::stream<int> stream("hybrid_done");
        auto use = internal::stream_access::use(stream, /*task_reads=*/true);
        std::function<void()> forward;
        use.frt_path_dropping_eot(forward, [] { return true; });
        forward();
        (void)stream.read();
      },
      "still empty after every kernel instance finished");
}

TEST(StreamCompletionDeathTest, StillBlockedAfterCompletionAborts) {
  EXPECT_DEATH(
      {
        internal::frt_queue<int> queue(1, "empty_after_completion");
        std::remove(queue.get_frt_stream().path().c_str());
        internal::note_frt_instance_scheduled();
        internal::note_frt_instance_finished();
        internal::check_kernel_can_still_serve(
            queue, "empty", &internal::type_erased_queue::empty);
      },
      "still empty after every kernel instance finished");

  EXPECT_DEATH(
      {
        internal::frt_queue<int> queue(1, "full_after_completion");
        std::remove(queue.get_frt_stream().path().c_str());
        internal::note_frt_instance_scheduled();
        queue.push(42);
        internal::note_frt_instance_finished();
        internal::check_kernel_can_still_serve(
            queue, "full", &internal::type_erased_queue::full);
      },
      "still full after every kernel instance finished");
}

TEST(LeftoverLogTest, SingleLeftoverIsReported) {
  NiceMock<ScopedLogSinkMock> log;

  tapa::stream<int> stream("foo");
  stream.write(2333);

  InSequence seq;
  EXPECT_CALL(log,
              Warning(HasSubstr("'foo' destroyed with leftover elements")));
  EXPECT_CALL(log, Warning(HasSubstr("'foo' leftover: 2333")));
}

TEST(LeftoverLogTest, ExcessiveLeftoverIsOmitted) {
  NiceMock<ScopedLogSinkMock> log;
  ScopedSetEnv env("TAPA_STREAM_LEFTOVER_LOG_COUNT", "3");

  tapa::stream<int, 10> stream("foo");
  for (int i = 0; i < 10; ++i) {
    stream.write(i);
  }

  InSequence seq;
  EXPECT_CALL(log,
              Warning(HasSubstr("'foo' destroyed with leftover elements")));
  EXPECT_CALL(log, Warning(HasSubstr("'foo' leftover: "))).Times(3);
  EXPECT_CALL(
      log, Warning("Set TAPA_STREAM_LEFTOVER_LOG_COUNT to log more leftovers"));
}

TEST(LeftoverLogTest, InvalidLeftoverLogCountIsReported) {
  NiceMock<ScopedLogSinkMock> log;
  ScopedSetEnv env("TAPA_STREAM_LEFTOVER_LOG_COUNT", "x");

  tapa::stream<int> stream("foo");
  stream.write(2333);

  EXPECT_CALL(
      log, Error("invalid TAPA_STREAM_LEFTOVER_LOG_COUNT value 'x'; expected a "
                 "positive integer"));
}

}  // namespace
}  // namespace tapa
