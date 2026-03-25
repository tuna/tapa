// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/task.h"

#include <thread>

#include <gtest/gtest.h>

#include "tapa.h"
#include "tapa/scoped_set_env.h"

namespace tapa {
namespace {

constexpr int kN = 5000;

void DataSource(tapa::ostream<int>& data_out_q, int n) {
  for (int i = 0; i < n; ++i) {
    data_out_q.write(i);
  }
}

template <typename T>
void DataSinkTemplated(tapa::istream<T>& data_in_q, int n) {
  for (int i = 0; i < n; ++i) {
    data_in_q.read();
  }
}

void DataSink(tapa::istream<int>& data_in_q, int n) {
  DataSinkTemplated(data_in_q, n);
}

// WARNING: This is not synthesizable.
TEST(TaskTest, YieldingWithoutCoroutineWorks) {
  tapa::stream<int, 2> data_q;
  auto data_out_q_out =
      tapa::internal::template accessor<tapa::ostream<int>&,
                                        tapa::stream<int, 2>&>::access(data_q,
                                                                       false);
  auto data_out_q_in =
      tapa::internal::template accessor<tapa::istream<int>&,
                                        tapa::stream<int, 2>&>::access(data_q,
                                                                       false);
  std::thread t1(DataSource, std::ref(data_out_q_out), kN);
  std::thread t2(DataSink, std::ref(data_out_q_in), kN);
  t1.join();
  t2.join();
}

// WARNING: This is not synthesizable.
TEST(TaskTest, DirectInvocationMixedWithTapaInvocationWorks) {
  tapa::stream<int, kN> data_q;
  DataSource(data_q, kN);
  tapa::task().invoke(DataSink, data_q, kN);
}

// WARNING: This is not synthesizable (yet).
TEST(TaskTest, InvokingTemplatedTaskWorks) {
  tapa::stream<int, kN> data_q;
  tapa::task()
      .invoke(DataSinkTemplated<int>, data_q, kN)
      .invoke(DataSource, data_q, kN);
}

// ---- Characterization tests for the scheduler (guards task.cpp refactors)
// ----

constexpr int kBoundedN = 200;

void BoundedProducer(tapa::ostream<int>& q, int n) {
  for (int i = 0; i < n; ++i) q.write(i);
}

void BoundedConsumer(tapa::istream<int>& in_q, tapa::ostream<int>& out_q,
                     int n) {
  for (int i = 0; i < n; ++i) out_q.write(in_q.read());
}

// Tasks communicating through a capacity-1 stream must yield and resume to
// make progress; this exercises the coroutine yield/resume cycle under
// back-pressure.
TEST(TaskTest, BoundedStreamForcesYield) {
  tapa::stream<int, 1> data_q("bounded");
  tapa::stream<int, kBoundedN> result_q("result");
  tapa::task()
      .invoke(BoundedProducer, data_q, kBoundedN)
      .invoke(BoundedConsumer, data_q, result_q, kBoundedN);
  for (int i = 0; i < kBoundedN; ++i) {
    ASSERT_FALSE(result_q.empty()) << "item " << i << " missing";
    EXPECT_EQ(result_q.read(), i);
  }
  EXPECT_TRUE(result_q.empty());
}

// TAPA_CONCURRENCY=1 forces a single coroutine worker; tasks must still
// make progress through cooperative yielding on the bounded stream.
TEST(TaskTest, TapaConcurrencyOneWorker) {
  tapa_testing::ScopedSetEnv env("TAPA_CONCURRENCY", "1");
  tapa::stream<int, 1> data_q("bounded1w");
  tapa::stream<int, kBoundedN> result_q("result1w");
  tapa::task()
      .invoke(BoundedProducer, data_q, kBoundedN)
      .invoke(BoundedConsumer, data_q, result_q, kBoundedN);
  for (int i = 0; i < kBoundedN; ++i) {
    ASSERT_FALSE(result_q.empty()) << "item " << i << " missing";
    EXPECT_EQ(result_q.read(), i);
  }
  EXPECT_TRUE(result_q.empty());
}

// allocate/deallocate are mmap-backed; verify they return usable memory and
// release cleanly — these stay in task.cpp after any scheduler split.
TEST(TaskTest, AllocateDeallocateSharedMemory) {
  constexpr size_t kSize = 4096;
  void* addr = tapa::internal::allocate(kSize);
  ASSERT_NE(addr, nullptr);
  auto* data = static_cast<int*>(addr);
  data[0] = 0xDEAD;
  data[kSize / sizeof(int) - 1] = 0xBEEF;
  EXPECT_EQ(data[0], 0xDEAD);
  EXPECT_EQ(data[kSize / sizeof(int) - 1], 0xBEEF);
  tapa::internal::deallocate(addr, kSize);
}

// Detached invocations must not block task completion; the top-level task
// destructor must not wait for detached children.
void DetachedNoOp() {}

TEST(TaskTest, DetachedInvokeCompletesWithoutWaiting) {
  // The tapa::task destructor must return promptly without deadlocking,
  // even when detached tasks were scheduled.  No assertion on side effects —
  // the test passes if no deadlock or crash occurs.
  tapa::task().invoke<tapa::detach>(DetachedNoOp);
}

}  // namespace
}  // namespace tapa
