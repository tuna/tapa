// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include "tapa/base/task.h"

#include "tapa/host/coroutine.h"
#include "tapa/host/logging.h"

#include <chrono>
#include <csignal>
#include <functional>
#include <memory>
#include <tuple>
#include <type_traits>
#include <utility>
#include <vector>

#include "tapa/host/frt/instance.h"
#include "tapa/host/stream.h"

namespace tapa {

namespace internal {

extern frt::Instance* frt_sync_kernel_instance;
extern "C" void kill_frt_sync_kernel(int);

template <typename Param, typename Arg>
struct accessor {
  static Param access(Arg&& arg, bool) { return arg; }
  static void access(frt::Instance& instance, int& idx, Arg&& arg) {
    Param val = static_cast<Param>(arg);
    instance.SetScalarArg(idx++, &val, sizeof(val));
  }
};

template <typename T>
struct accessor<T, seq> {
  static T access(seq&& arg, bool) { return arg.pos++; }
  static void access(frt::Instance& instance, int& idx, seq&& arg) {
    T val = static_cast<T>(arg.pos++);
    instance.SetScalarArg(idx++, &val, sizeof(val));
  }
};

void* allocate(size_t length);
void deallocate(void* addr, size_t length);

// Runs the kernel instance as a task; `on_finish` runs once it finished.
template <typename InstancePtr>
void schedule_frt_instance(InstancePtr instance,
                           std::function<void()> on_finish = {}) {
  schedule_cleanup([instance]() { instance->Kill(); });
  // Counted before the task is queued so a producer that blocks first sees
  // an instance in flight rather than "everything finished".
  note_frt_instance_scheduled();
  schedule(/*detach=*/false, [instance, on_finish]() {
    instance->WriteToDevice();
    instance->Exec();
    instance->ReadFromDevice();
    while (!instance->IsFinished()) {
      yield("frt::Instance() is not finished");
    }
    instance->Finish();
    note_frt_instance_finished();
    if (on_finish) on_finish();
  });
}

// One invoke of a hybrid run's top body, recorded instead of run.
struct recorded_invoke {
  bool detach;
  std::function<void()> functor;
  std::vector<stream_use> uses;
};

// Collects the invokes of a hybrid run's top body (see `hybrid_run`).
class hybrid_recorder {
 public:
  // The recorder of the top body running now, or null.
  static hybrid_recorder* current();
  void record(recorded_invoke invoke) { invokes_.push_back(std::move(invoke)); }

 private:
  friend bool hybrid_run(frt::Instance&, int, const std::function<void()>&);
  std::vector<recorded_invoke> invokes_;
};

// Hybrid cosimulation of a kernel whose PL half leaves stream arguments to
// the host: an AI Engine design's PL kernel has one AXIS port per stream
// to or from the AI Engine. Runs `top_body` recording its invokes; an
// invoke that reads a stream the kernel writes, or writes one the kernel
// reads, is on the AI Engine side, and so is every invoke it shares a
// stream with that is not a kernel port. Those run in software, with the
// kernel's streams bound by name, alongside the kernel; the PL invokes run
// in the kernel. Returns false, doing nothing, when the kernel leaves no
// stream to the host past the `bound_args` the caller set.
bool hybrid_run(frt::Instance& instance, int bound_args,
                const std::function<void()>& top_body);

// std::bind wrapper ensuring left-to-right argument evaluation.
struct binder {
  template <typename F, typename... Args>
  binder(F&& f, Args&&... args)
      : result(std::bind(std::forward<F>(f), std::forward<Args>(args)...)) {}
  std::function<void()> result;
};

template <typename T>
struct function_traits : public function_traits<decltype(&T::operator())> {};
template <typename R, typename... Args>
struct function_traits<R (*)(Args...)> {
  using return_type = R;
  using params = std::tuple<Args...>;
};
template <typename C, typename R, typename... Args>
struct function_traits<R (C::*)(Args...) const> {
  using return_type = R;
  using params = std::tuple<Args...>;
};
template <typename R, typename... Args>
struct function_traits<R (&)(Args...)> {
  using return_type = R;
  using params = std::tuple<Args...>;
};

template <typename F, typename = void>
struct is_callable : std::is_function<F> {};
template <typename F>
struct is_callable<F, std::void_t<decltype(&F::operator())>> : std::true_type {
};
template <typename F>
inline constexpr bool is_callable_v = is_callable<F>::value;

template <typename F>
struct invoker {
  using FuncType = std::decay_t<F>;
  using Params = typename function_traits<FuncType>::params;

