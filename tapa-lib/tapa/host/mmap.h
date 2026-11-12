// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <ostream>

#include <string>
#include <type_traits>
#include <vector>

#include "tapa/base/mmap.h"
#include "tapa/host/coroutine.h"
#include "tapa/host/frt/instance.h"
#include "tapa/host/stream.h"
#include "tapa/host/vec.h"

namespace tapa {

namespace internal {

template <typename Param, typename Arg>
struct accessor;

/// Validates one buffer's reinterpretation from @c T to @c U and returns the
/// converted pointer. Shared by `mmap<T>::reinterpret` and
/// `mmaps<T, S>::reinterpret`, whose elements are each one buffer.
template <typename T, typename U>
U* checked_reinterpret(T* ptr, uint64_t size) {
  static_assert(std::is_standard_layout<T>::value,
                "T must have standard layout");
  static_assert(std::is_standard_layout<U>::value,
                "U must have standard layout");

  if (sizeof(U) > sizeof(T)) {
    constexpr auto N = sizeof(U) / sizeof(T);
    CHECK_EQ(sizeof(U) % sizeof(T), 0)
        << "sizeof(U) must be a multiple of sizeof(T) when mmap<T> is "
           "reinterpreted as mmap<U> (i.e., `reinterpret<U>()` or "
           "`vectorized<N>()`); got "
           "sizeof(U) = "
        << sizeof(U) << ", sizeof(T) = " << sizeof(T);
    CHECK_EQ(size % N, 0)
        << "size of mmap<T> must be a multiple of N (= sizeof(U)/sizeof(T)) "
           "when reinterpreted as mmap<U> (i.e., `reinterpret<U>()` or "
           "`vectorized<N>()`); got "
           "size = "
        << size << ", N = " << sizeof(U) << " / " << sizeof(T) << " = " << N
        << ", but " << size << " % " << N << " != 0";
  } else if (sizeof(U) < sizeof(T)) {
    CHECK_EQ(sizeof(T) % sizeof(U), 0)
        << "sizeof(T) must be a multiple of sizeof(U) when mmap<T> is "
           "reinterpreted as mmap<U> (i.e., `reinterpret<U>()` or "
           "`vectorized<N>()`); got "
           "sizeof(T) = "
        << sizeof(T) << ", sizeof(U) = " << sizeof(U);
  }
  CHECK_EQ(reinterpret_cast<std::size_t>(ptr) % alignof(U), 0)
      << "data of mmap<T> must be " << alignof(U)
      << "-byte aligned when reinterpreted as mmap<U> (i.e., "
         "`reinterpret<U>()` or `vectorized<N>()`) because alignof(U) = "
      << alignof(U) << "; allocate it with tapa::aligned_allocator";
  return reinterpret_cast<U*>(ptr);
}

}  // namespace internal

template <typename T>
class async_mmap;

/// Defines a view of a piece of consecutive memory with synchronous random
/// accesses.
template <typename T>
class mmap {
 public:
  /// Constructs a @c tapa::mmap with unknown size.
  explicit mmap(T* ptr) : ptr_{ptr}, size_{0} {}

  /// Constructs a @c tapa::mmap with the given element count @c size.
  mmap(T* ptr, uint64_t size) : ptr_{ptr}, size_{size} {}

  /// Constructs a @c tapa::mmap from a container implementing @c data()/@c
  /// size().
  template <typename Container>
  explicit mmap(Container& container)
      : ptr_{container.data()}, size_{container.size()} {}

  /// The same memory, read-only, as a `U*` converts to a `const U*`.
  template <typename U, typename std::enable_if<std::is_same<T, const U>::value,
                                                int>::type = 0>
  mmap(const mmap<U>& other)  // NOLINT
      : ptr_{other.data()}, size_{other.size()} {}

  /// Implicitly casts to a raw pointer.
  operator T*() { return ptr_; }

  mmap& operator++() {
    ++ptr_;
    return *this;
  }
  mmap& operator--() {
    --ptr_;
    return *this;
  }
  mmap operator++(int) { return mmap(ptr_++, size_); }
  mmap operator--(int) { return mmap(ptr_--, size_); }

  T* data() const { return ptr_; }
  T* get() const { return ptr_; }
  uint64_t size() const { return size_; }

