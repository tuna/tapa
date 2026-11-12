// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "tapa/host/tapa.h"

#include <chrono>
#include <csignal>
#include <cstdlib>

#include <algorithm>
#include <atomic>
#include <deque>
#include <exception>
#include <functional>
#include <list>
#include <map>
#include <memory>
#include <mutex>
#include <new>
#include <queue>
#include <set>
#include <string>
#include <thread>
#include <unordered_map>

#include <sys/mman.h>
#include <time.h>

#include "tapa/host/frt/instance.h"
#include "tapa/host/private_util.h"

namespace tapa {

namespace internal {

thread_local int blocked_poll_streak = 0;
thread_local uint64_t blocked_poll_count = 0;

namespace {

// Whether the running task descends from a detached invoke. Hardware waits
// on no instance under a detached one, joined or not: the tasks a detached
// upper task invokes are detached with it.
thread_local bool in_detached_scope = false;

// `f` as `schedule` runs it: it and every task it invokes are detached if
// `detach`.
std::function<void()> scoped(bool detach, const std::function<void()>& f) {
  return [detach, f] {
    in_detached_scope = detach;
    f();
  };
}

}  // namespace

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

int frt_instances_scheduled() {
  // Scheduling adds one in flight; finishing moves one across. The sum is
  // therefore the running total and never decreases.
  return frt_instances_finished.load(std::memory_order_relaxed) +
         frt_instances_in_flight.load(std::memory_order_relaxed);
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

namespace {

hybrid_recorder* current_recorder = nullptr;

// A stream name as a kernel argument spells it: a channel `a[2]` of a
// stream array is the argument `a_2`.
std::string KernelArgName(const std::string& name) {
  std::string out;
  for (char c : name) {
    if (c == '[') {
      out += '_';
    } else if (c != ']') {
      out += c;
    }
  }
  return out;
}

// The invoke that binds the kernel stream `name`, among those `in` marks,
// and its use of that stream.
std::pair<size_t, const stream_use*> find_binding(
    const std::vector<recorded_invoke>& invokes, const std::vector<bool>& in,
    const std::string& name) {
  for (size_t i = 0; i < invokes.size(); ++i) {
    if (!in[i]) continue;
    for (const stream_use& use : invokes[i].uses) {
      if (KernelArgName(use.name) == name) return {i, &use};
    }
  }
  // A stream inside an inlined instance is named after the instance.
  const size_t scope = name.rfind("__");
  const std::string variable =
      scope == std::string::npos ? name : name.substr(scope + 2);
  LOG(FATAL) << "kernel stream '" << name << "' connects to no task; the "
             << "host binds each stream by its name, so name it after its "
             << "variable, e.g. tapa::stream<T> " << variable << "(\""
             << variable << "\")";
  return {0, nullptr};
}

// The invokes of an upper task's body, flattened as the kernel was built:
// the invoke of an instance the kernel inlined is replaced by the invokes
// of that instance's body, and a stream declared in that body is named
// after the instance, e.g. `Stage_0__mid`. Each inlined instance is found
// by its position, as the host makes a body's invokes in the order tapacc
// records. `scope` names the body: empty for the top, else an inlined
// instance.
std::vector<recorded_invoke> flatten(
    std::vector<recorded_invoke> invokes, const std::string& scope,
    const std::vector<frt::InlinedTask>& inlined) {
  std::vector<recorded_invoke> flat;
  for (size_t i = 0; i < invokes.size(); ++i) {
    const auto task = std::find_if(
        inlined.begin(), inlined.end(), [&](const frt::InlinedTask& task) {
          return task.parent == scope && task.position == static_cast<int>(i);
        });
    if (task == inlined.end()) {
      invokes[i].scope = scope;
      invokes[i].position = static_cast<int>(i);
      flat.push_back(std::move(invokes[i]));
      continue;
    }
    std::unordered_map<const void*, std::string> outer;
    for (const stream_use& use : invokes[i].uses) {
      outer.emplace(use.queue, use.name);
    }
    hybrid_recorder body;
    std::vector<recorded_invoke> children = body.run(invokes[i].functor);
    for (recorded_invoke& child : children) {
      for (stream_use& use : child.uses) {
        const auto known = outer.find(use.queue);
        use.name =
            known != outer.end() ? known->second : task->name + "__" + use.name;
      }
    }
    for (recorded_invoke& child :
         flatten(std::move(children), task->name, inlined)) {
      flat.push_back(std::move(child));
    }
  }
  for (const frt::InlinedTask& task : inlined) {
    LOG_IF(FATAL, task.parent == scope &&
                      task.position >= static_cast<int>(invokes.size()))
        << "the kernel inlines '" << task.name << "', invoke " << task.position
        << " of " << (scope.empty() ? "the top" : scope)
        << ", which makes only " << invokes.size() << " invokes; rebuild "
        << "the kernel from this source";
  }
  return flat;
}

// Adds the software tasks of every AI Engine async_mmap port in `crossings`
// to `invokes`, `side` of each; the invoke a port belongs to tells them when
// it returns. Returns which of the original invokes have such a port; their
// async_mmap is no longer served from host memory (see `serve_memories`).
std::vector<bool> cross_memories(
    std::vector<recorded_invoke>& invokes,
    const std::vector<frt::MemoryCrossing>& crossings, memory_side side) {
  const size_t original = invokes.size();
  std::vector<bool> crossing(original, false);
  std::vector<std::vector<std::function<void()>>> returned(original);
  std::vector<recorded_invoke> added;
  for (const frt::MemoryCrossing& memory : crossings) {
    const std::string invoke =
        "invoke " + std::to_string(memory.position) + " of " +
        (memory.parent.empty() ? "the top" : memory.parent);
    size_t i = 0;
    while (i < original && (invokes[i].scope != memory.parent ||
                            invokes[i].position != memory.position)) {
      ++i;
    }
    LOG_IF(FATAL, i == original)
        << "the kernel serves the memory of " << invoke << ", which this "
        << "program does not make; rebuild the kernel from this source";
    auto use =
        std::find_if(invokes[i].memories.begin(), invokes[i].memories.end(),
                     [&](const memory_use& use) {
                       return use.param == static_cast<size_t>(memory.param);
                     });
    LOG_IF(FATAL, use == invokes[i].memories.end() || !use->cross)
        << "the kernel serves parameter " << memory.param << " of " << invoke
        << " as an AI Engine async_mmap, which it is not here; rebuild the "
        << "kernel from this source";
    returned[i].push_back(use->cross(memory, side, added));
    use->serve = nullptr;  // its channels, not host memory, serve it
    crossing[i] = true;
  }
  for (size_t i = 0; i < original; ++i) {
    if (returned[i].empty()) continue;
    invokes[i].functor = [functor = std::move(invokes[i].functor),
                          returned = std::move(returned[i])] {
      functor();
      for (const std::function<void()>& done : returned) done();
    };
  }
  for (recorded_invoke& invoke : added) invokes.push_back(std::move(invoke));
  return crossing;
}

// Serves from host memory every async_mmap of `invoke` that no AI Engine
// port takes over (see `cross_memories`).
void serve_memories(const recorded_invoke& invoke) {
  for (const memory_use& use : invoke.memories) {
    if (use.serve) use.serve();
  }
}

// An AI Engine simulator that replays: every invoke runs in software, each
// kernel stream gets a copy of what is pushed on it, and the copies close
// once every joined invoke has returned. The instance takes the copies
// from the moment it runs, so it runs before any task.
void replay(frt::Instance& instance,
            const std::unordered_map<std::string, frt::HostStream>& ports,
            const std::vector<frt::HostStream>& top_ports,
            const std::map<int, stream_use>& top_streams,
            const std::vector<recorded_invoke>& invokes) {
  const std::vector<bool> every(invokes.size(), true);
  auto closes = std::make_shared<std::vector<std::function<void()>>>();
  for (const auto& [name, port] : ports) {
    auto [path, close] =
        find_binding(invokes, every, name).second->copy(/*reads=*/false);
    instance.SetStreamArg(port.index, path);
    closes->push_back(std::move(close));
  }
  // The graph's input is what software simulation takes from the host's
  // stream, which may hold more; its output, what software pushes there.
  for (const frt::HostStream& port : top_ports) {
    const auto stream = top_streams.find(port.index);
    LOG_IF(FATAL, stream == top_streams.end())
        << "the kernel's argument " << port.index << " ('" << port.name
        << "') is a stream, but the host binds no stream there";
    auto [path, close] = stream->second.copy(/*reads=*/port.kernel_reads);
    instance.SetStreamArg(port.index, path);
    closes->push_back(std::move(close));
  }
  const auto close_all = [closes] {
    for (const std::function<void()>& close : *closes) close();
  };
  auto joined = std::make_shared<std::atomic<size_t>>(0);
  for (const recorded_invoke& invoke : invokes) {
    if (!invoke.detach) joined->fetch_add(1);
  }

  LOG(INFO) << "AI Engine simulation: the design runs in software, then the "
            << "simulator replays its " << ports.size() + top_ports.size()
            << " stream(s)";
  instance.WriteToDevice();
  instance.Exec();
  if (joined->load() == 0) close_all();
  task scope;
  for (const recorded_invoke& invoke : invokes) {
    if (invoke.detach) {
      schedule(/*detach=*/true, invoke.functor);
      continue;
    }
    schedule(/*detach=*/false, [functor = invoke.functor, joined, close_all] {
      functor();
      if (joined->fetch_sub(1) == 1) close_all();
    });
  }
  schedule(/*detach=*/false, [&instance] {
    while (!instance.IsFinished()) yield("the AI Engine simulator is running");
    instance.ReadFromDevice();
    instance.Finish();
  });
}

}  // namespace

hybrid_recorder* hybrid_recorder::current() { return current_recorder; }

bool recording_invokes() { return current_recorder != nullptr; }

std::vector<recorded_invoke> hybrid_recorder::run(
    const std::function<void()>& body) {
  hybrid_recorder* const outer = current_recorder;
  current_recorder = this;
  body();
  current_recorder = outer;
  return std::move(invokes_);
}

bool hybrid_run(frt::Instance& instance, int bound_args,
                const std::map<int, stream_use>& top_streams,
                const std::function<void()>& top_body) {
  std::unordered_map<std::string, frt::HostStream> ports;
  std::vector<frt::HostStream> top_ports;
  for (const frt::HostStream& stream : instance.HostStreams()) {
    if (stream.index >= bound_args) {
      ports.emplace(stream.name, stream);
    } else {
      top_ports.push_back(stream);
    }
  }
  const frt::AieSimulation aie = instance.SimulatesAie();
  const bool replaying = aie == frt::AieSimulation::kReplay;
  if (ports.empty() && (top_ports.empty() || !replaying)) return false;

  hybrid_recorder recorder;
  std::vector<recorded_invoke> invokes =
      flatten(recorder.run(top_body), "", instance.InlinedTasks());
  const std::vector<frt::MemoryCrossing> memories = instance.MemoryCrossings();

  // An AI Engine task's async_mmap meets its channels through software
  // tasks: replaying, both ends of each run, joined by the streams the
  // simulator replays; live, the movers serve the simulator from host
  // memory; in hybrid cosimulation, a bridge carries the software task's
  // requests to the kernel's movers.
  if (replaying) {
    cross_memories(invokes, memories, memory_side::kBoth);
    for (const recorded_invoke& invoke : invokes) serve_memories(invoke);
    replay(instance, ports, top_ports, top_streams, invokes);
    return true;
  }

  // Each side of the kernel streams: an invoke binding one from the far
  // side, or from the kernel's own (near) side, and everything it reaches
  // through streams the kernel does not own. A PL kernel runs every invoke
  // not on the far side, so only the far side runs in software; a live AI
  // Engine simulator runs only its near side, so everything else does.
  const bool aie_device = aie == frt::AieSimulation::kLive;
  const std::vector<bool> crossing =
      cross_memories(invokes, memories,
                     aie_device ? memory_side::kMemory : memory_side::kTask);
  const auto side = [&](bool far) {
    std::vector<bool> marked(invokes.size(), false);
    for (size_t i = 0; i < invokes.size(); ++i) {
      for (const stream_use& use : invokes[i].uses) {
        const auto port = ports.find(KernelArgName(use.name));
        if (port != ports.end() &&
            (use.task_reads != port->second.kernel_reads) == far) {
          marked[i] = true;
        }
      }
    }
    for (bool changed = true; changed;) {
      changed = false;
      std::set<const void*> reached;
      for (size_t i = 0; i < invokes.size(); ++i) {
        if (!marked[i]) continue;
        for (const stream_use& use : invokes[i].uses) {
          if (ports.count(KernelArgName(use.name)) == 0)
            reached.insert(use.queue);
        }
      }
      for (size_t i = 0; i < invokes.size(); ++i) {
        if (marked[i]) continue;
        for (const stream_use& use : invokes[i].uses) {
          if (reached.count(use.queue) != 0) {
            marked[i] = true;
            changed = true;
          }
        }
      }
    }
    return marked;
  };
  std::vector<bool> software = side(/*far=*/true);
  // A task whose async_mmap the kernel serves is an AI Engine task, though
  // it may bind no stream of the kernel's.
  for (size_t i = 0; i < crossing.size(); ++i) {
    if (crossing[i] && !aie_device) software[i] = true;
  }
  if (aie_device) {
    std::vector<bool> near = side(/*far=*/false);
    for (size_t i = 0; i < crossing.size(); ++i) {
      if (crossing[i]) near[i] = true;
    }
    for (size_t i = 0; i < invokes.size(); ++i) {
      LOG_IF(FATAL, near[i] && software[i])
          << "a task binds AI Engine streams from both sides, which only an "
          << "upper task the kernel was built with inlined can do; rebuild "
          << "the kernel from this source with this version of TAPA";
      software[i] = !near[i];
    }
  }
  // An AI Engine simulator learns that an input is complete when its
  // software producer returns and the host closes it.
  std::vector<std::vector<std::function<void()>>> closes(invokes.size());

  auto kernel_done = std::make_shared<std::atomic<bool>>(false);
  std::vector<std::function<void()>> forwards;
  for (const auto& [name, port] : ports) {
    // A ring's frame count goes with its frames, below.
    if (port.ring_count) continue;
    const auto [producer, bound] = find_binding(invokes, software, name);
    if (port.kernel_reads && port.ring >= 0) {
      // The frames software releases on an AI Engine top's output ring,
      // counted, then their count once the writer returns, which the AI
      // Engine kernel sends on its own (see `HostStream::ring`).
      const int ring = port.ring;
      const auto count =
          std::find_if(ports.begin(), ports.end(), [ring](const auto& other) {
            return other.second.ring_count && other.second.ring == ring;
          });
      LOG_IF(FATAL, count == ports.end())
          << "the ring of the kernel's argument " << ring
          << " has no frame-count stream";
      auto counts = std::make_shared<frt_queue<elem_t<uint32_t>>>(
          2, count->first + ".count");
      instance.SetStreamArg(count->second.index,
                            counts->get_frt_stream().path());
      auto written = std::make_shared<std::atomic<bool>>(false);
      closes[producer].push_back(
          [written] { written->store(true, std::memory_order_release); });
      forwards.emplace_back();
      instance.SetStreamArg(
          port.index,
          bound->frt_path_counting(
              forwards.back(),
              [written] { return written->load(std::memory_order_acquire); },
              [counts, frame = port.frame](uint64_t moved) {
                counts->push({static_cast<uint32_t>(moved / frame), false});
              }));
      continue;
    }
    if (port.kernel_reads) {
      instance.SetStreamArg(port.index, bound->frt_path());
      if (aie_device) closes[producer].push_back(bound->close);
    } else {
      forwards.emplace_back();
      instance.SetStreamArg(
          port.index,
          bound->frt_path_dropping_eot(forwards.back(), [kernel_done] {
            return kernel_done->load(std::memory_order_acquire);
          }));
    }
  }

  size_t count = 0;
  for (bool in_software : software) count += in_software ? 1 : 0;
  if (aie_device) {
    LOG(INFO) << "AI Engine simulation: " << count << " PL task(s) run in "
              << "software against the simulator's " << ports.size()
              << " PLIO stream(s)";
  } else {
    LOG(INFO) << "hybrid cosimulation: " << count << " AI Engine task(s) run "
              << "in software against the kernel's " << ports.size()
              << " AI Engine stream(s)";
  }
  task scope;
  for (const std::function<void()>& forward : forwards) {
    schedule(/*detach=*/false, forward);
  }
  for (size_t i = 0; i < invokes.size(); ++i) {
    if (!software[i]) continue;
    serve_memories(invokes[i]);
    if (closes[i].empty()) {
      schedule(invokes[i].detach, invokes[i].functor);
      continue;
    }
    schedule(invokes[i].detach,
             [functor = invokes[i].functor, closes = std::move(closes[i])] {
               functor();
               for (const std::function<void()>& close : closes) close();
             });
  }
  schedule_frt_instance(&instance, [kernel_done] {
    kernel_done->store(true, std::memory_order_release);
  });
  return true;
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
#ifdef BOOST_USE_SEGMENTED_STACKS
#include <boost/coroutine2/segmented_stack.hpp>
using coroutine_stack = boost::coroutines2::segmented_stack;
#else
#include <boost/context/fixedsize_stack.hpp>
using coroutine_stack = boost::context::fixedsize_stack;
#endif
#include <boost/thread/condition_variable.hpp>
using condition_variable = boost::condition_variable;
using mutex = boost::mutex;

using std::function;
using std::string;
using std::unordered_map;

#ifdef BOOST_USE_SEGMENTED_STACKS
// libgcc's split-stack runtime keeps the page size in a static that only
// __morestack_load_mmap() fills in. It normally runs from an .init_array
// entry that rides in with libgcc's morestack.o, but nothing guarantees the
// linker pulls that member when no translation unit is compiled
// -fsplit-stack (Clang cannot). Left at zero, the page-size rounding in
// allocate_segment turns every request into mmap(length = 0), and each
// coroutine dies with "unable to allocate additional stack space: errno 22".
// Declared weak so a libgcc without the symbol still links.
extern "C" void __morestack_load_mmap(void) __attribute__((weak));
#endif

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
    // The coroutines this thread runs meanwhile set their own scope.
    const bool detached = in_detached_scope;
    (*current_handle)();
    in_detached_scope = detached;
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
#ifdef BOOST_USE_SEGMENTED_STACKS
  if (__morestack_load_mmap != nullptr) __morestack_load_mmap();
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
  detach = detach || in_detached_scope;
  pool->add_task(detach, scoped(detach, f));
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
    internal::pool->stop();
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

namespace {

std::deque<std::thread>* threads = nullptr;
const task* top_task = nullptr;
std::atomic<int> active_task_count{0};
std::mutex mtx;

// The invokes whose top task was constructed (under `mtx`), and those whose
// top task has returned. A task's thread serves the invoke it was scheduled
// under; once that invoke has ended, which only its detached tasks outlive,
// the thread ends at its next yield, unwinding as a coroutine does when the
// coroutine build deletes its pool.
uint64_t invokes_begun = 0;
std::atomic<uint64_t> invokes_ended{0};
thread_local uint64_t thread_invoke = 0;  // 0: not a task's thread

struct invoke_ended {};

void end_if_invoke_ended() {
  if (thread_invoke != 0 &&
      thread_invoke <= invokes_ended.load(std::memory_order_acquire) &&
      std::uncaught_exceptions() == 0) {
    throw invoke_ended{};
  }
}

}  // namespace

void yield(const char*) {
  end_if_invoke_ended();
  reschedule_this_thread();
}

void yield(const std::string& channel_name, const char* state) {
  end_if_invoke_ended();
  note_blocked_poll(channel_name, state);
  reschedule_this_thread();
}

void schedule(bool detach, const std::function<void()>& f) {
  detach = detach || in_detached_scope;
  std::unique_lock<std::mutex> lock(internal::mtx);
  const uint64_t invoke = thread_invoke != 0 ? thread_invoke : invokes_begun;
  auto run = [invoke, f = scoped(detach, f)] {
    thread_invoke = invoke;
    try {
      f();
    } catch (const invoke_ended&) {
    }
  };
  if (detach) {
    std::thread(run).detach();
  } else {
    threads->emplace_back(run);
  }
}

// Only the coroutine scheduler has a SIGINT cleanup flow.
void schedule_cleanup(const std::function<void()>&) {}

}  // namespace internal

task::task() {
  std::unique_lock<std::mutex> lock(internal::mtx);
  ++internal::active_task_count;
  // Only the invoke's own thread starts one: a detached task outliving an
  // invoke would otherwise become the next invoke's top, which then waits
  // on nothing.
  if (internal::top_task == nullptr && internal::thread_invoke == 0) {
    internal::top_task = this;
    ++internal::invokes_begun;
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
        // Joining is progress: back off only while no task can be joined.
        t.join();
        internal::blocked_poll_streak = 0;
      } else {
        reschedule_this_thread();
      }
    }
    std::unique_lock<std::mutex> lock(internal::mtx);
    internal::invokes_ended.store(internal::invokes_begun,
                                  std::memory_order_release);
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
