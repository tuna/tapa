// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/tapa.h"

#include <chrono>
#include <csignal>
#include <cstdlib>

#include <atomic>
#include <deque>
#include <fstream>
#include <functional>
#include <list>
#include <memory>
#include <mutex>
#include <new>
#include <queue>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <unordered_map>

#include <sys/mman.h>
#include <time.h>

#ifdef __APPLE__
#include <sys/sysctl.h>
#endif

#include "tapa/host/frt/instance.h"

namespace tapa {

namespace internal {

thread_local int blocked_poll_streak = 0;
thread_local uint64_t blocked_poll_count = 0;

}  // namespace internal

namespace {

// Back off a thread that cannot proceed yet.
//
// A blocked channel usually clears within microseconds, so sleeping a flat
// millisecond put a 1 kHz ceiling on every blocked transfer — without
// coroutines (the macOS default) that ceiling *is* the simulation's runtime.
// Hand the CPU over first, and only start sleeping once this thread has
// failed often enough to look genuinely stalled, so an idle worker or a wait
// on hardware still parks rather than spinning. `note_poll_progress` resets
// the streak, so an alternating producer/consumer stays on the cheap path.
void reschedule_this_thread() {
  constexpr int kYieldRounds = 64;
  constexpr int kShortSleepRounds = 256;

  // Capped at the slowest rung: every streak past it already sleeps 1 ms,
  // and an unbounded increment is signed-overflow UB after ~25 days.
  const int streak = internal::blocked_poll_streak;
  if (streak < kYieldRounds + kShortSleepRounds) {
    ++internal::blocked_poll_streak;
  }
  if (streak < kYieldRounds) {
    std::this_thread::yield();
  } else if (streak < kYieldRounds + kShortSleepRounds) {
    std::this_thread::sleep_for(std::chrono::microseconds(20));
  } else {
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
}

}  // namespace

namespace internal {

namespace {

// Instances handed to `schedule_frt_instance` that have not finished yet,
// and how many have finished. Both are needed to tell "no instance has
// started" (nothing to wait for yet) from "every instance is done" (a
// blocked FRT-bound stream will never be served).
std::atomic<int> frt_instances_in_flight{0};
std::atomic<int> frt_instances_finished{0};

}  // namespace

void note_frt_instance_scheduled() {
  frt_instances_in_flight.fetch_add(1, std::memory_order_relaxed);
}

void note_frt_instance_finished() {
  frt_instances_finished.fetch_add(1, std::memory_order_relaxed);
  frt_instances_in_flight.fetch_sub(1, std::memory_order_relaxed);
}

bool every_frt_instance_finished() {
  return frt_instances_finished.load(std::memory_order_relaxed) > 0 &&
         frt_instances_in_flight.load(std::memory_order_relaxed) == 0;
}

uint64_t parse_stall_warn_seconds(const char* text) {
  // Ten seconds sits far below the multi-hour hangs this exists to catch and
  // far above any legitimate gap between two stream operations in software
  // simulation, where a blocked channel normally clears in microseconds.
  constexpr double kDefaultSeconds = 10.;

  double seconds = kDefaultSeconds;
  if (text != nullptr && *text != '\0') {
    char* end = nullptr;
    const double parsed = std::strtod(text, &end);
    // UINT64_MAX nanoseconds; also rejects inf and NaN, whose conversion to
    // uint64_t is undefined behavior.
    constexpr double kMaxSeconds = static_cast<double>(UINT64_MAX) / 1e9;
    if (end != text && *end == '\0' && parsed >= 0. && parsed <= kMaxSeconds) {
      seconds = parsed;
    } else {
      LOG(WARNING) << "ignoring TAPA_STALL_WARN_SECONDS='" << text
                   << "'; expected a nonnegative number of seconds"
                   << " (at most " << kMaxSeconds << ")";
    }
  }
  return static_cast<uint64_t>(seconds * 1e9);
}

namespace {

// Blocked polls between clock reads. A deadlocked task polls in a tight
// scheduler loop, so this is reached in well under a second while keeping
// `clock_gettime` off all but a sliver of the blocked path.
constexpr uint64_t kStallSampleInterval = 512;

// A deadlock blocks every task at once, and one line per stuck channel is the
// useful part; past that it is repetition.
constexpr int kMaxStallWarnings = 8;

std::atomic<int> stall_warnings_emitted{0};

uint64_t stall_warn_threshold_ns() {
  static const uint64_t threshold =
      parse_stall_warn_seconds(std::getenv("TAPA_STALL_WARN_SECONDS"));
  return threshold;
}

uint64_t monotonic_ns() {
  return static_cast<uint64_t>(
      std::chrono::duration_cast<std::chrono::nanoseconds>(
          std::chrono::steady_clock::now().time_since_epoch())
          .count());
}

}  // namespace

void note_blocked_poll(const std::string& channel_name, const char* state) {
  const uint64_t threshold_ns = stall_warn_threshold_ns();
  if (threshold_ns == 0) return;

  // Zeroed by `note_poll_progress`, so this only climbs while nothing this
  // thread polls is clearing.
  const uint64_t polls = ++blocked_poll_count;
  if (polls % kStallSampleInterval != 0) return;

  thread_local uint64_t stall_since_ns = 0;
  thread_local bool warned = false;
  if (polls == kStallSampleInterval) {
    // First sample of this stall: start the clock rather than measure from a
    // previous one. Also re-arms the warning after progress resumed.
    stall_since_ns = monotonic_ns();
    warned = false;
    return;
  }
  if (warned) return;

  // A kernel instance may legitimately run for hours, and a host stream bound
  // to one is waiting on hardware or RTL rather than on a deadlocked peer.
  if (frt_instances_in_flight.load(std::memory_order_relaxed) > 0) return;

  const uint64_t elapsed_ns = monotonic_ns() - stall_since_ns;
  if (elapsed_ns < threshold_ns) return;
  warned = true;

  const int emitted =
      stall_warnings_emitted.fetch_add(1, std::memory_order_relaxed);
  if (emitted >= kMaxStallWarnings) return;
  LOG(WARNING) << "no stream progress for " << elapsed_ns / 1000000000
               << "s; blocked on channel '" << channel_name << "' (" << state
               << "). A consumer stuck on an empty channel or a producer stuck "
                  "on a full one usually means the two disagree on how many "
                  "elements the transaction carries. Set "
                  "TAPA_STALL_WARN_SECONDS to change the threshold, or 0 to "
                  "silence this.";
  if (emitted + 1 == kMaxStallWarnings) {
    LOG(WARNING) << "further channel stall warnings suppressed";
  }
}

// Killed via SIGINT when tapa::invoke synchronous kernel is running.
frt::Instance* frt_sync_kernel_instance = nullptr;
extern "C" void kill_frt_sync_kernel(int) {
  if (frt_sync_kernel_instance) {
    frt_sync_kernel_instance->Kill();
    frt_sync_kernel_instance = nullptr;
  }
  exit(EXIT_FAILURE);
}

}  // namespace internal

}  // namespace tapa

#if TAPA_ENABLE_COROUTINE

#include <boost/coroutine2/coroutine.hpp>
#include <boost/coroutine2/segmented_stack.hpp>
#include <boost/thread/condition_variable.hpp>

using std::function;
using std::string;
using std::unordered_map;

using boost::condition_variable;
using boost::mutex;
using boost::coroutines2::segmented_stack;

// libgcc's split-stack runtime keeps the page size in a static that only
// __morestack_load_mmap() fills in. It normally runs from an .init_array
// entry that rides in with libgcc's morestack.o, but nothing guarantees the
// linker pulls that member when no translation unit is compiled
// -fsplit-stack (Clang cannot). Left at zero, the page-size rounding in
// allocate_segment turns every request into mmap(length = 0), and each
// coroutine dies with "unable to allocate additional stack space: errno 22".
// Declared weak so a libgcc without the symbol still links.
extern "C" void __morestack_load_mmap(void) __attribute__((weak));

using pull_type = boost::coroutines2::coroutine<void>::pull_type;
using push_type = boost::coroutines2::coroutine<void>::push_type;
using unique_lock = boost::unique_lock<mutex>;

namespace tapa {

namespace internal {

namespace {

thread_local pull_type* current_handle = nullptr;
thread_local bool debug = false;
mutex debug_mtx;  // Serialize debug logging across threads.

}  // namespace

namespace {

void yield_to_scheduler() {
  if (current_handle == nullptr) {
    reschedule_this_thread();
  } else {
    (*current_handle)();
  }
}

}  // namespace

void yield(const char* reason) {
  if (debug) {
    unique_lock l(debug_mtx);
    LOG(INFO) << reason;
  }
  yield_to_scheduler();
}

void yield(const string& channel_name, const char* state) {
  if (debug) {
    unique_lock l(debug_mtx);
    LOG(INFO) << "channel '" << channel_name << "' is " << state;
  }
  note_blocked_poll(channel_name, state);
  yield_to_scheduler();
}

namespace {

uint64_t get_time_ns() {
  timespec tp;
  clock_gettime(CLOCK_MONOTONIC, &tp);
  return static_cast<uint64_t>(tp.tv_sec) * 1000000000 + tp.tv_nsec;
}

// Idempotent: safe whether or not the .init_array entry already ran.
void ensure_split_stack_runtime_ready() {
  if (__morestack_load_mmap != nullptr) __morestack_load_mmap();
}

int get_physical_core_count() {
#ifdef __APPLE__
  int count = 0;
  size_t size = sizeof(count);
  if (sysctlbyname("hw.physicalcpu", &count, &size, nullptr, 0) == 0 &&
      count > 0) {
    return count;
  }
  return std::thread::hardware_concurrency();
#else
  auto trim = [](std::string s) {
    auto b = s.find_first_not_of(" \t");
    auto e = s.find_last_not_of(" \t");
    return (b == std::string::npos) ? "" : s.substr(b, e - b + 1);
  };
  std::ifstream cpuinfo("/proc/cpuinfo");
  std::string line;
  std::set<int> cores;
  while (std::getline(cpuinfo, line)) {
    std::istringstream iss(line);
    std::string key, val;
    if (std::getline(iss, key, ':') && std::getline(iss, val)) {
      if (trim(key) == "core id") cores.insert(std::stoi(trim(val)));
    }
  }
  return cores.size();
#endif
}

#include "tapa/host/private_scheduler.h"

thread_pool* pool = nullptr;
const task* top_task = nullptr;
mutex mtx;

// SIGINT flow: main thread receives -> each worker sets signal ->
// next coroutine iteration prints debug info -> each worker clears signal.
constexpr int64_t kSignalThreshold = 500 * 1000 * 1000;  // 500 ms
int64_t last_signal_timestamp = 0;
void signal_handler(int signal) {
  const int64_t signal_timestamp = get_time_ns();
  if (last_signal_timestamp != 0 &&
      signal_timestamp - last_signal_timestamp < kSignalThreshold) {
    LOG(INFO) << "caught SIGINT twice in " << kSignalThreshold / 1000000
              << " ms; exit";
    pool->run_cleanup_tasks();
    exit(EXIT_FAILURE);
  }
  LOG(INFO) << "caught SIGINT";
  last_signal_timestamp = signal_timestamp;
  pool->send(signal);
}

}  // namespace

void schedule(bool detach, const function<void()>& f) {
  pool->add_task(detach, f);
}

void schedule_cleanup(const function<void()>& f) { pool->add_cleanup_task(f); }

}  // namespace internal

task::task() {
  unique_lock lock(internal::mtx);
  if (internal::pool == nullptr) {
    internal::pool = new internal::thread_pool;
    internal::top_task = this;
  }
}

task::~task() {
  if (this == internal::top_task) {
    internal::pool->wait();
    unique_lock lock(internal::mtx);
    delete internal::pool;
    internal::pool = nullptr;
    internal::top_task = nullptr;
  }
}

}  // namespace tapa

// Weak definitions for asan compatibility with boost's ucontext.
extern "C" {
__attribute__((weak)) void __sanitizer_start_switch_fiber(void**, const void*,
                                                          size_t) {}
__attribute__((weak)) void __sanitizer_finish_switch_fiber(void*, const void**,
                                                           size_t*) {}
}

#else  // TAPA_ENABLE_COROUTINE

namespace tapa {
namespace internal {

void yield(const char*) { reschedule_this_thread(); }

void yield(const std::string& channel_name, const char* state) {
  note_blocked_poll(channel_name, state);
  reschedule_this_thread();
}

namespace {

std::deque<std::thread>* threads = nullptr;
const task* top_task = nullptr;
std::atomic<int> active_task_count{0};
std::mutex mtx;

}  // namespace

void schedule(bool detach, const std::function<void()>& f) {
  if (detach) {
    std::thread(f).detach();
  } else {
    std::unique_lock<std::mutex> lock(internal::mtx);
    threads->emplace_back(f);
  }
}

namespace {

std::list<std::function<void()> > cleanup_tasks;

}  // namespace

void schedule_cleanup(const std::function<void()>& f) {
  cleanup_tasks.push_back(f);
}

}  // namespace internal

task::task() {
  std::unique_lock<std::mutex> lock(internal::mtx);
  ++internal::active_task_count;
  if (internal::top_task == nullptr) {
    internal::top_task = this;
  }
  if (internal::threads == nullptr) {
    internal::threads = new std::deque<std::thread>;
  }
}

task::~task() {
  if (this == internal::top_task) {
    for (;;) {
      std::thread t;
      {
        std::unique_lock<std::mutex> lock(internal::mtx, std::defer_lock);
        if (internal::active_task_count == 1 && lock.try_lock()) {
          if (internal::threads->empty()) {
            break;
          }
          t = std::move(internal::threads->front());
          internal::threads->pop_front();
        }
      }
      if (t.joinable()) {
        t.join();
      }
      reschedule_this_thread();
    }
    internal::top_task = nullptr;
  }
  std::unique_lock<std::mutex> lock(internal::mtx);
  --internal::active_task_count;
}

}  // namespace tapa

#endif  // TAPA_ENABLE_COROUTINE

namespace tapa {
namespace internal {

void* allocate(size_t length) {
  void* addr = ::mmap(nullptr, length, PROT_READ | PROT_WRITE,
                      MAP_SHARED | MAP_ANONYMOUS, /*fd=*/-1, /*offset=*/0);
  if (addr == MAP_FAILED) throw std::bad_alloc();
  return addr;
}
void deallocate(void* addr, size_t length) {
  if (::munmap(addr, length) != 0) throw std::bad_alloc();
}

}  // namespace internal

task& task::invoke_frt(std::shared_ptr<internal::frt::Instance> instance) {
  internal::schedule_frt_instance(std::move(instance));
  return *this;
}

}  // namespace tapa