  /// Reinterprets as @c mmap<vec_t<T,N>>. Size must be a multiple of @c N.
  template <uint64_t N>
  mmap<vec_t<T, N>> vectorized() const {
    return reinterpret<vec_t<T, N>>();
  }

  /// Reinterprets element type as @c U. Both @c T and @c U must have standard
  /// layout; pointer must be properly aligned.
  template <typename U>
  mmap<U> reinterpret() const {
    return mmap<U>(internal::checked_reinterpret<T, U>(get(), size()),
                   size() * sizeof(T) / sizeof(U));
  }

 protected:
  T* ptr_;
  uint64_t size_;
};

inline std::ostream& operator<<(std::ostream& os, const burst& value) {
  return os << value.offset << "+" << value.n;
}

namespace internal {

// An address channel of an async_mmap. It queues bursts; an element index
// is a burst of one.
class request_ostream : public ostream<burst> {
 public:
  explicit request_ostream(const ostream<burst>& channel)
      : basic_stream<burst>(channel), ostream<burst>(channel) {}

  bool try_write(const burst& value) {
    return ostream<burst>::try_write(checked(value));
  }
  void write(const burst& value) { ostream<burst>::write(checked(value)); }
  bool try_write(int64_t index) { return try_write(burst{index, 1}); }
  void write(int64_t index) { write(burst{index, 1}); }
  request_ostream& operator<<(const burst& value) {
    write(value);
    return *this;
  }
  request_ostream& operator<<(int64_t index) {
    write(index);
    return *this;
  }

 private:
  // The hardware request carries a 32-bit count.
  static const burst& checked(const burst& value) {
    CHECK_GE(value.n, 0) << "a burst cannot request " << value.n << " elements";
    CHECK_LT(value.n, int64_t{1} << 32)
        << "a burst holds fewer than 2^32 elements, got " << value.n;
    return value;
  }
};

}  // namespace internal

/// Asynchronous random-access memory view backed by AXI-like channels.
template <typename T>
class async_mmap : public mmap<T> {
 public:
  using addr_t = int64_t;
  using resp_t = uint8_t;

 private:
  using super = mmap<T>;

  // Each channel's log takes the place of the invoke binding the argument
  // (see `internal::binding_log_prefix`).
  stream<burst, 64> read_addr_q_ = channel<burst>("read_addr");
  stream<T, 64> read_data_q_ = channel<T>("read_data");
  stream<burst, 64> write_addr_q_ = channel<burst>("write_addr");
  stream<T, 64> write_data_q_ = channel<T>("write_data");
  stream<resp_t, 64> write_resp_q_ = channel<resp_t>("write_resp");

  template <typename U>
  static stream<U, 64> channel(const std::string& name) {
    return internal::stream_factory::logged<U, 64>(
        name, internal::join_path(internal::binding_log_prefix(), name));
  }

  // A size of 0 is unknown (`mmap(T* ptr)`): only a known one bounds it.
  int64_t checked(int64_t index) const {
    CHECK_GE(index, 0);
    if (this->size_ != 0) {
      CHECK_LT(index, this->size_);
    }
    return index;
  }

  // Only constructible via schedule().
  async_mmap(const super& mem)
      : super(mem),
        read_addr(read_addr_q_),
        read_data(read_data_q_),
        write_addr(write_addr_q_),
        write_data(write_data_q_),
        write_resp(write_resp_q_) {}

  // Direct pointer access is not permitted; use the channel APIs. Deleting
  // the steps also hides the ones `mmap` defines, which would move only this
  // handle's copy of the pointer, not the one its channels serve.
  operator T*() = delete;
  async_mmap& operator++() = delete;
  async_mmap& operator--() = delete;
  async_mmap operator++(int) = delete;
  async_mmap operator--(int) = delete;

 public:
  /// Read address channel: write an element index, or a `burst` of
  /// consecutive elements, to trigger an async read.
  internal::request_ostream read_addr;
  /// Read data channel: read the data returned by a prior read request.
  istream<T> read_data;
  /// Write address channel: write an element index, or a `burst` of
  /// consecutive elements, to trigger an async write.
  internal::request_ostream write_addr;
  /// Write data channel: write data to supply to the pending write request.
  ostream<T> write_data;
  /// Write response channel: read to consume a write-completion
  /// acknowledgement.
  istream<resp_t> write_resp;

