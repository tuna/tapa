// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef TAPA_HOST_STREAM_H_
#define TAPA_HOST_STREAM_H_

#include <cstddef>
#include <cstdint>
#include <cstring>

#include <array>
#include <atomic>
#include <deque>
#include <fstream>
#include <iomanip>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <variant>
#include <vector>

#include <frt.h>
#include <glog/logging.h>

#include <unistd.h>

#include "tapa/base/stream.h"
#include "tapa/host/coroutine.h"
#include "tapa/host/util.h"

namespace tapa {

template <typename T>
class istream;

template <typename T>
class ostream;

template <typename T, uint64_t S>
class istreams;

template <typename T, uint64_t S>
class ostreams;

namespace internal {

template <typename Param, typename Arg>
struct accessor;

class type_erased_queue {
 public:
  virtual ~type_erased_queue() = default;

  // debug helpers
  const std::string& get_name() const;
  void set_name(const std::string& name);

  virtual bool empty() const = 0;
  virtual bool full() const = 0;

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
  virtual fpga::Stream<T>& get_frt_stream() = 0;

 protected:
  using type_erased_queue::type_erased_queue;

  void log_leftovers(int n) final override {
    for (int i = 0; i < n && !this->empty(); ++i) {
      LOG(WARNING) << "channel '" << this->name
                   << "' leftover: " << this->pop();
    }
  }
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
    return val;
  }
  void push(const T& val) override {
    std::unique_lock<std::mutex> lock(this->mtx);
    this->maybe_log(val);
    this->buffer.push_back(val);
  }

  fpga::Stream<T>& get_frt_stream() override {
    LOG(FATAL) << "Cannot pass this stream to FRT: " << this->get_name();
  }

  ~locked_queue() { this->check_leftover(); }
};

// `base_queue` backed by `fpga::Stream`
template <typename T>
class frt_queue : public base_queue<T> {
 public:
  explicit frt_queue(int64_t depth, const std::string& name)
      : base_queue<T>(name), stream_(depth) {}

  ~frt_queue() override { this->check_leftover(); }

  bool empty() const override {
    if (this->stream_.empty()) {
      if (is_frt_arg_.load(std::memory_order_relaxed)) {
        sleep(0);
      }
      return true;
    }
    return false;
  }
  bool full() const override {
    if (this->stream_.full()) {
      if (is_frt_arg_.load(std::memory_order_relaxed)) {
        sleep(0);
      }
      return true;
    }
    return false;
  }
  void push(const T& val) override {
    this->maybe_log(val);
    stream_.push(val);
  }
  T pop() override { return stream_.pop(); };
  T front() const override { return stream_.front(); }

  fpga::Stream<T>& get_frt_stream() override {
    is_frt_arg_.store(true, std::memory_order_relaxed);
    return stream_;
  }

 private:
  fpga::Stream<T> stream_;
  std::atomic<bool> is_frt_arg_ = false;
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
    // EoT: emit an empty line.
  } else if constexpr (has_ostream_overload<T>::value) {
    os << elem.val;
  } else {
    // Dump bytes in hex for types without operator<<.
    auto bytes = bit_cast<std::array<char, sizeof(elem.val)>>(elem.val);
    os << "0x" << std::hex;
    for (int byte : bytes) {
      os << std::setfill('0') << std::setw(2) << byte;
    }
  }
  return os;
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
      internal::yield("channel '" + this->get_name() + "' is empty");
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

  /// Returns true if next token is available and is EoT.
  bool eot(std::nullptr_t) {
    bool eot = false;
    try_eot(eot);
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
    T val;
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
    T val;
    try_read(val);
    return val;
  }

  /// Non-blocking read; returns value or @p default_value; optionally sets @p
  /// is_success.
  T read(const T& default_value, bool* is_success = nullptr) {
    T val;
    bool succeeded = try_read(val);
    if (is_success != nullptr) {
      *is_success = succeeded;
    }
    return succeeded ? val : default_value;
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
    }
  }

 protected:
  istream() : internal::basic_stream<T>() {}

 private:
  template <typename U, uint64_t S>
  friend class istreams;
  template <typename U, uint64_t S, uint64_t N, uint64_t SimulationDepth>
  friend class streams;
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
      internal::yield("channel '" + this->get_name() + "' is full");
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
    }
  }

  /// Blocking write of @p value; returns @c *this.
  ostream& operator<<(const T& value) {
    write(value);
    return *this;
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
    }
  }

 protected:
  ostream() : internal::basic_stream<T>() {}

 private:
  template <typename U, uint64_t S>
  friend class ostreams;
  template <typename U, uint64_t S, uint64_t N, uint64_t SimulationDepth>
  friend class streams;
  ostream(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}
};

/// Communication channel between two task instances.
template <typename T, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
class stream : public internal::unbound_stream<T> {
 public:
  constexpr static int depth = N;

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

  stream(const internal::basic_stream<T>& base)
      : internal::basic_stream<T>(base) {}

