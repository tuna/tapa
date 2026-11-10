// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <fstream>
#include <functional>
#include <iomanip>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#include <glog/logging.h>

#include "tapa/base/stream.h"
#include "tapa/host/coroutine.h"
#include "tapa/host/frt/instance.h"
#include "tapa/host/frt/stream.h"
#include "tapa/host/util.h"

namespace tapa {

// Software simulation runs every task in program order.
inline void fence() {}

template <typename T>
class istream;

template <typename T>
class ostream;

template <typename T, uint64_t S>
class istreams;

template <typename T, uint64_t S>
class ostreams;

template <typename T, uint64_t N>
class iframes;

template <typename T, uint64_t N>
class oframes;

namespace internal {

template <typename Param, typename Arg>
struct accessor;

struct stream_access;

class type_erased_queue {
 public:
  virtual ~type_erased_queue() = default;

  // debug helpers
  const std::string& get_name() const;
  void set_name(const std::string& name);

  virtual bool empty() const = 0;
  virtual bool full() const = 0;

  // Whether this channel is bound to a kernel instance, making that
  // instance the only party that can drain or fill it.
  virtual bool is_kernel_bound() const { return false; }

  // A hybrid forwarder marks its destination when it has drained and exited;
  // no later transfer can then complete, even if other kernels are running.
  void mark_kernel_done() {
    kernel_done_.store(true, std::memory_order_release);
  }
  bool is_kernel_done() const {
    return kernel_done_.load(std::memory_order_acquire);
  }

 protected:
  // Pops up to `n` elements and logs them as leftovers.
  virtual void log_leftovers(int n) = 0;

  struct LogContext {
    static std::unique_ptr<LogContext> New(std::string_view name);
    std::ofstream ofs;
    std::mutex mtx;
  };

  std::string name;
  const std::unique_ptr<LogContext> log;
  std::atomic<bool> kernel_done_ = false;

  type_erased_queue(const std::string& name);

  void check_leftover();

  template <typename T>
  void maybe_log(const T& elem) {
    if (this->log != nullptr) {
      std::unique_lock<std::mutex> lock(this->log->mtx);
      this->log->ofs << elem << std::endl;
    }
  }
};

template <typename T>
class base_queue : public type_erased_queue {
 public:
  virtual void push(const T& val) = 0;
  virtual T pop() = 0;
  virtual T front() const = 0;

  // Returns a stream suitable for passing to FRT. Crashes if incompatible.
  virtual frt::Stream<T>& get_frt_stream() = 0;

  // Hands every element pushed from now on to `recorder` as well, or with
  // `pops` every element popped (an AI Engine replay, see `hybrid_run`);
  // set before any task uses the queue.
  void set_recorder(std::function<void(const T&)> recorder, bool pops) {
    recorder_ = std::move(recorder);
    record_pops_ = pops;
  }

 protected:
  using type_erased_queue::type_erased_queue;

  // What every push shows the stream log and a recorder of pushes.
  void observe(const T& elem) {
    this->maybe_log(elem);
    if (recorder_ && !record_pops_) recorder_(elem);
  }

  // What every pop shows a recorder of pops.
  void observe_pop(const T& elem) {
    if (recorder_ && record_pops_) recorder_(elem);
  }

  void log_leftovers(int n) final override {
    for (int i = 0; i < n && !this->empty(); ++i) {
      LOG(WARNING) << "channel '" << this->name
                   << "' leftover: " << this->pop();
    }
  }

 private:
  std::function<void(const T&)> recorder_;
  bool record_pops_ = false;
};

template <typename T>
class locked_queue : public base_queue<T> {
  size_t depth;
  mutable std::mutex mtx;
  std::deque<T> buffer;

 public:
  locked_queue(size_t depth, const std::string& name)
      : base_queue<T>(name), depth(depth) {}

  bool empty() const override {
    std::unique_lock<std::mutex> lock(this->mtx);
    return this->buffer.empty();
  }
  bool full() const override {
    std::unique_lock<std::mutex> lock(this->mtx);
    return this->buffer.size() >= this->depth;
  }
  T front() const override {
    std::unique_lock<std::mutex> lock(this->mtx);
    return this->buffer.front();
  }
  T pop() override {
    std::unique_lock<std::mutex> lock(this->mtx);
    auto val = this->buffer.front();
    this->buffer.pop_front();
    this->observe_pop(val);
    return val;
  }
  void push(const T& val) override {
    std::unique_lock<std::mutex> lock(this->mtx);
    this->observe(val);
    this->buffer.push_back(val);
  }

  frt::Stream<T>& get_frt_stream() override {
    LOG(FATAL) << "stream '" << this->get_name()
               << "' has infinite depth and cannot be passed to an FRT "
                  "instance; declare it with a finite depth to use it as a "
                  "kernel argument";
  }

  ~locked_queue() { this->check_leftover(); }
};

// `base_queue` backed by `frt::Stream`
template <typename T>
class frt_queue : public base_queue<T> {
 public:
  explicit frt_queue(int64_t depth, const std::string& name)
      : base_queue<T>(name), stream_(depth) {}

  ~frt_queue() override { this->check_leftover(); }

  bool empty() const override { return poll(stream_.empty()); }
  bool full() const override { return poll(stream_.full()); }
  void push(const T& val) override {
    this->observe(val);
    stream_.push(val);
  }
  T pop() override {
    T val = stream_.pop();
    this->observe_pop(val);
    return val;
  }
  T front() const override { return stream_.front(); }

  frt::Stream<T>& get_frt_stream() override {
    bound_at_.store(frt_instances_scheduled(), std::memory_order_relaxed);
    is_frt_arg_.store(true, std::memory_order_relaxed);
    return stream_;
  }