  // Serves each request element by element, in order; a write response
  // acknowledges up to 256 element writes, as the hardware bridge does.
  void operator()() { run(nullptr); }

  // The same, until `stop` is set, when it is not null.
  void run(const std::atomic<bool>* stop) {
    int16_t write_count = 0;
    burst read{0, 0};     // what is left of the read request being served
    burst written{0, 0};  // what is left of the write request being served
    while (stop == nullptr || !stop->load(std::memory_order_acquire)) {
      if (read.n <= 0 && !read_addr_q_.empty()) {
        read = read_addr_q_.read();
      }
      if (read.n > 0 && !read_data_q_.full()) {
        read_data_q_.write(this->ptr_[checked(read.offset)]);
        ++read.offset;
        --read.n;
      }
      if (written.n <= 0 && !write_addr_q_.empty()) {
        written = write_addr_q_.read();
      }
      if (write_count != 256 && written.n > 0 && !write_data_q_.empty()) {
        this->ptr_[checked(written.offset)] = write_data_q_.read();
        ++written.offset;
        --written.n;
        ++write_count;
      } else if (write_count > 0 &&
                 this->write_resp_q_.try_write(resp_t(write_count - 1))) {
        CHECK_LE(write_count, 256);
        write_count = 0;
      }
    }
  }

  static async_mmap schedule(super mem) {
    async_mmap async_mem(mem);
    async_mem.serve();
    return async_mem;
  }

  // While a hybrid run records invokes, an async_mmap waits for it to choose
  // who serves its channels: this host memory (`serve`), or an AI Engine
  // task's PLIOs (`bridge_reads` / `bridge_writes`).
  static async_mmap unscheduled(super mem) { return async_mmap(mem); }

  // Serves the channels from host memory.
  void serve() { internal::schedule(/*detach=*/true, *this); }

  // Serves `use(*this)` from host memory, for as long as it runs; whatever
  // it requested is complete when it returns.
  template <typename Use>
  void serve_while(Use&& use) {
    auto stop = std::make_shared<std::atomic<bool>>(false);
    internal::schedule(/*detach=*/true, [server = *this, stop]() mutable {
      server.run(stop.get());
    });
    use(*this);
    stop->store(true, std::memory_order_release);
  }

  // What an AI Engine task's async_mmap does with its reads, in software:
  // each request goes out on `requests`, its data comes back on `data`, and
  // once `returned()` the end request follows (see tapa/aie/mmap.h). Returns
  // when the data of every request is in.
  void bridge_reads(ostream<burst>& requests, istream<T>& data,
                    const std::function<bool()>& returned) {
    int64_t requested = 0;
    int64_t delivered = 0;
    bool ended = false;
    for (;;) {
      if (!read_addr_q_.empty() && !requests.full()) {
        const burst request = read_addr_q_.read();
        requests.write(request);
        requested += request.n;
      } else if (!ended && returned() && read_addr_q_.empty()) {
        requests.write(internal::end_of_requests());
        ended = true;
      }
      if (!data.empty() && !read_data_q_.full()) {
        read_data_q_.write(data.read());
        ++delivered;
      }
      if (ended && delivered == requested) return;
    }
  }

  // The same for writes: each request goes out on `requests` as the elements
  // it occupies, its data behind it, and acknowledgements come back on
  // `responses` when the body reads them (null otherwise). Returns once the
  // end request is out.
  void bridge_writes(ostream<T>& requests, istream<uint32_t>* responses,
                     const std::function<bool()>& returned) {
    static_assert(sizeof(burst) % sizeof(T) == 0,
                  "an AI Engine memory element is 4, 8 or 16 bytes");
    using parts_t = std::array<T, sizeof(burst) / sizeof(T)>;
    const auto send = [&](const burst& request) {
      for (const T& part : bit_cast<parts_t>(request)) requests.write(part);
    };
    int64_t left = 0;  // elements of data still to go for the last request
    bool ended = false;
    while (!ended) {
      if (left == 0 && !write_addr_q_.empty()) {
        const burst request = write_addr_q_.read();
        send(request);
        left = request.n;
      } else if (left > 0 && !write_data_q_.empty() && !requests.full()) {
        requests.write(write_data_q_.read());
        --left;
      } else if (left == 0 && returned() && write_addr_q_.empty()) {
        send(internal::end_of_requests());
        ended = true;
      }
      if (responses != nullptr && !responses->empty() &&
          !write_resp_q_.full()) {
        write_resp_q_.write(static_cast<resp_t>(responses->read()));
      }
    }
  }
};

/// An array of @c tapa::mmap.
template <typename T, uint64_t S>
class mmaps {
 protected:
  std::vector<mmap<T>> mmaps_;