  stream(const std::string name, uint64_t simulation_depth = SimulationDepth)
      : internal::basic_stream<T>(name, simulation_depth) {}
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
    CHECK_LT(access_pos_, this->ptr->refs.size())
        << "istream slice '" << this->get_slice_name(length)
        << "' accessed for " << access_pos_ + 1
        << " times but it only contains " << this->ptr->refs.size()
        << " channels";
    return this->ptr->refs[access_pos_++];
  }
  template <uint64_t length>
  istreams<T, length> access() {
    CHECK_NOTNULL(this->ptr.get());
    istreams<T, length> result;
    result.ptr =
        std::make_shared<typename internal::basic_streams<T>::metadata_t>(
            this->ptr->name, this->ptr->pos);
    result.ptr->refs.reserve(length);
    for (uint64_t i = 0; i < length; ++i) {
      result.ptr->refs.emplace_back(access());
    }
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
    CHECK_LT(access_pos_, this->ptr->refs.size())
        << "ostream slice '" << this->get_slice_name(length)
        << "' accessed for " << access_pos_ + 1
        << " times but it only contains " << this->ptr->refs.size()
        << " channels";
    return this->ptr->refs[access_pos_++];
  }
  template <uint64_t length>
  ostreams<T, length> access() {
    CHECK_NOTNULL(this->ptr.get());
    ostreams<T, length> result;
    result.ptr =
        std::make_shared<typename internal::basic_streams<T>::metadata_t>(
            this->ptr->name, this->ptr->pos);
    result.ptr->refs.reserve(length);
    for (uint64_t i = 0; i < length; ++i) {
      result.ptr->refs.emplace_back(access());
    }
    return result;
  }
};

/// Array of communication channels.
template <typename T, uint64_t S, uint64_t N = kStreamDefaultDepth,
          uint64_t SimulationDepth = N>
class streams : public internal::unbound_streams<T, S> {
 public:
  constexpr static int length = S;
  constexpr static int depth = N;

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
  };

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;

  uint64_t istream_access_pos_ = 0;
  uint64_t ostream_access_pos_ = 0;

  istream<T> access_as_istream() {
    CHECK_LT(istream_access_pos_, this->ptr->refs.size())
        << "channels '" << this->ptr->name << "' accessed as istream for "
        << istream_access_pos_ + 1 << " times but it only contains "
        << this->ptr->refs.size() << " channels";
    return this->ptr->refs[istream_access_pos_++];
  }
  ostream<T> access_as_ostream() {
    CHECK_LT(ostream_access_pos_, this->ptr->refs.size())
        << "channels '" << this->ptr->name << "' accessed as ostream for "
        << ostream_access_pos_ + 1 << " times but it only contains "
        << this->ptr->refs.size() << " channels";
    return this->ptr->refs[ostream_access_pos_++];
  }
  template <uint64_t length>
  istreams<T, length> access_as_istreams() {
    CHECK_NOTNULL(this->ptr.get());
    istreams<T, length> result;
    result.ptr =
        std::make_shared<typename internal::basic_streams<T>::metadata_t>(
            this->ptr->name, istream_access_pos_);
    result.ptr->refs.reserve(length);
    for (uint64_t i = 0; i < length; ++i) {
      result.ptr->refs.emplace_back(access_as_istream());
    }
    return result;
  }
  template <uint64_t length>
  ostreams<T, length> access_as_ostreams() {
    CHECK_NOTNULL(this->ptr.get());
    ostreams<T, length> result;
    result.ptr =
        std::make_shared<typename internal::basic_streams<T>::metadata_t>(
            this->ptr->name, ostream_access_pos_);
    result.ptr->refs.reserve(length);
    for (uint64_t i = 0; i < length; ++i) {
      result.ptr->refs.emplace_back(access_as_ostream());
    }
    return result;
  }
};

namespace internal {

// TODO: Remove when all staging builds pass. This is not needed with CWG2518,
// but older compilers do not implement it.
template <typename T>
constexpr bool dependent_false() {
  return false;
}

#define TAPA_DEFINE_DISALLOWED_ACCESSOR(io, arg_ref) /***********************/ \
  /* param = i/ostream, arg = stream */                                        \
  template <typename T, uint64_t N, typename U>                                \
  struct accessor<io##stream<T>, stream<U, N> arg_ref> {                       \
    static io##stream<T> access(stream<U, N> arg_ref arg, bool sequential) {   \
      static_assert(dependent_false<T>(),                                      \
                    "accessing stream as value is disallowed. you must use "   \
                    "\"i/ostream &\" as the formal parameter in a TAPA task"); \
    }                                                                          \
    static void access(fpga::Instance& instance, int& idx,                     \
                       stream<U, N> arg_ref arg) {                             \
      static_assert(dependent_false<T>(),                                      \
                    "accessing stream as value is disallowed. you must use "   \
                    "\"i/ostream &\" as the formal parameter in a TAPA task"); \
    }                                                                          \
  };