  // Bound once the instance this stream was handed to has actually been
  // scheduled: from then on that instance is the only party that fills or
  // drains it, which is what lets `check_kernel_can_still_serve` call a
  // blocked operation unservable.
  //
  // Arguments are bound before `schedule_frt_instance` counts the instance,
  // so between the two a second sequential invoke would otherwise look like
  // "bound, and everything finished" and abort a healthy run. Comparing
  // against the total at bind time closes that window.
  bool is_kernel_bound() const override {
    return is_frt_arg_.load(std::memory_order_relaxed) &&
           frt_instances_scheduled() >
               bound_at_.load(std::memory_order_relaxed);
  }

 private:
  // A bound stream whose poll comes back blocked is spinning on the serving
  // instance; yield so that instance can run.
  bool poll(bool blocked) const {
    if (blocked && is_frt_arg_.load(std::memory_order_relaxed)) {
      std::this_thread::yield();
    }
    return blocked;
  }

  frt::Stream<T> stream_;
  std::atomic<bool> is_frt_arg_ = false;
  std::atomic<int> bound_at_ = 0;
};

template <typename T>
std::shared_ptr<base_queue<T>> make_queue(uint64_t depth,
                                          const std::string& name) {
  if (depth == ::tapa::kStreamInfiniteDepth) {
    VLOG(1) << "channel '" << name << "' created as a locked queue";
    return std::make_shared<locked_queue<T>>(depth, name);
  }
  VLOG(1) << "channel '" << name << "' created as a lock-free queue";
  return std::make_shared<frt_queue<T>>(depth, name);
}

template <typename T>
class basic_stream {
 public:
  const std::string& get_name() const { return get_queue().get_name(); }
  void set_name(const std::string& name) { get_queue().set_name(name); }

  // Not protected: used in std::vector<basic_stream<T>>.
  basic_stream() {}
  basic_stream(const std::string& name, uint64_t depth)
      : queue(make_queue<elem_t<T>>(depth, name)) {}

  basic_stream(const basic_stream&) = default;
  basic_stream(basic_stream&&) = default;
  basic_stream& operator=(const basic_stream&) = default;
  basic_stream& operator=(basic_stream&&) = delete;  // -Wvirtual-move-assign

 protected:
  base_queue<elem_t<T>>& get_queue() const { return *CHECK_NOTNULL(queue); }

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;
  friend struct internal::stream_access;

  std::shared_ptr<base_queue<elem_t<T>>> queue;
};

template <typename T>
class basic_streams {
 protected:
  struct metadata_t {
    metadata_t(const std::string& name, int pos) : name(name), pos(pos) {}
    std::vector<basic_stream<T>> refs;  // references to the original streams
    const std::string name;             // name of the original streams
    const int pos;                      // position in the original streams
  };

  basic_streams(const std::shared_ptr<metadata_t>& ptr) : ptr({ptr}) {}
  basic_streams(const basic_streams&) = default;
  basic_streams(basic_streams&&) = default;
  basic_streams& operator=(const basic_streams&) = default;
  basic_streams& operator=(basic_streams&&) = delete;  // -Wvirtual-move-assign

  basic_stream<T> operator[](int pos) const {
    CHECK_NOTNULL(ptr.get());
    CHECK_GE(pos, 0);
    CHECK_LT(pos, ptr->refs.size());
    return ptr->refs[pos];
  }

  std::string get_slice_name(int length) {
    return ptr->name + "[" + std::to_string(ptr->pos) + ":" +
           std::to_string(ptr->pos + length) + ")";
  }

  // Hands out the channel at `pos` and advances it. `accessed()` describes
  // the access for the out-of-range error, e.g. "istream slice 'q[0:2)'
  // accessed" or "channels 'q' accessed as istream"; it runs only on error.
  template <typename Describe>
  basic_stream<T> access_next(uint64_t& pos, const Describe& accessed) {
    CHECK_LT(pos, ptr->refs.size())
        << accessed() << " for " << pos + 1 << " times but it only contains "
        << ptr->refs.size() << " channels";
    return ptr->refs[pos++];
  }

  // Builds the metadata of a slice of the next `length` channels, named at
  // `slice_pos`, handing the channels out from `pos` as in `access_next`.
  template <typename Describe>
  std::shared_ptr<metadata_t> access_slice(uint64_t& pos, int slice_pos,
                                           uint64_t length,
                                           const Describe& accessed) {
    auto slice = std::make_shared<metadata_t>(ptr->name, slice_pos);
    slice->refs.reserve(length);
    for (uint64_t i = 0; i < length; ++i) {
      slice->refs.emplace_back(access_next(pos, accessed));
    }
    return slice;
  }