 public:
  template <typename PtrContainer, typename SizeContainer>
  mmaps(const PtrContainer& pointers, const SizeContainer& sizes) {
    for (uint64_t i = 0; i < S; ++i) {
      mmaps_.emplace_back(pointers[i], sizes[i]);
    }
  }

  template <typename Container>
  explicit mmaps(Container& container) {
    for (uint64_t i = 0; i < S; ++i) {
      mmaps_.emplace_back(container[i]);
    }
  }

  mmap<T>& operator[](int idx) { return mmaps_[idx]; }

  template <uint64_t offset, uint64_t length>
  mmaps<T, length> slice() {
    static_assert(offset <= S && length <= S - offset, "invalid slice");
    std::array<T*, length> pointers;
    std::array<uint64_t, length> sizes;
    for (uint64_t i = 0; i < length; ++i) {
      pointers[i] = mmaps_[offset + i].data();
      sizes[i] = mmaps_[offset + i].size();
    }
    return mmaps<T, length>(pointers, sizes);
  }

  /// Reinterprets each element as @c vec_t<T,N>. Each size must be a multiple
  /// of @c N.
  template <uint64_t N>
  mmaps<vec_t<T, N>, S> vectorized() const {
    return reinterpret<vec_t<T, N>>();
  }

  /// Reinterprets each element type as @c U.
  template <typename U>
  mmaps<U, S> reinterpret() const {
    std::array<U*, S> ptrs;
    std::array<uint64_t, S> sizes;
    for (uint64_t i = 0; i < S; ++i) {
      ptrs[i] = internal::checked_reinterpret<T, U>(mmaps_[i].get(),
                                                    mmaps_[i].size());
      sizes[i] = mmaps_[i].size() * sizeof(T) / sizeof(U);
    }
    return mmaps<U, S>(ptrs, sizes);
  }

 private:
  template <typename Param, typename Arg>
  friend struct internal::accessor;

  uint64_t access_pos_ = 0;

  mmap<T> access() {
    LOG_IF(WARNING, access_pos_ >= S)
        << "invocation #" << access_pos_ << " accesses mmaps["
        << access_pos_ % S << "]";
    return mmaps_[access_pos_++ % S];
  }
};

template <typename T, int chan_count, int64_t chan_size>
class hmap : public mmap<T> {
 private:
  using super = mmap<T>;

 public:
  hmap(const super& mem) : super(mem) {
    CHECK_EQ(chan_size * chan_count, this->size())
        << "hmap<T, " << chan_count << ", " << chan_size
        << "> must have size = " << chan_size * chan_count << ", got "
        << this->size();
  }
};

// Every buffer-access tag gets a subclass of `mmap` that keeps the tag
// through `vectorized()` and `reinterpret()`.
#define TAPA_DEFINE_TAGGED_MMAP(tag)                            \
  template <typename T>                                         \
  class tag##_mmap : public mmap<T> {                           \
   public:                                                      \
    using mmap<T>::mmap;                                        \
    tag##_mmap(const mmap<T>& base) : mmap<T>(base) {}          \
                                                                \
    template <uint64_t N>                                       \
    tag##_mmap<vec_t<T, N>> vectorized() const {                \
      const auto view = mmap<T>::template vectorized<N>();      \
      return tag##_mmap<vec_t<T, N>>(view.data(), view.size()); \
    }                                                           \
    template <typename U>                                       \
    tag##_mmap<U> reinterpret() const {                         \
      const auto view = mmap<T>::template reinterpret<U>();     \
      return tag##_mmap<U>(view.data(), view.size());           \
    }                                                           \
  }