  static_assert(
      std::is_same_v<void, typename function_traits<FuncType>::return_type>,
      "task function must return void");

  template <typename... Args>
  static void invoke(InvokeMode mode, F&& f, Args&&... args) {
    if (hybrid_recorder* recorder = hybrid_recorder::current()) {
      recorder->record(invoker::recorded(mode, std::forward<F>(f),
                                         std::index_sequence_for<Args...>{},
                                         std::forward<Args>(args)...));
      return;
    }
    auto functor = invoker::functor_with_accessors(
        mode == InvokeMode::kSequential, std::forward<F>(f),
        std::index_sequence_for<Args...>{}, std::forward<Args>(args)...);
    if (mode == InvokeMode::kSequential) {
      std::move(functor)();
    } else {
      schedule(mode == InvokeMode::kDetach, std::move(functor));
    }
  }

  template <typename... Args>
  static int64_t invoke(F&& f, const std::string& bitstream, Args&&... args) {
    if (bitstream.empty()) {
      LOG(INFO) << "running software simulation with TAPA library";
      const auto tic = std::chrono::steady_clock::now();
      f(std::forward<Args>(args)...);
      const auto toc = std::chrono::steady_clock::now();
      return std::chrono::duration_cast<std::chrono::nanoseconds>(toc - tic)
          .count();
    } else {
      return invoke_frt(f, bitstream, std::forward<Args>(args)...);
    }
  }

  // Sets the kernel arguments; returns how many it set.
  template <size_t... Is, typename... CapturedArgs>
  static int set_fpga_args(frt::Instance& instance, std::index_sequence<Is...>,
                           CapturedArgs&&... args) {
    int idx = 0;
    (accessor<std::tuple_element_t<Is, Params>, CapturedArgs>::access(
         instance, idx, std::forward<CapturedArgs>(args)),
     ...);
    return idx;
  }

 private:
  template <typename... Args>
  static int64_t invoke_frt(F&& f, const std::string& bitstream,
                            Args&&... args) {
    auto instance = frt::Instance(bitstream);

    // Register SIGINT handler to kill the kernel.
    CHECK(frt_sync_kernel_instance == nullptr)
        << "kernel instance already exists";
    frt_sync_kernel_instance = &instance;
    signal(SIGINT, &kill_frt_sync_kernel);

    const int bound =
        set_fpga_args(instance, std::index_sequence_for<Args...>{},
                      std::forward<Args>(args)...);
    if (!hybrid_run(instance, bound, [&] { f(std::forward<Args>(args)...); })) {
      instance.WriteToDevice();
      instance.Exec();
      instance.ReadFromDevice();
      instance.Finish();
    }

    // Unregister SIGINT handler.
    signal(SIGINT, SIG_DFL);
    CHECK(frt_sync_kernel_instance == &instance) << "kernel instance mismatch";
    frt_sync_kernel_instance = nullptr;

    return instance.ComputeTimeNanoSeconds();
  }

  // The invoke as a hybrid run records it: the task applied to its
  // accessed arguments, and the streams those arguments touch.
  template <size_t... Is, typename... CapturedArgs>
  static recorded_invoke recorded(InvokeMode mode, F&& func,
                                  std::index_sequence<Is...>,
                                  CapturedArgs&&... args) {
    // Braced initialization evaluates the accessors left to right.
    std::tuple<std::decay_t<std::tuple_element_t<Is, Params>>...> values{
        accessor<std::tuple_element_t<Is, Params>, CapturedArgs>::access(
            std::forward<CapturedArgs>(args), /*sequential=*/false)...};
    recorded_invoke invoke;
    invoke.detach = mode == InvokeMode::kDetach;
    (stream_uses<std::decay_t<std::tuple_element_t<Is, Params>>>::collect(
         std::get<Is>(values), invoke.uses),
     ...);
    invoke.functor = [func = FuncType(func), values]() mutable {
      std::apply(func, values);
    };
    return invoke;
  }

