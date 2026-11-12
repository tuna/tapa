// Test that software simulation works on macOS via tapa-sim.

#include "tapa/host/tapa.h"

#include <chrono>
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

TEST(TapaSimTest, AlignedAllocatorSupportsContainerReallocation) {
  using Vector = std::vector<int, tapa::aligned_allocator<int>>;
  Vector data = {1, 2, 3};
  data.shrink_to_fit();
  Vector moved;
  moved = std::move(data);
  EXPECT_EQ(moved, (Vector{1, 2, 3}));
}
