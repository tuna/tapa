// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <ostream>

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

template <typename T>
class immap : public mmap<T> {
 public:
  using mmap<T>::mmap;
};

template <typename T>
class ommap : public mmap<T> {
 public:
  using mmap<T>::mmap;
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

  stream<burst, 64> read_addr_q_{"read_addr"};
  stream<T, 64> read_data_q_{"read_data"};
  stream<burst, 64> write_addr_q_{"write_addr"};
  stream<T, 64> write_data_q_{"write_data"};
  stream<resp_t, 64> write_resp_q_{"write_resp"};

  int64_t checked(int64_t index) const {
    CHECK_GE(index, 0);
    if (index != 0) {
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
  void operator()() {
    int16_t write_count = 0;
    burst read{0, 0};     // what is left of the read request being served
    burst written{0, 0};  // what is left of the write request being served
    for (;;) {
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
    internal::schedule(/*detach=*/true, async_mem);
    return async_mem;
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

  mmaps(const mmaps&) = default;
  mmaps(mmaps&&) = default;
  mmaps& operator=(const mmaps&) = default;
  mmaps& operator=(mmaps&&) = default;

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

// Every buffer-access tag gets a subclass of each single-buffer handle, and
// they differ only in which handle they extend.
#define TAPA_DEFINE_TAGGED_MMAP(tag, kind)                        \
  template <typename T>                                           \
  class tag##_##kind : public kind<T> {                           \
   public:                                                        \
    using kind<T>::kind;                                          \
    tag##_##kind(const kind<T>& base) : kind<T>(base) {}          \
                                                                  \
    template <uint64_t N>                                         \
    tag##_##kind<vec_t<T, N>> vectorized() const {                \
      const auto view = kind<T>::template vectorized<N>();        \
      return tag##_##kind<vec_t<T, N>>(view.data(), view.size()); \
    }                                                             \
    template <typename U>                                         \
    tag##_##kind<U> reinterpret() const {                         \
      const auto view = kind<T>::template reinterpret<U>();       \
      return tag##_##kind<U>(view.data(), view.size());           \
    }                                                             \
  }

#define TAPA_DEFINE_TAGGED_MMAP_FAMILY(kind)  \
  TAPA_DEFINE_TAGGED_MMAP(placeholder, kind); \
  TAPA_DEFINE_TAGGED_MMAP(read_only, kind);   \
  TAPA_DEFINE_TAGGED_MMAP(write_only, kind);  \
  TAPA_DEFINE_TAGGED_MMAP(read_write, kind)
TAPA_DEFINE_TAGGED_MMAP_FAMILY(mmap);
TAPA_DEFINE_TAGGED_MMAP_FAMILY(immap);
TAPA_DEFINE_TAGGED_MMAP_FAMILY(ommap);
#undef TAPA_DEFINE_TAGGED_MMAP_FAMILY
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

template <typename T>
struct accessor<async_mmap<T>&, mmap<T>&> {
  static async_mmap<T> access(mmap<T>& arg, bool) {
    return async_mmap<T>::schedule(arg);
  }
};

template <typename T, uint64_t S>
struct accessor<mmap<T>, mmaps<T, S>&> {
  static mmap<T> access(mmaps<T, S>& arg, bool) { return arg.access(); }
};

template <typename T, uint64_t S>
struct accessor<async_mmap<T>&, mmaps<T, S>&> {
  static async_mmap<T> access(mmaps<T, S>& arg, bool) {
    return async_mmap<T>::schedule(arg.access());
  }
};

#define TAPA_DEFINE_ACCESSOR(tag, tag_ref, buffer_tag)                        \
  template <typename T>                                                       \
  struct accessor<mmap<T>, tag##mmap<T> tag_ref> {                            \
    static mmap<T> access(tag##mmap<T> tag_ref arg, bool) { return arg; }     \
    static void access(frt::Instance& instance, int& idx,                     \
                       tag##mmap<T> tag_ref arg) {                            \
      instance.SetBufferArg(idx++, arg.get(), arg.size() * sizeof(T),         \
                            buffer_tag);                                      \
    }                                                                         \
  };                                                                          \
  template <typename T, uint64_t S>                                           \
  struct accessor<mmaps<T, S>, tag##mmaps<T, S> tag_ref> {                    \
    static void access(frt::Instance& instance, int& idx,                     \
                       tag##mmaps<T, S> tag_ref arg) {                        \
      for (uint64_t i = 0; i < S; ++i) {                                      \
        instance.SetBufferArg(idx++, arg[i].get(), arg[i].size() * sizeof(T), \
                              buffer_tag);                                    \
      }                                                                       \
    }                                                                         \
  };                                                                          \
  template <typename T, int chan_count, int64_t chan_size>                    \
  struct accessor<hmap<T, chan_count, chan_size>, tag##mmap<T tag_ref>> {     \
    static void access(frt::Instance& instance, int& idx,                     \
                       tag##mmap<T> tag_ref arg) {                            \
      for (int i = 0; i < chan_count; ++i) {                                  \
        instance.SetBufferArg(idx++, &arg[i * chan_size],                     \
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

// mmaps accessors
TAPA_DEFINE_ACCESSOR(, &, BufferAccess::ReadWrite);

#undef TAPA_DEFINE_ACCESSOR

// Named directional handles use the same buffer lowering as temporaries.
// Keep this forwarding specific to mmap handles: stream and seq arguments
// carry reference identity in their accessor specializations.
#define TAPA_FORWARD_TAGGED_MMAP_ACCESSOR(tag, qualifier)                     \
  template <typename T>                                                       \
  struct accessor<mmap<T>, tag##_mmap<T> qualifier>                           \
      : accessor<mmap<T>, tag##_mmap<T>> {};                                  \
  template <typename T, uint64_t S>                                           \
  struct accessor<mmaps<T, S>, tag##_mmaps<T, S> qualifier>                   \
      : accessor<mmaps<T, S>, tag##_mmaps<T, S>> {                            \
    using accessor<mmaps<T, S>, tag##_mmaps<T, S>>::access;                   \
    static mmaps<T, S> access(tag##_mmaps<T, S> qualifier arg, bool) {        \
      return arg;                                                             \
    }                                                                         \
  };                                                                          \
  template <typename T, int chan_count, int64_t chan_size>                    \
  struct accessor<hmap<T, chan_count, chan_size>, tag##_mmap<T> qualifier>    \
      : accessor<hmap<T, chan_count, chan_size>, tag##_mmap<T>> {             \
    using accessor<hmap<T, chan_count, chan_size>, tag##_mmap<T>>::access;    \
    static hmap<T, chan_count, chan_size> access(tag##_mmap<T> qualifier arg, \
                                                 bool) {                      \
      return arg;                                                             \
    }                                                                         \
  }

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
  static async_mmap<T> access(Arg&&, bool) {
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
  static async_mmap<T>&& access(Arg&&, bool) {
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
  static Self access(Arg&&, bool) {                                     \
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
template <typename T, typename Arg>
struct accessor<immap<T>&, Arg> {
  using Self = immap<T>&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("immap<T>")
};
template <typename T, typename Arg>
struct accessor<immap<T>&&, Arg> {
  using Self = immap<T>&&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("immap<T>")
};
template <typename T, typename Arg>
struct accessor<ommap<T>&, Arg> {
  using Self = ommap<T>&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("ommap<T>")
};
template <typename T, typename Arg>
struct accessor<ommap<T>&&, Arg> {
  using Self = ommap<T>&&;
  TAPA_DISALLOWED_VALUE_PARAM_BODY("ommap<T>")
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