// Accessing stream as value (not reference) is forbidden.
TAPA_DEFINE_DISALLOWED_ACCESSOR(i, )
TAPA_DEFINE_DISALLOWED_ACCESSOR(i, &)
TAPA_DEFINE_DISALLOWED_ACCESSOR(o, )
TAPA_DEFINE_DISALLOWED_ACCESSOR(o, &)

#undef TAPA_DEFINE_DISALLOWED_ACCESSOR

#define TAPA_DEFINE_DISALLOWED_ACCESSOR(io)                                    \
  template <typename T>                                                        \
  struct accessor<io##stream<T>, io##stream<T>&> {                             \
    static io##stream<T> access(io##stream<T>& arg, bool sequential) {         \
      static_assert(dependent_false<T>(),                                      \
                    "accessing stream as value is disallowed. you must use "   \
                    "\"i/ostream &\" as the formal parameter in a TAPA task"); \
    }                                                                          \
    static void access(fpga::Instance& instance, int& idx,                     \
                       io##stream<T>& arg) {                                   \
      static_assert(dependent_false<T>(),                                      \
                    "accessing stream as value is disallowed. you must use "   \
                    "\"i/ostream &\" as the formal parameter in a TAPA task"); \
    }                                                                          \
  };

TAPA_DEFINE_DISALLOWED_ACCESSOR(i)
TAPA_DEFINE_DISALLOWED_ACCESSOR(o)

#undef TAPA_DEFINE_DISALLOWED_ACCESSOR

#define TAPA_DEFINE_DEVICE_ACCESSOR(io, arg_ref)                             \
  template <typename T, uint64_t N, typename U>                              \
  struct accessor<io##stream<T>&, stream<U, N> arg_ref> {                    \
    static io##stream<T> access(stream<U, N> arg_ref arg, bool sequential) { \
      return arg;                                                            \
    }                                                                        \
    static void access(fpga::Instance& instance, int& idx,                   \
                       stream<U, N> arg_ref arg) {                           \
      return instance.SetArg(idx++, arg.get_queue().get_frt_stream());       \
    }                                                                        \
  };

TAPA_DEFINE_DEVICE_ACCESSOR(i, )
TAPA_DEFINE_DEVICE_ACCESSOR(i, &)
TAPA_DEFINE_DEVICE_ACCESSOR(o, )
TAPA_DEFINE_DEVICE_ACCESSOR(o, &)
TAPA_DEFINE_DEVICE_ACCESSOR(unbound_, )
TAPA_DEFINE_DEVICE_ACCESSOR(unbound_, &)

#undef TAPA_DEFINE_DEVICE_ACCESSOR

// Pass-through accessors: underlying mechanism differs per io type.

template <typename T>
struct accessor<istream<T>&, istream<T>&> {
  static istream<T> access(istream<T>& arg, bool sequential) { return arg; }
  static void access(fpga::Instance& instance, int& idx, istream<T>& arg) {
    instance.SetArg(idx++, arg.get_queue().get_frt_stream());
  }
};

template <typename T>
struct accessor<ostream<T>&, ostream<T>&> {
  static ostream<T> access(ostream<T>& arg, bool sequential) { return arg; }
  static void access(fpga::Instance& instance, int& idx, ostream<T>& arg) {
    instance.SetArg(idx++, arg.get_queue().get_frt_stream());
  }
};

#define TAPA_DEFINE_ACCESSER(io, reference) /********************************/ \
  /* param = i/ostream, arg = streams */                                       \
  template <typename T, uint64_t length, uint64_t depth>                       \
  struct accessor<io##stream<T> reference, streams<T, length, depth>&> {       \
    static io##stream<T> access(streams<T, length, depth>& arg,                \
                                bool sequential) {                             \
      return arg.access_as_##io##stream();                                     \
    }                                                                          \
    static void access(fpga::Instance& instance, int& idx,                     \
                       streams<T, length, depth>& arg) {                       \
      return instance.SetArg(                                                  \
          idx++, arg.access_as_##io##stream().get_queue().get_frt_stream());   \
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
    static void access(fpga::Instance& instance, int& idx,                     \
                       streams<T, arg_length, depth>& arg) {                   \
      for (int i = 0; i < param_length; ++i) {                                 \
        instance.SetArg(idx++, arg[i].get_queue().get_frt_stream());           \
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

TAPA_DEFINE_ACCESSER(i, )
TAPA_DEFINE_ACCESSER(i, &)
TAPA_DEFINE_ACCESSER(i, &&)
TAPA_DEFINE_ACCESSER(o, )
TAPA_DEFINE_ACCESSER(o, &)
TAPA_DEFINE_ACCESSER(o, &&)

#undef TAPA_DEFINE_ACCESSER

}  // namespace internal

}  // namespace tapa

#endif  // TAPA_HOST_STREAM_H_