  std::shared_ptr<metadata_t> ptr;
};

// stream/streams with no bound depth; default-constructible by derived classes.
template <typename T>
class unbound_stream : public istream<T>, public ostream<T> {
 protected:
  unbound_stream() : basic_stream<T>() {}
};

template <typename T, int S>
class unbound_streams : public istreams<T, S>, public ostreams<T, S> {
 protected:
  unbound_streams() : basic_streams<T>(nullptr) {}
};

template <typename T, typename = void>
struct has_ostream_overload : std::false_type {};

template <typename T>
struct has_ostream_overload<
    T,
    std::void_t<decltype(std::declval<std::ostream&>() << std::declval<T>())>>
    : std::true_type {};

template <typename T>
std::ostream& operator<<(std::ostream& os, const elem_t<T>& elem) {
  if (elem.eot) {
    // EoT: nothing to display for the value.
  } else if constexpr (has_ostream_overload<T>::value) {
    os << elem.val;
  } else {
    auto bytes =
        bit_cast<std::array<unsigned char, sizeof(elem.val)>>(elem.val);
    auto flags = os.flags();
    os << "0x" << std::hex;
    for (unsigned char b : bytes) {
      os << std::setfill('0') << std::setw(2) << static_cast<unsigned>(b);
    }
    os.flags(flags);
  }
  return os;
}

/// Abort a blocked channel operation that can never complete.
///
/// A channel bound to a kernel instance is filled or drained only by that
/// instance. Once every scheduled instance has finished — the simulator
/// exited, the device returned — a blocked operation on such a channel has
/// no one left to serve it, and waiting forever hides the real failure.
template <typename T>
void check_kernel_can_still_serve(const base_queue<T>& queue,
                                  const char* waiting_for,
                                  bool (type_erased_queue::*is_blocked)()
                                      const) {
  // A blocked poll yields before returning its readiness snapshot. The
  // kernel may complete its final transfer and finish during that yield.
  const bool unservable =
      queue.is_kernel_done() ||
      (queue.is_kernel_bound() && every_frt_instance_finished());
  if (unservable && (queue.*is_blocked)()) {
    LOG(FATAL) << "channel '" << queue.get_name() << "' is still "
               << waiting_for
               << " after every kernel instance finished; the kernel stopped "
                  "before the transfer completed";
  }
}

}  // namespace internal

/// Consumer-side view of a @c tapa::stream. Use only as a task parameter.
template <typename T>
class istream : virtual public internal::basic_stream<T> {
 public:
  /// Returns true if the stream is empty (non-blocking, non-destructive).
  bool empty() {
    bool is_empty = this->get_queue().empty();
    if (is_empty) {
      internal::yield(this->get_name(), "empty");
    } else {
      internal::note_poll_progress();
    }
    return is_empty;
  }

  /// Sets @p is_eot and returns true if next token is available (non-blocking,
  /// non-destructive).
  bool try_eot(bool& is_eot) {
    if (!empty()) {
      is_eot = this->get_queue().front().eot;
      return true;
    }
    return false;
  }

  /// Returns true if next token is EoT; sets @p is_success to availability.
  bool eot(bool& is_success) {
    bool eot = false;
    is_success = try_eot(eot);
    return eot;
  }

  /// Sets @p value to the next non-EoT token and returns true if available
  /// (non-blocking).
  bool try_peek(T& value) {
    if (!empty()) {
      auto elem = this->get_queue().front();
      if (elem.eot) {
        LOG(FATAL) << "channel '" << this->get_name() << "' peeked when closed";
      }
      value = elem.val;
      return true;
    }
    return false;
  }

  /// Returns next token value and sets @p is_success (non-blocking, non-EoT).
  T peek(bool& is_success) {
    T val;
    is_success = try_peek(val);
    return val;
  }

  /// Returns next token value or @c T() if empty (non-blocking, non-EoT).
  T peek(std::nullptr_t) {
    T val{};
    try_peek(val);
    return val;
  }

  /// Returns next token and sets @p is_success and @p is_eot (non-blocking).
  T peek(bool& is_success, bool& is_eot) {
    if (!empty()) {
      auto elem = this->get_queue().front();
      is_success = true;
      is_eot = elem.eot;
      return elem.val;
    }
    is_success = false;
    is_eot = false;
    return {};
  }

  /// Sets @p value and returns true if a non-EoT token was popped
  /// (non-blocking).
  bool try_read(T& value) {
    if (!empty()) {
      auto elem = this->get_queue().pop();
      if (elem.eot) {
        LOG(FATAL) << "channel '" << this->get_name() << "' read when closed";
      }
      value = elem.val;
      return true;
    }
    return false;
  }

  /// Blocking read; returns the next non-EoT token value.
  T read() {
    T val;
    while (!try_read(val)) {
      internal::check_kernel_can_still_serve(
          this->get_queue(), "empty", &internal::type_erased_queue::empty);
    }
    return val;
  }

  /// Blocking read into @p value; returns @c *this.
  istream& operator>>(T& value) {
    value = read();
    return *this;
  }

  /// Non-blocking read; returns value and sets @p is_success.
  T read(bool& is_success) {
    T val;
    is_success = try_read(val);
    return val;
  }

  /// Non-blocking read; returns value or @c T() if empty.
  T read(std::nullptr_t) {
    T val{};
    try_read(val);
    return val;
  }

  /// Blocking read of @p n values of @c U into @p dst, lined up with the
  /// elements byte for byte (see `internal::bulk_layout`).
  template <typename U>
  void read(U* dst, int n) {
    internal::bulk_read<T>(*this, dst, n);
  }

  /// Non-blocking consume of an EoT token; returns true if consumed.
  bool try_open() {
    if (!empty()) {
      auto elem = this->get_queue().pop();
      if (!elem.eot) {
        LOG(FATAL) << "channel '" << this->get_name()
                   << "' opened when not closed";
      }
      return true;
    }
    return false;
  }

  /// Blocking consume of an EoT token.
  void open() {
    while (!try_open()) {
      internal::check_kernel_can_still_serve(
          this->get_queue(), "empty", &internal::type_erased_queue::empty);
    }
  }

 protected:
  istream() : internal::basic_stream<T>() {}