TAPA_DEFINE_TAGGED_MMAP(placeholder);
TAPA_DEFINE_TAGGED_MMAP(read_only);
TAPA_DEFINE_TAGGED_MMAP(write_only);
TAPA_DEFINE_TAGGED_MMAP(read_write);
#undef TAPA_DEFINE_TAGGED_MMAP

// `mmaps` carries an extra size parameter, so it does not fit the macro above.
#define TAPA_DEFINE_MMAPS(tag)                                  \
  template <typename T, uint64_t S>                             \
  class tag##_mmaps : public mmaps<T, S> {                      \
   public:                                                      \
    using mmaps<T, S>::mmaps;                                   \
    tag##_mmaps(const mmaps<T, S>& base) : mmaps<T, S>(base) {} \
                                                                \
    template <uint64_t N>                                       \
    tag##_mmaps<vec_t<T, N>, S> vectorized() const {            \
      return mmaps<T, S>::template vectorized<N>();             \
    }                                                           \
    template <typename U>                                       \
    tag##_mmaps<U, S> reinterpret() const {                     \
      return mmaps<T, S>::template reinterpret<U>();            \
    }                                                           \
  }
TAPA_DEFINE_MMAPS(placeholder);
TAPA_DEFINE_MMAPS(read_only);
TAPA_DEFINE_MMAPS(write_only);
TAPA_DEFINE_MMAPS(read_write);
#undef TAPA_DEFINE_MMAPS

namespace internal {

// Whether a hybrid run is recording invokes (see `hybrid_run`), which then
// chooses who serves each async_mmap.
bool recording_invokes();

// A memory parameter, which binds only through a buffer accessor below.
template <typename T>
struct is_memory_param : std::false_type {};
template <typename T>
struct is_memory_param<mmap<T>> : std::true_type {};
template <typename T, uint64_t S>
struct is_memory_param<mmaps<T, S>> : std::true_type {};
template <typename T, int chan_count, int64_t chan_size>
struct is_memory_param<hmap<T, chan_count, chan_size>> : std::true_type {};
template <typename T>
struct is_memory_param<async_mmap<T>> : std::true_type {};

template <typename T>
struct accessor<async_mmap<T>&, mmap<T>&> {
  static async_mmap<T> access(mmap<T>& arg) {
    return recording_invokes() ? async_mmap<T>::unscheduled(arg)
                               : async_mmap<T>::schedule(arg);
  }
};

template <typename T, uint64_t S>
struct accessor<mmap<T>, mmaps<T, S>&> {
  static mmap<T> access(mmaps<T, S>& arg) { return arg.access(); }
};

template <typename T, uint64_t S>
struct accessor<mmap<const T>, mmaps<T, S>&> {
  static mmap<const T> access(mmaps<T, S>& arg) { return arg.access(); }
};

template <typename T, uint64_t S>
struct accessor<async_mmap<T>&, mmaps<T, S>&> {
  static async_mmap<T> access(mmaps<T, S>& arg) {
    return recording_invokes() ? async_mmap<T>::unscheduled(arg.access())
                               : async_mmap<T>::schedule(arg.access());
  }
};

// An hmap hands the kernel one buffer per channel, once its constructor has
// checked the size as software simulation does. The bitstream access takes
// any instance with `SetBufferArg`, so a test can record the buffers.
#define TAPA_DEFINE_ACCESSOR(tag, tag_ref, buffer_tag)                        \
  template <typename T>                                                       \
  struct accessor<mmap<T>, tag##mmap<T> tag_ref> {                            \
    static mmap<T> access(tag##mmap<T> tag_ref arg) { return arg; }           \
    template <typename Instance>                                              \
    static void access(Instance& instance, int& idx,                          \
                       tag##mmap<T> tag_ref arg) {                            \
      instance.SetBufferArg(idx++, arg.get(), arg.size() * sizeof(T),         \
                            buffer_tag);                                      \
    }                                                                         \
  };                                                                          \
  /* A read-only parameter takes the same buffer, as `T*` converts. */        \
  template <typename T>                                                       \
  struct accessor<mmap<const T>, tag##mmap<T> tag_ref>                        \
      : accessor<mmap<T>, tag##mmap<T> tag_ref> {                             \
    using accessor<mmap<T>, tag##mmap<T> tag_ref>::access;                    \
    static mmap<const T> access(tag##mmap<T> tag_ref arg) { return arg; }     \
  };                                                                          \
  template <typename T, uint64_t S>                                           \
  struct accessor<mmaps<T, S>, tag##mmaps<T, S> tag_ref> {                    \
    /* The whole array, its regions handed out afresh from the first. */      \
    static mmaps<T, S> access(tag##mmaps<T, S> tag_ref arg) {                 \
      return arg.template slice<0, S>();                                      \
    }                                                                         \
    static void access(frt::Instance& instance, int& idx,                     \
                       tag##mmaps<T, S> tag_ref arg) {                        \
      for (uint64_t i = 0; i < S; ++i) {                                      \
        instance.SetBufferArg(idx++, arg[i].get(), arg[i].size() * sizeof(T), \
                              buffer_tag);                                    \
      }                                                                       \
    }                                                                         \
  };                                                                          \
  template <typename T, int chan_count, int64_t chan_size>                    \
  struct accessor<hmap<T, chan_count, chan_size>, tag##mmap<T> tag_ref> {     \
    static hmap<T, chan_count, chan_size> access(tag##mmap<T> tag_ref arg) {  \
      return arg;                                                             \
    }                                                                         \
    template <typename Instance>                                              \
    static void access(Instance& instance, int& idx,                          \
                       tag##mmap<T> tag_ref arg) {                            \
      const hmap<T, chan_count, chan_size> mem(arg);                          \
      for (int i = 0; i < chan_count; ++i) {                                  \
        instance.SetBufferArg(idx++, mem.data() + i * chan_size,              \
                              chan_size * sizeof(T), buffer_tag);             \
      }                                                                       \
    }                                                                         \
  }
