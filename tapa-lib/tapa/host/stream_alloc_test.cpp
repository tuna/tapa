// Guards the simulator's blocked-poll path against two regressions it has
// already had. A producer/consumer pair sharing a shallow channel blocks on
// nearly every element, so anything charged per blocked poll scales with the
// transferred data rather than with the design.
//
//  1. Allocation. A blocked poll yields with a human-readable reason that is
//     only formatted when stream debugging is on. Built eagerly, every poll
//     paid for a `std::string`. Asserted exactly: blocking must cost no more
//     allocations than the same transfer through a channel deep enough never
//     to block.
//
//  2. Sleeping. A thread that cannot proceed backs off. When that backoff was
//     a flat one-millisecond sleep, every blocked poll hit a 1 kHz ceiling --
//     which without coroutines (the macOS default) was the entire runtime of a
//     simulation: 20000 elements took over ten seconds instead of ten
//     milliseconds. Asserted coarsely, since separating microseconds from
//     milliseconds needs no precision.
//
// Both configurations are covered: `:stream-alloc-test` links the coroutine
// scheduler, `:stream-alloc-test-sim` the thread-per-task one.
//
// This target replaces global `operator new`, which is process-wide, so it is
// deliberately kept out of the catch-all `tapa-lib-test` binary.

#include <cstddef>
#include <cstdlib>

#include <atomic>
#include <chrono>
#include <iostream>
#include <new>
#include <utility>

#include <gtest/gtest.h>

#include "tapa.h"

namespace {

std::atomic<std::size_t> g_alloc_count{0};
std::atomic<bool> g_counting{false};

void CountAllocation() {
  if (g_counting.load(std::memory_order_relaxed)) {
    g_alloc_count.fetch_add(1, std::memory_order_relaxed);
  }
}

// Counts allocations made while `g_counting` is set, and returns the total.
class AllocationScope {
 public:
  AllocationScope() {
    g_alloc_count.store(0, std::memory_order_relaxed);
    g_counting.store(true, std::memory_order_relaxed);
  }
  std::size_t Stop() {
    g_counting.store(false, std::memory_order_relaxed);
    return g_alloc_count.load(std::memory_order_relaxed);
  }
};

constexpr int kElements = 20000;

void Producer(tapa::ostream<int>& out_q, int n) {
  for (int i = 0; i < n; ++i) out_q.write(i);
}

void Consumer(tapa::istream<int>& in_q, int n) {
  for (int i = 0; i < n; ++i) in_q.read();
}

// Allocations charged to moving `kElements` through a channel of `Depth`.
// A depth of 2 blocks on nearly every element; a depth past `kElements` never
// blocks at all. Everything else about the two runs is identical, so the
// difference isolates what blocking itself costs.
template <int Depth>
std::pair<std::size_t, double> Transfer() {
  tapa::stream<int, Depth> q;
  const auto start = std::chrono::steady_clock::now();
  AllocationScope scope;
  tapa::task().invoke(Producer, q, kElements).invoke(Consumer, q, kElements);
  const std::size_t allocations = scope.Stop();
  const std::chrono::duration<double, std::milli> elapsed =
      std::chrono::steady_clock::now() - start;
  return {allocations, elapsed.count()};
}

TEST(StreamAllocTest, BlockingDoesNotAllocatePerPoll) {
  const auto [blocking, blocking_ms] = Transfer<2>();
  const auto [non_blocking, non_blocking_ms] = Transfer<kElements + 1>();

  // Reported unconditionally so a regression shows the actual numbers.
  std::cerr << "elements=" << kElements << "\n"
            << "  blocking(depth=2):            " << blocking
            << " allocations, " << blocking_ms << " ms\n"
            << "  non_blocking(depth=" << (kElements + 1)
            << "): " << non_blocking << " allocations, " << non_blocking_ms
            << " ms\n";

  ASSERT_GT(blocking + non_blocking, 0u)
      << "the allocation hook never fired; the counting build is broken";

  // Scheduling differs slightly between the two shapes, so allow a fixed
  // slack -- but nothing proportional to kElements. Before the blocked-poll
  // path stopped formatting its reason eagerly, `blocking` exceeded
  // `non_blocking` by roughly two allocations per element.
  constexpr std::size_t kSlack = 2000;
  EXPECT_LE(blocking, non_blocking + kSlack)
      << "blocking cost " << blocking << " allocations vs " << non_blocking
      << " when never blocking, over " << kElements
      << " elements: the blocked-poll path is allocating per poll again";

  // Deliberately coarse. This does not measure throughput -- it separates a
  // microsecond-scale backoff from a millisecond-scale sleep, which are two
  // orders of magnitude apart, so machine speed and load cannot bridge them.
  // Sleeping a millisecond per blocked poll costs upwards of ten seconds here;
  // yielding costs tens of milliseconds.
  constexpr double kMillisecondSleepWouldExceedMs = 2000.0;
  EXPECT_LT(blocking_ms, kMillisecondSleepWouldExceedMs)
      << "moving " << kElements << " elements through a depth-2 channel took "
      << blocking_ms
      << " ms: a blocked poll is sleeping again instead of backing off";
}

}  // namespace

// Replacing the global allocation functions has to be done as a consistent
// set, or memory taken from one allocator is returned to another. The aligned
// (`align_val_t`) forms are deliberately left to the default implementations:
// they pair only with each other, so not replacing them keeps them consistent.
void* operator new(std::size_t size) {
  CountAllocation();
  void* p = std::malloc(size == 0 ? 1 : size);
  if (p == nullptr) throw std::bad_alloc();
  return p;
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
  CountAllocation();
  return std::malloc(size == 0 ? 1 : size);
}
void* operator new[](std::size_t size, const std::nothrow_t& tag) noexcept {
  return ::operator new(size, tag);
}
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t) noexcept { std::free(p); }
void operator delete(void* p, const std::nothrow_t&) noexcept { std::free(p); }
void operator delete[](void* p, const std::nothrow_t&) noexcept {
  std::free(p);
}