 private:
  template <typename U, uint64_t S>
  friend class istreams;
  template <typename U, uint64_t S, uint64_t N, uint64_t SimulationDepth>
  friend class streams;
  template <typename U, uint64_t N>
  friend class iframes;
  istream(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}
};

/// Producer-side view of a @c tapa::stream. Use only as a task parameter.
template <typename T>
class ostream : virtual public internal::basic_stream<T> {
 public:
  /// Returns true if the stream is full (non-blocking, non-destructive).
  bool full() {
    bool is_full = this->get_queue().full();
    if (is_full) {
      internal::yield(this->get_name(), "full");
    } else {
      internal::note_poll_progress();
    }
    return is_full;
  }

  /// Non-blocking write; returns true on success.
  bool try_write(const T& value) {
    if (!full()) {
      this->get_queue().push({value, false});
      return true;
    }
    return false;
  }

  /// Blocking write of @p value.
  void write(const T& value) {
    while (!try_write(value)) {
      internal::check_kernel_can_still_serve(
          this->get_queue(), "full", &internal::type_erased_queue::full);
    }
  }

  /// Blocking write of @p value; returns @c *this.
  ostream& operator<<(const T& value) {
    write(value);
    return *this;
  }

  /// Blocking write of @p n values of @c U from @p src, lined up with the
  /// elements byte for byte (see `internal::bulk_layout`).
  template <typename U>
  void write(const U* src, int n) {
    internal::bulk_write<T>(*this, src, n);
  }

  /// Non-blocking write of an EoT token; returns true on success.
  bool try_close() {
    if (!full()) {
      this->get_queue().push({{}, true});
      return true;
    }
    return false;
  }

  /// Blocking write of an EoT token.
  void close() {
    while (!try_close()) {
      internal::check_kernel_can_still_serve(
          this->get_queue(), "full", &internal::type_erased_queue::full);
    }
  }

 protected:
  ostream() : internal::basic_stream<T>() {}

 private:
  template <typename U, uint64_t S>
  friend class ostreams;
  template <typename U, uint64_t S, uint64_t N, uint64_t SimulationDepth>
  friend class streams;
  template <typename U, uint64_t N>
  friend class oframes;
  ostream(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}
};

/// Consumer-side frame port of a @c tapa::stream: the stream arrives as
/// frames of @p N elements, each read whole and in any order through the
/// view `acquire()` returns. Use only as a task parameter.
///
/// Software simulation copies a frame out of the stream when it is acquired,
/// so the stream holds up to its depth besides the frame each end holds.
template <typename T, uint64_t N>
class iframes : virtual public internal::basic_stream<T> {
  static_assert(N > 0, "a frame holds at least one element");

 public:
  /// One frame, read-only, until `release()` or the end of its scope.
  class view {
   public:
    view(view&& other) noexcept
        : port_(std::exchange(other.port_, nullptr)),
          data_(std::move(other.data_)) {}
    view(const view&) = delete;
    view& operator=(const view&) = delete;
    view& operator=(view&&) = delete;
    ~view() { release(); }

    const T& operator[](int i) const {
      CHECK(port_ != nullptr) << "a released frame of a frame port was read";
      CHECK(i >= 0 && static_cast<uint64_t>(i) < N)
          << "frame port '" << port_->get_name() << "' read at " << i
          << ", outside its frame of " << N;
      return data_[i];
    }

    /// Hands the frame back to the stream before the view's scope ends.
    void release() {
      if (port_ != nullptr) std::exchange(port_, nullptr)->live_ = false;
    }

   private:
    friend class iframes;
    explicit view(iframes& port) : port_(&port), data_(N) {}
    iframes* port_;
    std::vector<T> data_;
  };

  /// Blocks until a whole frame has arrived and returns it.
  view acquire() {
    CHECK(!live_) << "frame port '" << this->get_name()
                  << "' acquired while a frame of it is still held; release "
                     "one before acquiring the next";
    view frame(*this);
    istream<T> in(*this);
    for (uint64_t i = 0; i < N; ++i) {
      bool is_eot = false;
      while (!in.try_eot(is_eot)) {
        internal::check_kernel_can_still_serve(
            this->get_queue(), "empty", &internal::type_erased_queue::empty);
      }
      CHECK(!is_eot) << "frame port '" << this->get_name()
                     << "' received an end-of-transmission token; a frame "
                        "port's stream carries none";
      frame.data_[i] = in.read();
    }
    live_ = true;
    return frame;
  }

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;
  iframes(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}
  bool live_ = false;
};

/// Producer-side frame port of a @c tapa::stream: the task fills each frame
/// of @p N elements in any order through the view `acquire()` returns, and
/// the frame enters the stream when the view is released. Use only as a
/// task parameter.
template <typename T, uint64_t N>
class oframes : virtual public internal::basic_stream<T> {
  static_assert(N > 0, "a frame holds at least one element");

 public:
  /// One frame, write-only, until `release()` or the end of its scope. A
  /// slot the task does not write holds an unspecified value.
  class view {
   public:
    /// Element `i` of the frame; it can only be assigned.
    class slot {
     public:
      slot& operator=(const T& value) {
        *value_ = value;
        return *this;
      }

     private:
      friend class view;
      explicit slot(T* value) : value_(value) {}
      T* value_;
    };

    view(view&& other) noexcept
        : port_(std::exchange(other.port_, nullptr)),
          data_(std::move(other.data_)) {}
    view(const view&) = delete;
    view& operator=(const view&) = delete;
    view& operator=(view&&) = delete;
    ~view() { release(); }

    slot operator[](int i) {
      CHECK(port_ != nullptr) << "a released frame of a frame port was written";
      CHECK(i >= 0 && static_cast<uint64_t>(i) < N)
          << "frame port '" << port_->get_name() << "' written at " << i
          << ", outside its frame of " << N;
      return slot(&data_[i]);
    }