// The accessor names and `BufferAccess` share the kernel's view: a
// `read_only_mmap` is read by the kernel, so the host has to load it.
TAPA_DEFINE_ACCESSOR(placeholder_, , BufferAccess::PlaceHolder);

// mmap accessors
TAPA_DEFINE_ACCESSOR(read_only_, , BufferAccess::ReadOnly);
TAPA_DEFINE_ACCESSOR(write_only_, , BufferAccess::WriteOnly);
TAPA_DEFINE_ACCESSOR(read_write_, , BufferAccess::ReadWrite);

// A named untagged mmap or mmaps, which the kernel may read and write.
TAPA_DEFINE_ACCESSOR(, &, BufferAccess::ReadWrite);

#undef TAPA_DEFINE_ACCESSOR

// Named directional handles use the same buffer lowering as temporaries.
// Keep this forwarding specific to mmap handles: stream and seq arguments
// carry reference identity in their accessor specializations.
#define TAPA_FORWARD_TAGGED_MMAP_ACCESSOR(tag, qualifier)                  \
  template <typename T>                                                    \
  struct accessor<mmap<T>, tag##_mmap<T> qualifier>                        \
      : accessor<mmap<T>, tag##_mmap<T>> {};                               \
  template <typename T>                                                    \
  struct accessor<mmap<const T>, tag##_mmap<T> qualifier>                  \
      : accessor<mmap<const T>, tag##_mmap<T>> {};                         \
  template <typename T, uint64_t S>                                        \
  struct accessor<mmaps<T, S>, tag##_mmaps<T, S> qualifier>                \
      : accessor<mmaps<T, S>, tag##_mmaps<T, S>> {};                       \
  template <typename T, int chan_count, int64_t chan_size>                 \
  struct accessor<hmap<T, chan_count, chan_size>, tag##_mmap<T> qualifier> \
      : accessor<hmap<T, chan_count, chan_size>, tag##_mmap<T>> {}

#define TAPA_FORWARD_TAGGED_MMAP_FAMILY(tag)      \
  TAPA_FORWARD_TAGGED_MMAP_ACCESSOR(tag, &);      \
  TAPA_FORWARD_TAGGED_MMAP_ACCESSOR(tag, const&); \
  TAPA_FORWARD_TAGGED_MMAP_ACCESSOR(tag, const)

TAPA_FORWARD_TAGGED_MMAP_FAMILY(placeholder);
TAPA_FORWARD_TAGGED_MMAP_FAMILY(read_only);
TAPA_FORWARD_TAGGED_MMAP_FAMILY(write_only);
TAPA_FORWARD_TAGGED_MMAP_FAMILY(read_write);

