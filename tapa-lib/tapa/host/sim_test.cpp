// Test that software simulation works on macOS via tapa-sim.

#include "tapa/host/tapa.h"

#include <array>
#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdint>
#include <cstdlib>
#include <thread>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

void Add(tapa::istream<int>& in, tapa::ostream<int>& out) {
  int val = in.read();
  out.write(val + 1);
}

void Top(tapa::mmap<int> data, int n) {
  tapa::stream<int> in("in");
  tapa::stream<int> out("out");

  // Write input data to stream
  for (int i = 0; i < n; ++i) {
    in.write(data[i]);
  }

  // Process
  tapa::task().invoke(Add, in, out);

  // Read output
  data[0] = out.read();
}

TEST(TapaSimTest, BasicStreamSimulation) {
  std::vector<int> data = {41};
  Top(tapa::mmap<int>(data.data(), data.size()), 1);
  EXPECT_EQ(data[0], 42);
}

TEST(TapaSimTest, InvokeWithEmptyBitstream) {
  std::vector<int> data = {99};
  auto ns =
      tapa::invoke(Top, /*bitstream=*/"",
                   tapa::read_write_mmap<int>(data.data(), data.size()), 1);
  EXPECT_EQ(data[0], 100);
  EXPECT_GT(ns, 0);
}

void Nop() {}

// A detached upper task still starting when every joined task has finished:
// it constructs its `task` while the invoke shuts the scheduler down.
void LateDetachedUpper() {
  std::this_thread::sleep_for(std::chrono::milliseconds(100));
  tapa::task().invoke<tapa::detach>(Nop);
}

void LateDetachedUpperTop() {
  tapa::task().invoke<tapa::detach>(LateDetachedUpper).invoke(Nop);
}

TEST(TapaSimTest, InvokeEndsWhileADetachedUpperTaskStarts) {
  tapa::invoke(LateDetachedUpperTop, /*bitstream=*/"");
}

void NeverFed(tapa::istream<int>& in) { in.read(); }

void Unfed(tapa::ostream<int>&) {}

// A detached upper task whose joined child never finishes, as a free-running
// switch does.
void DetachedUpper() {
  tapa::stream<int> q("never_fed");
  tapa::task().invoke(NeverFed, q).invoke<tapa::detach>(Unfed, q);
}

void Increment(tapa::mmap<int> data) { ++data[0]; }

void DetachedUpperTop(tapa::mmap<int> data) {
  tapa::task().invoke<tapa::detach>(DetachedUpper).invoke(Increment, data);
}

// Hardware waits on no instance under a detached one; neither may an invoke.
TEST(TapaSimTest, DetachedUpperTaskDoesNotHoldUpCompletion) {
  std::vector<int> data = {6};
  tapa::invoke(DetachedUpperTop, /*bitstream=*/"",
               tapa::read_write_mmap<int>(data.data(), data.size()));
  EXPECT_EQ(data[0], 7);
}

std::atomic<bool> idle_task_ended{false};

void Idle(tapa::istream<int>& in) {
  struct OnEnd {
    ~OnEnd() { idle_task_ended = true; }
  } on_end;
  in.read();
}

void IdleTop() {
  tapa::stream<int> q("idle");
  tapa::task().invoke<tapa::detach>(Idle, q);
}

// A detached task ends with its invoke, at the stream operation it waits
// on, rather than outliving it on a thread of its own.
TEST(TapaSimTest, DetachedTasksEndWithTheirInvoke) {
  tapa::invoke(IdleTop, /*bitstream=*/"");
  for (int i = 0; i < 1000 && !idle_task_ended; ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  EXPECT_TRUE(idle_task_ended);
}

volatile std::sig_atomic_t interrupted = 0;

void OnInterrupt(int) { interrupted = 1; }

void NopTop() { tapa::task().invoke(Nop); }

// The scheduler handles SIGINT only while an invoke runs.
TEST(TapaSimDeathTest, InvokeRestoresTheProgramsInterruptHandler) {
  EXPECT_EXIT(
      {
        std::signal(SIGINT, OnInterrupt);
        tapa::invoke(NopTop, /*bitstream=*/"");
        std::raise(SIGINT);
        std::exit(interrupted ? 0 : 1);
      },
      testing::ExitedWithCode(0), "");
}

void AddAt(int idx, tapa::mmap<int> mem) { mem[idx] += 1; }

void MmapsUpper(tapa::mmaps<int, 2> mems) {
  tapa::task().invoke<tapa::join, 2>(AddAt, 1, mems);
}

// An upper task hands its whole mmaps on to another upper task.
void MmapsTop(tapa::mmaps<int, 2> mems) {
  tapa::task().invoke<tapa::join, 2>(AddAt, 0, mems).invoke(MmapsUpper, mems);
}

TEST(TapaSimTest, UpperTaskHandsItsMmapsToAnother) {
  std::vector<int> first = {1, 10};
  std::vector<int> second = {2, 20};
  const std::array<int*, 2> buffers = {first.data(), second.data()};
  const std::array<uint64_t, 2> sizes = {2, 2};
  tapa::invoke(MmapsTop, /*bitstream=*/"",
               tapa::read_write_mmaps<int, 2>(buffers, sizes));
  EXPECT_EQ(first, (std::vector<int>{2, 11}));
  EXPECT_EQ(second, (std::vector<int>{3, 21}));
}

void Double(tapa::mmap<const int> in, tapa::mmap<int> out) {
  out[0] = in[0] * 2;
}

// A task that only reads a memory may take it as `mmap<const T>` whatever
// its invoker holds, as `T*` converts to `const T*` in hardware.
void ConstMmapUpper(tapa::mmap<int> in, tapa::mmaps<int, 2> ins,
                    tapa::mmaps<int, 3> outs) {
  tapa::task()
      .invoke(Double, in, outs)
      .invoke<tapa::join, 2>(Double, ins, outs);
}

TEST(TapaSimTest, ReadOnlyMmapParametersTakeWritableMemory) {
  std::vector<int> in = {1};
  std::vector<int> ins = {2, 3};
  std::vector<int> outs(3);
  const std::array<int*, 2> in_ptrs = {&ins[0], &ins[1]};
  const std::array<int*, 3> out_ptrs = {&outs[0], &outs[1], &outs[2]};
  const std::array<uint64_t, 3> sizes = {1, 1, 1};
  tapa::invoke(ConstMmapUpper, /*bitstream=*/"", tapa::read_only_mmap<int>(in),
               tapa::read_only_mmaps<int, 2>(in_ptrs, sizes),
               tapa::write_only_mmaps<int, 3>(out_ptrs, sizes));
  EXPECT_EQ(outs, (std::vector<int>{2, 4, 6}));
  // The top itself may take one from the host's writable buffer.
  std::vector<int> out(1);
  tapa::invoke(Double, /*bitstream=*/"", tapa::read_only_mmap<int>(in),
               tapa::write_only_mmap<int>(out));
  EXPECT_EQ(out[0], 2);
}

TEST(TapaSimTest, AlignedAllocatorSupportsContainerReallocation) {
  using Vector = std::vector<int, tapa::aligned_allocator<int>>;
  Vector data = {1, 2, 3};
  data.shrink_to_fit();
  Vector moved;
  moved = std::move(data);
  EXPECT_EQ(moved, (Vector{1, 2, 3}));
}