    /// Puts the frame on the stream before the view's scope ends.
    void release() {
      if (port_ == nullptr) return;
      oframes* port = std::exchange(port_, nullptr);
      ostream<T> out(*port);
      for (const T& value : data_) out.write(value);
      port->live_ = false;
    }

   private:
    friend class oframes;
    explicit view(oframes& port) : port_(&port), data_(N) {}
    oframes* port_;
    std::vector<T> data_;
  };

  /// Returns an empty frame to fill.
  view acquire() {
    CHECK(!live_) << "frame port '" << this->get_name()
                  << "' acquired while a frame of it is still held; release "
                     "one before acquiring the next";
    live_ = true;
    return view(*this);
  }

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;
  oframes(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}
  bool live_ = false;
};

namespace internal {
struct stream_factory;
}  // namespace internal

/// Communication channel between two task instances.
template <typename T, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
class stream : public internal::unbound_stream<T> {
 public:
  constexpr static uint64_t depth = N;

  stream() : internal::basic_stream<T>("", SimulationDepth) {}

  /// Constructs with @p name for debugging.
  template <size_t S>
  stream(const char (&name)[S])
      : internal::basic_stream<T>(name, SimulationDepth) {}

 private:
  template <typename U, uint64_t friend_length, uint64_t friend_depth,
            uint64_t friend_simulation_depth>
  friend class streams;
  template <typename Param, typename Arg>
  friend struct internal::accessor;
  friend struct internal::stream_factory;

  stream(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}
};

/// Consumer-side view of a @c tapa::stream array. Use only as a task parameter.
template <typename T, uint64_t S>
class istreams : virtual public internal::basic_streams<T> {
 public:
  constexpr static int length = S;

  istream<T> operator[](int pos) const {
    return internal::basic_streams<T>::operator[](pos);
  }

 protected:
  istreams() : internal::basic_streams<T>(nullptr) {}

  template <typename U, uint64_t friend_length, uint64_t friend_depth,
            uint64_t friend_simulation_depth>
  friend class streams;

  template <typename U, uint64_t friend_length>
  friend class istreams;

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;

  uint64_t access_pos_ = 0;

  istream<T> access() {
    return this->access_next(access_pos_, [&] {
      return "istream slice '" + this->get_slice_name(length) + "' accessed";
    });
  }
  template <uint64_t length>
  istreams<T, length> access() {
    CHECK_NOTNULL(this->ptr.get());
    istreams<T, length> result;
    result.ptr = this->access_slice(access_pos_, this->ptr->pos, length, [&] {
      return "istream slice '" + this->get_slice_name(istreams::length) +
             "' accessed";
    });
    return result;
  }
};

/// Producer-side view of a @c tapa::stream array. Use only as a task parameter.
template <typename T, uint64_t S>
class ostreams : virtual public internal::basic_streams<T> {
 public:
  constexpr static int length = S;

  ostream<T> operator[](int pos) const {
    return internal::basic_streams<T>::operator[](pos);
  }

 protected:
  ostreams() : internal::basic_streams<T>(nullptr) {}

  template <typename U, uint64_t friend_length, uint64_t friend_depth,
            uint64_t friend_simulation_depth>
  friend class streams;

  template <typename U, uint64_t friend_length>
  friend class ostreams;

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;

  uint64_t access_pos_ = 0;

  ostream<T> access() {
    return this->access_next(access_pos_, [&] {
      return "ostream slice '" + this->get_slice_name(length) + "' accessed";
    });
  }
  template <uint64_t length>
  ostreams<T, length> access() {
    CHECK_NOTNULL(this->ptr.get());
    ostreams<T, length> result;
    result.ptr = this->access_slice(access_pos_, this->ptr->pos, length, [&] {
      return "ostream slice '" + this->get_slice_name(ostreams::length) +
             "' accessed";
    });
    return result;
  }
};

/// Array of communication channels.
template <typename T, uint64_t S, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
class streams : public internal::unbound_streams<T, S> {
 public:
  constexpr static uint64_t length = S;
  constexpr static uint64_t depth = N;

  streams()
      : internal::basic_streams<T>(
            std::make_shared<typename internal::basic_streams<T>::metadata_t>(
                "", 0)) {
    for (uint64_t i = 0; i < S; ++i) {
      this->ptr->refs.emplace_back("", SimulationDepth);
    }
  }

  /// Constructs with @p name as base name; each element is named @c name[i].
  template <size_t name_length>
  streams(const char (&name)[name_length])
      : internal::basic_streams<T>(
            std::make_shared<typename internal::basic_streams<T>::metadata_t>(
                name, 0)) {
    for (uint64_t i = 0; i < S; ++i) {
      this->ptr->refs.emplace_back(
          std::string(name) + "[" + std::to_string(i) + "]", SimulationDepth);
    }
  }

  stream<T, N> operator[](int pos) const {
    return internal::basic_streams<T>::operator[](pos);
  }

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;

  uint64_t istream_access_pos_ = 0;
  uint64_t ostream_access_pos_ = 0;

