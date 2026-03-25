// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/tapa.h"

#include <chrono>
#include <csignal>

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

#include <frt.h>

namespace tapa {

namespace {

void reschedule_this_thread() {
  std::this_thread::sleep_for(std::chrono::milliseconds(1));
}

}  // namespace

}  // namespace tapa

#if TAPA_ENABLE_COROUTINE

#include <boost/coroutine2/coroutine.hpp>
#include <boost/coroutine2/segmented_stack.hpp>
#include <boost/thread/condition_variable.hpp>

#if TAPA_ENABLE_STACKTRACE
#include <boost/algorithm/string/predicate.hpp>
#include <boost/stacktrace.hpp>
#endif  // TAPA_ENABLE_STACKTRACE

using std::function;
using std::string;
using std::unordered_map;

using boost::condition_variable;
using boost::mutex;
using boost::coroutines2::segmented_stack;

using pull_type = boost::coroutines2::coroutine<void>::pull_type;
using push_type = boost::coroutines2::coroutine<void>::push_type;
using unique_lock = boost::unique_lock<mutex>;

namespace tapa {

namespace internal {

// Killed via SIGINT when tapa::invoke synchronous kernel is running.
fpga::Instance* frt_sync_kernel_instance = nullptr;
extern "C" void kill_frt_sync_kernel(int) {
  if (frt_sync_kernel_instance) {
    frt_sync_kernel_instance->Kill();
    frt_sync_kernel_instance = nullptr;
  }
  exit(EXIT_FAILURE);
}

namespace {

thread_local pull_type* current_handle = nullptr;
thread_local bool debug = false;
mutex debug_mtx;  // Print stacktrace one-by-one.

}  // namespace

void yield(const string& msg) {
  if (debug) {
    unique_lock l(debug_mtx);
    LOG(INFO) << msg;
#if TAPA_ENABLE_STACKTRACE
    using boost::algorithm::ends_with;
    using boost::algorithm::starts_with;
    for (auto& frame : boost::stacktrace::stacktrace()) {
      const auto line = frame.source_line();
      const auto file = frame.source_file();
      auto name = frame.name();
      if (line == 0 || file == __FILE__ ||
          // Ignore STL functions.
          starts_with(name, "void std::") || starts_with(name, "std::") ||
          // Ignore TAPA channel functions.
          ends_with(file, "/tapa/mmap.h") ||
          ends_with(file, "/tapa/stream.h")) {
        continue;
      }
      name = name.substr(0, name.find('('));
      const auto space_pos = name.find(' ');
      if (space_pos != string::npos) name = name.substr(space_pos + 1);
      LOG(INFO) << "  in " << name << "(...) from " << file << ":" << line;
    }
#endif  // TAPA_ENABLE_STACKTRACE
  }
  if (current_handle == nullptr) {
    reschedule_this_thread();
  } else {
    (*current_handle)();
  }
}

namespace {

uint64_t get_time_ns() {
  timespec tp;
  clock_gettime(CLOCK_MONOTONIC, &tp);
  return static_cast<uint64_t>(tp.tv_sec) * 1000000000 + tp.tv_nsec;
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

void yield(const std::string& msg) { reschedule_this_thread(); }

namespace {

std::deque<std::thread>* threads = nullptr;
const task* top_task = nullptr;
int active_task_count = 0;
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

fpga::Instance* frt_sync_kernel_instance = nullptr;
extern "C" void kill_frt_sync_kernel(int) {
  if (frt_sync_kernel_instance) {
    frt_sync_kernel_instance->Kill();
    frt_sync_kernel_instance = nullptr;
  }
  exit(EXIT_FAILURE);
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

task& task::invoke_frt(std::shared_ptr<fpga::Instance> instance) {
  instance->WriteToDevice();
  instance->Exec();
  instance->ReadFromDevice();
  internal::schedule_cleanup([instance]() { instance->Kill(); });
  internal::schedule(
      /*detach=*/false, [instance]() {
        while (!instance->IsFinished()) {
          reschedule_this_thread();
          internal::yield("fpga::Instance() is not finished");
        }
        instance->Finish();
      });
  return *this;
}

}  // namespace tapa