#undef TAPA_FORWARD_TAGGED_MMAP_FAMILY
#undef TAPA_FORWARD_TAGGED_MMAP_ACCESSOR

// An async_mmap parameter names five channel endpoints with identity; a
// formal task parameter must take it by lvalue reference. The same
// catch-all trick as the stream accessors: any use fires the static_assert
// at the offending `tapa::task().invoke(...)` site.
template <typename T, typename Arg>
struct accessor<async_mmap<T>, Arg> {
  static async_mmap<T> access(Arg&&) {
    static_assert(dependent_false<Arg>(),
                  "tapa::async_mmap<T>& must be passed by reference as a "
                  "TAPA task parameter");
  }
  static void access(frt::Instance&, int&, Arg&&) {
    static_assert(dependent_false<Arg>(),
                  "tapa::async_mmap<T>& must be passed by reference as a "
                  "TAPA task parameter");
  }
};
template <typename T, typename Arg>
struct accessor<async_mmap<T>&&, Arg> {
  static async_mmap<T>&& access(Arg&&) {
    static_assert(dependent_false<Arg>(),
                  "tapa::async_mmap<T>& must be passed by reference as a "
                  "TAPA task parameter");
  }
  static void access(frt::Instance&, int&, Arg&&) {
    static_assert(dependent_false<Arg>(),
                  "tapa::async_mmap<T>& must be passed by reference as a "
                  "TAPA task parameter");
  }
};

// mmap-family parameters are pointer-like handles: the kernel receives the
// base address by value. A formal task parameter by (rvalue or lvalue)
// reference is rejected here for every arg kind.
#define TAPA_DISALLOWED_VALUE_PARAM_BODY(name)                          \
  static Self access(Arg&&) {                                           \
    static_assert(dependent_false<Arg>(),                               \
                  "tapa::" name                                         \
                  " must be passed by value as a TAPA task parameter"); \
  }                                                                     \
  static void access(frt::Instance&, int&, Arg&&) {                     \
    static_assert(dependent_false<Arg>(),                               \
                  "tapa::" name                                         \
                  " must be passed by value as a TAPA task parameter"); \
  }

// `Self` carries the comma-bearing specialized param type outside macro
// argument lists.
template <typename T, typename Arg>
struct accessor<mmap<T>&, Arg> {
  using Self = mmap<T>&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("mmap<T>")
};
template <typename T, typename Arg>
struct accessor<mmap<T>&&, Arg> {
  using Self = mmap<T>&&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("mmap<T>")
};
template <typename T, uint64_t S, typename Arg>
struct accessor<mmaps<T, S>&, Arg> {
  using Self = mmaps<T, S>&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("mmaps<T, S>")
};
template <typename T, uint64_t S, typename Arg>
struct accessor<mmaps<T, S>&&, Arg> {
  using Self = mmaps<T, S>&&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("mmaps<T, S>")
};
template <typename T, int chan_count, int64_t chan_size, typename Arg>
struct accessor<hmap<T, chan_count, chan_size>&, Arg> {
  using Self = hmap<T, chan_count, chan_size>&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("hmap<T, chan_count, chan_size>")
};
template <typename T, int chan_count, int64_t chan_size, typename Arg>
struct accessor<hmap<T, chan_count, chan_size>&&, Arg> {
  using Self = hmap<T, chan_count, chan_size>&&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("hmap<T, chan_count, chan_size>")
};

#undef TAPA_DISALLOWED_VALUE_PARAM_BODY

// Passing mmap/mmaps by value in tapa::invoke is an error; must use typed
// variants.
template <typename T>
struct accessor<mmap<T>, mmap<T>> {
  static_assert(dependent_false<T>(),
                "must use one of "
                "placeholder_mmap/read_only_mmap/write_only_mmap/"
                "read_write_mmap in tapa::invoke");
};
template <typename T, uint64_t S>
struct accessor<mmaps<T, S>, mmaps<T, S>> {
  static_assert(dependent_false<T>(),
                "must use one of "
                "placeholder_mmaps/read_only_mmaps/write_only_mmaps/"
                "read_write_mmaps in tapa::invoke");
};

}  // namespace internal

}  // namespace tapa