  istream<T> access_as_istream() {
    return this->access_next(istream_access_pos_, [&] {
      return "channels '" + this->ptr->name + "' accessed as istream";
    });
  }
  ostream<T> access_as_ostream() {
    return this->access_next(ostream_access_pos_, [&] {
      return "channels '" + this->ptr->name + "' accessed as ostream";
    });
  }
  template <uint64_t length>
  istreams<T, length> access_as_istreams() {
    CHECK_NOTNULL(this->ptr.get());
    istreams<T, length> result;
    result.ptr = this->access_slice(
        istream_access_pos_, istream_access_pos_, length, [&] {
          return "channels '" + this->ptr->name + "' accessed as istream";
        });
    return result;
  }
  template <uint64_t length>
  ostreams<T, length> access_as_ostreams() {
    CHECK_NOTNULL(this->ptr.get());
    ostreams<T, length> result;
    result.ptr = this->access_slice(
        ostream_access_pos_, ostream_access_pos_, length, [&] {
          return "channels '" + this->ptr->name + "' accessed as ostream";
        });
    return result;
  }
};

namespace internal {

// Makes the static_asserts below dependent on the template parameter so
// they only fire on instantiation. CWG2518 would allow plain
// `static_assert(false)`, but the staging matrix includes compilers too
// old to implement it.
template <typename T>
constexpr bool dependent_false() {
  return false;
}

// A stream parameter names a channel endpoint with identity, so only an
// lvalue reference may be a formal stream parameter of a TAPA task. These
// catch-all specializations subsume every (param, arg) combination whose
// param is a by-value or rvalue-reference stream or stream array; any use
// fires the static_assert at the offending `tapa::task().invoke(...)` site.
// More specialized allowed accessors (param = lvalue reference) win by
// partial ordering.
#define TAPA_DISALLOWED_REFERENCE_PARAM_BODY(name)                          \
  static Self access(Arg&&, bool) {                                         \
    static_assert(dependent_false<Arg>(),                                   \
                  "tapa::" name                                             \
                  " must be passed by reference as a TAPA task parameter"); \
  }                                                                         \
  static void access(frt::Instance&, int&, Arg&&) {                         \
    static_assert(dependent_false<Arg>(),                                   \
                  "tapa::" name                                             \
                  " must be passed by reference as a TAPA task parameter"); \
  }

template <typename T, typename Arg>
struct accessor<istream<T>, Arg> {
  using Self = istream<T>;
  TAPA_DISALLOWED_REFERENCE_PARAM_BODY("istream<T>&")
};
template <typename T, typename Arg>
struct accessor<istream<T>&&, Arg> {
  using Self = istream<T>&&;
  TAPA_DISALLOWED_REFERENCE_PARAM_BODY("istream<T>&")
};
template <typename T, typename Arg>
struct accessor<ostream<T>, Arg> {
  using Self = ostream<T>;
  TAPA_DISALLOWED_REFERENCE_PARAM_BODY("ostream<T>&")
};
template <typename T, typename Arg>
struct accessor<ostream<T>&&, Arg> {
  using Self = ostream<T>&&;
  TAPA_DISALLOWED_REFERENCE_PARAM_BODY("ostream<T>&")
};
template <typename T, uint64_t L, typename Arg>
struct accessor<istreams<T, L>, Arg> {
  using Self = istreams<T, L>;
  TAPA_DISALLOWED_REFERENCE_PARAM_BODY("istreams<T, L>&")
};
template <typename T, uint64_t L, typename Arg>
struct accessor<ostreams<T, L>, Arg> {
  using Self = ostreams<T, L>;
  TAPA_DISALLOWED_REFERENCE_PARAM_BODY("ostreams<T, L>&")
};

// The copy of a stream an AI Engine replay binds (see `hybrid_run`): every
// element pushed on the stream, or popped from it (see `stream_use`),
// without its end-of-transmission tokens,
// until `close` ends the copy with one. A later push, from a detached task,
// is dropped.
template <typename T>
class stream_copy {
 public:
  explicit stream_copy(const std::string& name)
      : queue_(kDepth, name + ".copy"), path_(queue_.get_frt_stream().path()) {}

  const std::string& path() const { return path_; }

  void push(const elem_t<T>& elem) {
    if (elem.eot) return;
    std::lock_guard<std::mutex> lock(mtx_);
    if (!closed_) put(elem);
  }

  void close() {
    std::lock_guard<std::mutex> lock(mtx_);
    if (closed_) return;
    put({T{}, true});
    closed_ = true;
  }

 private:
  static constexpr int64_t kDepth = 1024;

  // The instance drains the copy on a thread of its own from the moment it
  // runs, so waiting here for room never waits on another task.
  void put(const elem_t<T>& elem) {
    while (queue_.full()) std::this_thread::yield();
    queue_.push(elem);
  }

