// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

// Guards the simulator's blocked-poll path against per-poll heap allocation.
//
// A task that blocks on a full or empty channel yields to the scheduler, and
// the yield carries a human-readable reason that is only ever formatted when
// stream debugging is switched on. If that reason is built eagerly, every
// blocked poll pays for a `std::string` -- and a producer/consumer pair sharing
// a shallow channel blocks on nearly every element, so the cost scales with
// the data, not with the number of times anyone reads a log.
//
// Wall-clock is a poor measure here: without coroutines a yield sleeps for a
// millisecond, which swamps the allocation entirely. Counting allocations
// instead is deterministic and measures exactly the defect.
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

  // Reported unconditionally so a regression shows the actual numbers. The
  // timings are informational only: they are far too machine- and
  // load-dependent to assert on, whereas the allocation counts are exact.
  std::cerr << "elements=" << kElements << "\n"
            << "  blocking(depth=2):            " << blocking
            << " allocations, " << blocking_ms << " ms\n"
            << "  non_blocking(depth=" << (kElements + 1) << "): "
            << non_blocking << " allocations, " << non_blocking_ms << " ms\n";

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