  template <typename Func, size_t... Is, typename... CapturedArgs>
  static auto functor_with_accessors(bool is_sequential, Func&& func,
                                     std::index_sequence<Is...>,
                                     CapturedArgs&&... args) {
    // Aggregate initialization evaluates args left-to-right; std::bind copies.
    return binder{
        func, accessor<std::tuple_element_t<Is, Params>, CapturedArgs>::access(
                  std::forward<CapturedArgs>(args), is_sequential)...}
        .result;
  }
};

}  // namespace internal

/// Overrides the executable target for @c tapa::task::invoke.
class executable {
 public:
  explicit executable(std::string path) : path_(std::move(path)) {}

  // Not copyable or movable.
  executable(const executable& other) = delete;
  executable& operator=(const executable& other) = delete;

 private:
  friend struct task;
  const std::string path_;
};

/// Parent task that instantiates and joins/detaches child task instances.
struct task {
  explicit task();
  ~task();

  task(const task&) = delete;
  task& operator=(const task&) = delete;

  template <typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args) {
    return invoke<join>(std::forward<Func>(func), "",
                        std::forward<Args>(args)...);
  }

  template <internal::InvokeMode mode, typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args) {
    return invoke<mode>(std::forward<Func>(func), "",
                        std::forward<Args>(args)...);
  }

  template <typename Func, typename... Args, size_t name_size>
  task& invoke(Func&& func, const char (&name)[name_size], Args&&... args) {
    return invoke<join>(std::forward<Func>(func), name,
                        std::forward<Args>(args)...);
  }

  template <internal::InvokeMode mode, typename Func, typename... Args,
            size_t name_size>
  task& invoke(Func&& func, const char (&name)[name_size], Args&&... args) {
    static_assert(
        internal::is_callable_v<typename std::remove_reference_t<Func>>,
        "the first argument for tapa::task::invoke() must be callable");
    internal::invoker<Func>::template invoke<Args...>(
        mode, std::forward<Func>(func), std::forward<Args>(args)...);
    return *this;
  }

  /// Host-only invoke with @c executable to override execution target.
  /// Must be called before any direct @c tapa::stream reader/writer.
  template <typename Func, typename... Args>
  task& invoke(Func&& func, executable exe, Args&&... args) {
    if (exe.path_.empty()) {
      return invoke(std::forward<Func>(func), std::forward<Args>(args)...);
    }

    auto instance = std::make_shared<internal::frt::Instance>(exe.path_);
    internal::invoker<Func>::set_fpga_args(*instance,
                                           std::index_sequence_for<Args...>{},
                                           std::forward<Args>(args)...);
    return invoke_frt(std::move(instance));
  }

  template <internal::InvokeMode mode, int n, typename Func, typename... Args>
  task& invoke(Func&& func, Args&&... args) {
    return invoke<mode, n>(std::forward<Func>(func), "",
                           std::forward<Args>(args)...);
  }

  template <internal::InvokeMode mode, int n, typename Func, typename... Args,
            size_t name_size>
  task& invoke(Func&& func, const char (&name)[name_size], Args&&... args) {
    for (int i = 0; i < n; ++i) {
      // Forwarding inside the loop is load-bearing: accessor<T, seq> only
      // matches rvalue `seq&&` and increments the shared position counter on
      // each replica; the accessors never move from the forwarded arguments.
      invoke<mode>(std::forward<Func>(func), std::forward<Args>(args)...);
    }
    return *this;
  }

 private:
  task& invoke_frt(std::shared_ptr<internal::frt::Instance> instance);
};

}  // namespace tapa
