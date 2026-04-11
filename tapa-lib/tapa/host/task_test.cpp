// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/task.h"

#include <atomic>
#include <condition_variable>
#include <mutex>
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

struct MockFrtInstance {
  explicit MockFrtInstance(std::atomic<int>& running_count,
                           std::atomic<int>& max_running_count,
                           int slices_to_finish = 20)
      : running_count(running_count),
        max_running_count(max_running_count),
        remaining_slices(slices_to_finish) {}

  ~MockFrtInstance() { Kill(); }

  void WriteToDevice() {}

  void Exec() {
    {
      std::lock_guard<std::mutex> lock(mtx);
      exec_started = true;
      running = true;
    }
    TrackRunningProcess();
    worker = std::thread([this] {
      std::unique_lock<std::mutex> lock(mtx);
      while (!killed && !finished) {
        cv.wait(lock, [this] { return killed || running; });
        if (killed || finished) break;

        lock.unlock();
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
        lock.lock();

        if (!running || killed || finished) continue;
        if (--remaining_slices == 0) {
          finished = true;
          running = false;
          --running_count;
          cv.notify_all();
        }
      }
    });
    cv.notify_all();
  }

  void ReadFromDevice() {}

  void Pause() {
    std::lock_guard<std::mutex> lock(mtx);
    if (!running || finished) return;
    running = false;
    --running_count;
    cv.notify_all();
  }

  void Resume() {
    std::lock_guard<std::mutex> lock(mtx);
    if (running || finished || killed) return;
    running = true;
    TrackRunningProcess();
    cv.notify_all();
  }

  bool IsFinished() {
    std::lock_guard<std::mutex> lock(mtx);
    return finished;
  }

  void Finish() {
    {
      std::unique_lock<std::mutex> lock(mtx);
      cv.wait(lock, [this] { return finished || killed; });
    }
    JoinWorker();
  }

  void Kill() {
    {
      std::lock_guard<std::mutex> lock(mtx);
      killed = true;
      if (running) {
        running = false;
        if (!finished) {
          --running_count;
        }
      }
      cv.notify_all();
    }
    JoinWorker();
  }

  void WaitUntilExecStarts() {
    std::unique_lock<std::mutex> lock(mtx);
    cv.wait(lock, [this] { return exec_started; });
  }

  bool HasStarted() const {
    std::lock_guard<std::mutex> lock(mtx);
    return exec_started;
  }

 private:
  void TrackRunningProcess() {
    const int running_now = ++running_count;
    int prev = max_running_count.load();
    while (running_now > prev &&
           !max_running_count.compare_exchange_weak(prev, running_now)) {
    }
  }

  void JoinWorker() {
    if (worker.joinable()) {
      worker.join();
    }
  }

  std::atomic<int>& running_count;
  std::atomic<int>& max_running_count;
  std::thread worker;
  mutable std::mutex mtx;
  std::condition_variable cv;
  bool exec_started = false;
  bool running = false;
  bool finished = false;
  bool killed = false;
  int remaining_slices;
};

TEST(TaskTest, TapaConcurrencyOneTimeSlicesFrtExecLaunches) {
  tapa_testing::ScopedSetEnv env("TAPA_CONCURRENCY", "1");
  std::atomic<int> running_count = 0;
  std::atomic<int> max_running_count = 0;
  auto first =
      std::make_shared<MockFrtInstance>(running_count, max_running_count);
  auto second =
      std::make_shared<MockFrtInstance>(running_count, max_running_count);

  {
    tapa::task parent;
    tapa::internal::schedule_frt_instance(first);
    tapa::internal::schedule_frt_instance(second);

    first->WaitUntilExecStarts();
    std::this_thread::sleep_for(std::chrono::milliseconds(80));
    EXPECT_TRUE(second->HasStarted());
    EXPECT_EQ(max_running_count.load(), 1);
  }

  EXPECT_EQ(max_running_count.load(), 1);
  EXPECT_EQ(running_count.load(), 0);
}

struct ContinuousRunMockFrtInstance {
  ContinuousRunMockFrtInstance(std::atomic<int>& running_count,
                               std::atomic<int>& max_running_count,
                               std::chrono::milliseconds run_quantum,
                               int quanta_to_finish = 1)
      : running_count(running_count),
        max_running_count(max_running_count),
        run_quantum(run_quantum),
        remaining_quanta(quanta_to_finish) {}