  frt_queue<elem_t<T>> queue_;
  const std::string path_;
  std::mutex mtx_;
  bool closed_ = false;
};

// One stream a recorded invoke touches (hybrid cosimulation, see
// `hybrid_run`): the queue it names, the stream's name, whether the task
// reads it, and how to hand it to a kernel argument. `frt_path` binds the
// stream itself. `frt_path_dropping_eot` binds a fresh queue for the kernel
// to write and sets `forward` to the task that moves its elements into the
// stream with the end-of-transmission bit cleared, until the kernel is done
// and the queue drained: a stream into the AI Engine asserts TLAST on every
// beat, which the cosimulation reads as EoT. `close` ends the stream with
// an EoT token, which tells an AI Engine simulator its input is complete.
// `frt_path_counting` binds a fresh queue for the kernel to read and sets
// `forward` to the task that moves what tasks push on the stream into it,
// then, once `written` and the stream is drained, calls `moved` with how
// many elements it moved: software's frames on an AI Engine top's output
// ring, whose count the kernel waits for (see `HostStream::ring`).
// `copy` starts a `stream_copy` of what tasks push on the stream, or with
// `reads` of what they pop from it, and returns its path and the function
// that closes it.
struct stream_use {
  const void* queue;
  std::string name;
  bool task_reads;
  std::function<std::string()> frt_path;
  std::function<std::string(std::function<void()>& forward,
                            std::function<bool()> kernel_done)>
      frt_path_dropping_eot;
  std::function<std::string(std::function<void()>& forward,
                            std::function<bool()> written,
                            std::function<void(uint64_t)> moved)>
      frt_path_counting;
  std::function<void()> close;
  std::function<std::pair<std::string, std::function<void()>>(bool reads)> copy;
};

struct stream_access {
  template <typename T>
  static stream_use use(const basic_stream<T>& stream, bool task_reads) {
    const std::string name = stream.get_queue().get_name();
    return {&stream.get_queue(),
            name,
            task_reads,
            [stream] { return stream.get_queue().get_frt_stream().path(); },
            [stream, name](std::function<void()>& forward,
                           std::function<bool()> kernel_done) {
              auto beats = std::make_shared<frt_queue<elem_t<T>>>(
                  kHybridForwardDepth, name + ".beats");
              const std::string path = beats->get_frt_stream().path();
              forward = [beats, stream, name, kernel_done] {
                for (;;) {
                  while (beats->empty()) {
                    if (kernel_done()) {
                      stream.get_queue().mark_kernel_done();
                      return;
                    }
                    yield(name, "empty");
                  }
                  const elem_t<T> beat = beats->pop();
                  while (stream.get_queue().full()) yield(name, "full");
                  stream.get_queue().push({beat.val, false});
                }
              };
              return path;
            },
            [stream, name](std::function<void()>& forward,
                           std::function<bool()> written,
                           std::function<void(uint64_t)> moved) {
              auto beats = std::make_shared<frt_queue<elem_t<T>>>(
                  kHybridForwardDepth, name + ".beats");
              const std::string path = beats->get_frt_stream().path();
              forward = [beats, stream, name, written, moved] {
                uint64_t count = 0;
                for (;;) {
                  while (stream.get_queue().empty()) {
                    // Done once the writer returned and left nothing.
                    if (written() && stream.get_queue().empty()) {
                      moved(count);
                      return;
                    }
                    yield(name, "empty");
                  }
                  const elem_t<T> elem = stream.get_queue().pop();
                  while (beats->full()) yield(name, "full");
                  beats->push(elem);
                  if (!elem.eot) ++count;
                }
              };
              return path;
            },
            [stream, name] {
              while (stream.get_queue().full()) yield(name, "full");
              stream.get_queue().push({T{}, true});
            },
            [stream, name](bool reads) {
              auto copy = std::make_shared<stream_copy<T>>(name);
              stream.get_queue().set_recorder(
                  [copy](const elem_t<T>& elem) { copy->push(elem); }, reads);
              return std::make_pair(copy->path(), std::function<void()>([copy] {
                                      copy->close();
                                    }));
            }};
  }
  static constexpr int64_t kHybridForwardDepth = 64;
};

// Whether `V` is a `tapa::stream`.
template <typename V>
struct is_tapa_stream : std::false_type {};
template <typename T, uint64_t N, uint64_t SimulationDepth>
struct is_tapa_stream<stream<T, N, SimulationDepth>> : std::true_type {};

// A `tapa::stream` named at run time, which the host makes for itself (see
// `hybrid_run`): the type of `like`, a stream of its own.
struct stream_factory {
  template <typename T, uint64_t N, uint64_t SimulationDepth>
  static std::shared_ptr<stream<T, N, SimulationDepth>> named(
      const stream<T, N, SimulationDepth>& like, const std::string& name) {
    (void)like;
    return std::shared_ptr<stream<T, N, SimulationDepth>>(
        new stream<T, N, SimulationDepth>(
            basic_stream<T>(name, SimulationDepth)));
  }
};

// The streams one task argument touches; none for a non-stream argument.
template <typename V, typename = void>
struct stream_uses {
  static void collect(const V&, std::vector<stream_use>&) {}
};
template <typename T>
struct stream_uses<istream<T>> {
  static void collect(const istream<T>& v, std::vector<stream_use>& out) {
    out.push_back(stream_access::use(v, /*task_reads=*/true));
  }
};
template <typename T>
struct stream_uses<ostream<T>> {
  static void collect(const ostream<T>& v, std::vector<stream_use>& out) {
    out.push_back(stream_access::use(v, /*task_reads=*/false));
  }
};
template <typename T, uint64_t N>
struct stream_uses<iframes<T, N>> {
  static void collect(const iframes<T, N>& v, std::vector<stream_use>& out) {
    out.push_back(stream_access::use(v, /*task_reads=*/true));
  }
};
template <typename T, uint64_t N>
struct stream_uses<oframes<T, N>> {
  static void collect(const oframes<T, N>& v, std::vector<stream_use>& out) {
    out.push_back(stream_access::use(v, /*task_reads=*/false));
  }
};
template <typename T, uint64_t S>
struct stream_uses<istreams<T, S>> {
  static void collect(const istreams<T, S>& v, std::vector<stream_use>& out) {
    for (uint64_t i = 0; i < S; ++i)
      stream_uses<istream<T>>::collect(v[i], out);
  }
};
template <typename T, uint64_t S>
struct stream_uses<ostreams<T, S>> {
  static void collect(const ostreams<T, S>& v, std::vector<stream_use>& out) {
    for (uint64_t i = 0; i < S; ++i)
      stream_uses<ostream<T>>::collect(v[i], out);
  }
};

#define TAPA_DEFINE_DEVICE_ACCESSOR(io, arg_ref)                             \
  template <typename T, uint64_t N, typename U>                              \
  struct accessor<io##stream<T>&, stream<U, N> arg_ref> {                    \
    static io##stream<T> access(stream<U, N> arg_ref arg, bool sequential) { \
      return arg;                                                            \
    }                                                                        \
    static void access(frt::Instance& instance, int& idx,                    \
                       stream<U, N> arg_ref arg) {                           \
      return instance.SetStreamArg(idx++,                                    \
                                   arg.get_queue().get_frt_stream().path()); \
    }                                                                        \
  };