  ~ContinuousRunMockFrtInstance() { Kill(); }

  void WriteToDevice() {}

  void Exec() {
    {
      std::lock_guard<std::mutex> lock(mtx);
      exec_started = true;
      running = true;
    }
    TrackRunningProcess();
    worker = std::thread([this] {
      std::unique_lock<std::mutex> lock(mtx);
      while (!killed && !finished) {
        cv.wait(lock, [this] { return killed || running; });
        if (killed || finished) break;

        const bool interrupted = cv.wait_for(lock, run_quantum, [this] {
          return killed || finished || !running;
        });
        if (interrupted || killed || finished || !running) continue;

        if (--remaining_quanta == 0) {
          finished = true;
          running = false;
          --running_count;
          cv.notify_all();
        }
      }
    });
    cv.notify_all();
  }

  void ReadFromDevice() {}

  void Pause() {
    std::lock_guard<std::mutex> lock(mtx);
    if (!running || finished) return;
    running = false;
    ++pause_count;
    --running_count;
    cv.notify_all();
  }

  void Resume() {
    std::lock_guard<std::mutex> lock(mtx);
    if (running || finished || killed) return;
    running = true;
    ++resume_count;
    TrackRunningProcess();
    cv.notify_all();
  }

  bool IsFinished() {
    std::lock_guard<std::mutex> lock(mtx);
    return finished;
  }

  void Finish() {
    {
      std::unique_lock<std::mutex> lock(mtx);
      cv.wait(lock, [this] { return finished || killed; });
    }
    JoinWorker();
  }

  void Kill() {
    {
      std::lock_guard<std::mutex> lock(mtx);
      killed = true;
      if (running) {
        running = false;
        if (!finished) {
          --running_count;
        }
      }
      cv.notify_all();
    }
    JoinWorker();
  }

  int PauseCount() const { return pause_count.load(); }

  int ResumeCount() const { return resume_count.load(); }

 private:
  void TrackRunningProcess() {
    const int running_now = ++running_count;
    int prev = max_running_count.load();
    while (running_now > prev &&
           !max_running_count.compare_exchange_weak(prev, running_now)) {
    }
  }

  void JoinWorker() {
    if (worker.joinable()) {
      worker.join();
    }
  }

  std::atomic<int>& running_count;
  std::atomic<int>& max_running_count;
  const std::chrono::milliseconds run_quantum;
  std::thread worker;
  mutable std::mutex mtx;
  std::condition_variable cv;
  std::atomic<int> pause_count = 0;
  std::atomic<int> resume_count = 0;
  bool exec_started = false;
  bool running = false;
  bool finished = false;
  bool killed = false;
  int remaining_quanta;
};

TEST(TaskTest, FrtTimesliceEnvAllowsMeaningfulProgress) {
  tapa_testing::ScopedSetEnv concurrency_env("TAPA_CONCURRENCY", "1");
  tapa_testing::ScopedSetEnv timeslice_env("TAPA_FRT_TIMESLICE_MS", "20");
  std::atomic<int> running_count = 0;
  std::atomic<int> max_running_count = 0;
  auto first = std::make_shared<ContinuousRunMockFrtInstance>(
      running_count, max_running_count, std::chrono::milliseconds(10));
  auto second = std::make_shared<ContinuousRunMockFrtInstance>(
      running_count, max_running_count, std::chrono::milliseconds(10));

  std::thread watchdog([first, second] {
    std::this_thread::sleep_for(std::chrono::milliseconds(150));
    first->Kill();
    second->Kill();
  });

  const auto started = std::chrono::steady_clock::now();
  {
    tapa::task parent;
    tapa::internal::schedule_frt_instance(first);
    tapa::internal::schedule_frt_instance(second);
  }
  const auto elapsed = std::chrono::steady_clock::now() - started;
  watchdog.join();

  EXPECT_LT(elapsed, std::chrono::milliseconds(120));
  EXPECT_EQ(max_running_count.load(), 1);
  EXPECT_LE(first->PauseCount() + second->PauseCount(), 1);
  EXPECT_LE(first->ResumeCount() + second->ResumeCount(), 1);
}

}  // namespace
}  // namespace tapa