TAPA_DEFINE_DEVICE_ACCESSOR(i, )
TAPA_DEFINE_DEVICE_ACCESSOR(i, &)
TAPA_DEFINE_DEVICE_ACCESSOR(o, )
TAPA_DEFINE_DEVICE_ACCESSOR(o, &)
#undef TAPA_DEFINE_DEVICE_ACCESSOR

// param = i/oframes, arg = stream: a frame port of a leaf task.
#define TAPA_DEFINE_FRAMES_ACCESSOR(io, arg_ref)                         \
  template <typename T, uint64_t F, uint64_t D, uint64_t SD>             \
  struct accessor<io##frames<T, F>&, stream<T, D, SD> arg_ref> {         \
    static io##frames<T, F> access(stream<T, D, SD> arg_ref arg, bool) { \
      return io##frames<T, F>(arg);                                      \
    }                                                                    \
  };

TAPA_DEFINE_FRAMES_ACCESSOR(i, )
TAPA_DEFINE_FRAMES_ACCESSOR(i, &)
TAPA_DEFINE_FRAMES_ACCESSOR(o, )
TAPA_DEFINE_FRAMES_ACCESSOR(o, &)
#undef TAPA_DEFINE_FRAMES_ACCESSOR

// param = i/oframes, arg = an upper task's own stream port: a frame port
// whose stream the upper task takes from its invoker.
#define TAPA_DEFINE_PORT_FRAMES_ACCESSOR(io)                   \
  template <typename T, uint64_t F>                            \
  struct accessor<io##frames<T, F>&, io##stream<T>&> {         \
    static io##frames<T, F> access(io##stream<T>& arg, bool) { \
      return io##frames<T, F>(arg);                            \
    }                                                          \
  };

TAPA_DEFINE_PORT_FRAMES_ACCESSOR(i)
TAPA_DEFINE_PORT_FRAMES_ACCESSOR(o)
#undef TAPA_DEFINE_PORT_FRAMES_ACCESSOR

// Pass-through accessors: underlying mechanism differs per io type.

template <typename T>
struct accessor<istream<T>&, istream<T>&> {
  static istream<T> access(istream<T>& arg, bool sequential) { return arg; }
  static void access(frt::Instance& instance, int& idx, istream<T>& arg) {
    instance.SetStreamArg(idx++, arg.get_queue().get_frt_stream().path());
  }
};

template <typename T>
struct accessor<ostream<T>&, ostream<T>&> {
  static ostream<T> access(ostream<T>& arg, bool sequential) { return arg; }
  static void access(frt::Instance& instance, int& idx, ostream<T>& arg) {
    instance.SetStreamArg(idx++, arg.get_queue().get_frt_stream().path());
  }
};

#define TAPA_DEFINE_ACCESSOR(io, reference) /********************************/ \
  /* param = i/ostream, arg = streams */                                       \
  template <typename T, uint64_t length, uint64_t depth>                       \
  struct accessor<io##stream<T> reference, streams<T, length, depth>&> {       \
    static io##stream<T> access(streams<T, length, depth>& arg,                \
                                bool sequential) {                             \
      return arg.access_as_##io##stream();                                     \
    }                                                                          \
    static void access(frt::Instance& instance, int& idx,                      \
                       streams<T, length, depth>& arg) {                       \
      return instance.SetStreamArg(                                            \
          idx++,                                                               \
          arg.access_as_##io##stream().get_queue().get_frt_stream().path());   \
    }                                                                          \
  };                                                                           \
                                                                               \
  /* param = i/ostream, arg = i/ostreams */                                    \
  template <typename T, uint64_t length>                                       \
  struct accessor<io##stream<T> reference, io##streams<T, length>&> {          \
    static io##stream<T> access(io##streams<T, length>& arg, bool) {           \
      return arg.access();                                                     \
    }                                                                          \
  };                                                                           \
                                                                               \
  /* param = i/ostreams, arg = streams */                                      \
  template <typename T, uint64_t param_length, uint64_t arg_length,            \
            uint64_t depth>                                                    \
  struct accessor<io##streams<T, param_length> reference,                      \
                  streams<T, arg_length, depth>&> {                            \
    static io##streams<T, param_length> access(                                \
        streams<T, arg_length, depth>& arg, bool sequential) {                 \
      return arg.template access_as_##io##streams<param_length>();             \
    }                                                                          \
    static void access(frt::Instance& instance, int& idx,                      \
                       streams<T, arg_length, depth>& arg) {                   \
      for (int i = 0; i < param_length; ++i) {                                 \
        instance.SetStreamArg(idx++,                                           \
                              arg[i].get_queue().get_frt_stream().path());     \
      }                                                                        \
    }                                                                          \
  };                                                                           \
                                                                               \
  /* param = i/ostreams, arg = i/ostreams */                                   \
  template <typename T, uint64_t param_length, uint64_t arg_length>            \
  struct accessor<io##streams<T, param_length> reference,                      \
                  io##streams<T, arg_length>&> {                               \
    static io##streams<T, param_length> access(                                \
        io##streams<T, arg_length>& arg, bool) {                               \
      return arg.template access<param_length>();                              \
    }                                                                          \
  };

TAPA_DEFINE_ACCESSOR(i, &)

TAPA_DEFINE_ACCESSOR(o, &)

#undef TAPA_DEFINE_ACCESSOR

}  // namespace internal

}  // namespace tapa
