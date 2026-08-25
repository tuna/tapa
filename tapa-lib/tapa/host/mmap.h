// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#pragma once

#include <cstddef>
#include <cstdint>

#include <type_traits>
#include <vector>

#include "tapa/host/coroutine.h"
#include "tapa/host/frt/instance.h"
#include "tapa/host/stream.h"
#include "tapa/host/vec.h"

namespace tapa {

namespace internal {

template <typename Param, typename Arg>
struct accessor;

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
    CHECK_EQ(size() % N, 0)
        << "size of mmap<T> must be a multiple of N when vectorized as a "
           "mmap<vec_t<T, N>> (i.e., `vectorized<N>()`); got size = "
        << size() << ", N = " << N << ", but " << size() << " % " << N
        << " != 0";
    return mmap<vec_t<T, N>>(reinterpret_cast<vec_t<T, N>*>(get()), size() / N);
  }

  /// Reinterprets element type as @c U. Both @c T and @c U must have standard
  /// layout; pointer must be properly aligned.
  template <typename U>
  mmap<U> reinterpret() const {
    static_assert(std::is_standard_layout<T>::value,
                  "T must have standard layout");
    static_assert(std::is_standard_layout<U>::value,
                  "U must have standard layout");

    if (sizeof(U) > sizeof(T)) {
      constexpr auto N = sizeof(U) / sizeof(T);
      CHECK_EQ(sizeof(U) % sizeof(T), 0)
          << "sizeof(U) must be a multiple of sizeof(T) when mmap<T> is "
             "reinterpreted as mmap<U> (i.e., `reinterpret<U>()`); got "
             "sizeof(U) = "
          << sizeof(U) << ", sizeof(T) = " << sizeof(T);
      CHECK_EQ(size() % N, 0)
          << "size of mmap<T> must be a multiple of N (= sizeof(U)/sizeof(T)) "
             "when reinterpreted as mmap<U> (i.e., `reinterpret<U>()`); got "
             "size = "
          << size() << ", N = " << sizeof(U) << " / " << sizeof(T) << " = " << N
          << ", but " << size() << " % " << N << " != 0";
    } else if (sizeof(U) < sizeof(T)) {
      CHECK_EQ(sizeof(T) % sizeof(U), 0)
          << "sizeof(T) must be a multiple of sizeof(U) when mmap<T> is "
             "reinterpreted as mmap<U> (i.e., `reinterpret<U>()`); got "
             "sizeof(T) = "
          << sizeof(T) << ", sizeof(U) = " << sizeof(U);
    }
    CHECK_EQ(reinterpret_cast<std::size_t>(get()) % alignof(U), 0)
        << "data of mmap<T> must be " << alignof(U)
        << "-byte aligned when reinterpreted as mmap<U> (i.e., "
           "`reinterpret<U>()`) because alignof(U) = "
        << alignof(U);
    return mmap<U>(reinterpret_cast<U*>(get()), size() * sizeof(T) / sizeof(U));
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

/// Asynchronous random-access memory view backed by AXI-like channels.
template <typename T>
class async_mmap : public mmap<T> {
 public:
  using addr_t = int64_t;
  using resp_t = uint8_t;

 private:
  using super = mmap<T>;

  stream<addr_t, 64> read_addr_q_{"read_addr"};
  stream<T, 64> read_data_q_{"read_data"};
  stream<addr_t, 64> write_addr_q_{"write_addr"};
  stream<T, 64> write_data_q_{"write_data"};
  stream<resp_t, 64> write_resp_q_{"write_resp"};

  // Only constructible via schedule().
  async_mmap(const super& mem)
      : super(mem),
        read_addr(read_addr_q_),
        read_data(read_data_q_),
        write_addr(write_addr_q_),
        write_data(write_data_q_),
        write_resp(write_resp_q_) {}

  // Direct pointer operations are not permitted; use channel APIs instead.
  operator T*() { return super::ptr_; }
  T& operator[](std::size_t idx) { return super::ptr_[idx]; }
  const T& operator[](std::size_t idx) const { return super::ptr_[idx]; }
  T& operator*() { return *super::ptr_; }
  const T& operator*() const { return *super::ptr_; }
  T& operator++() { return *++super::ptr_; }
  T& operator--() { return *--super::ptr_; }
  T operator++(int) { return *super::ptr_++; }
  T operator--(int) { return *super::ptr_--; }
  async_mmap<T> operator+(std::ptrdiff_t diff) { return super::ptr_ + diff; }
  async_mmap<T> operator-(std::ptrdiff_t diff) { return super::ptr_ - diff; }
  std::ptrdiff_t operator-(async_mmap<T> ptr) { return super::ptr_ - ptr; }

 public:
  /// Read address channel: write an address to trigger an async read.
  ostream<addr_t> read_addr;
  /// Read data channel: read the data returned by a prior read request.
  istream<T> read_data;
  /// Write address channel: write an address to trigger an async write.
  ostream<addr_t> write_addr;
  /// Write data channel: write data to supply to the pending write request.
  ostream<T> write_data;
  /// Write response channel: read to consume a write-completion
  /// acknowledgement.
  istream<resp_t> write_resp;

  void operator()() {
    int16_t write_count = 0;
    for (;;) {
      if (!read_addr_q_.empty() && !read_data_q_.full()) {
        const auto addr = read_addr_q_.read();
        CHECK_GE(addr, 0);
        if (addr != 0) {
          CHECK_LT(addr, this->size_);
        }
        read_data_q_.write(this->ptr_[addr]);
      }
      if (write_count != 256 && !write_addr_q_.empty() &&
          !write_data_q_.empty()) {
        const auto addr = write_addr_q_.read();
        CHECK_GE(addr, 0);
        if (addr != 0) {
          CHECK_LT(addr, this->size_);
        }
        this->ptr_[addr] = write_data_q_.read();
        ++write_count;
      } else if (write_count > 0 &&
                 this->write_resp_q_.try_write(resp_t(write_count - 1))) {
        CHECK_LE(write_count, 256);
        write_count = 0;
      }
    }
  }

  static async_mmap schedule(super mem) {
    using i_addr_t = istream<addr_t>&;
    using i_data_t = istream<T>&;
    using o_addr_t = ostream<addr_t>&;
    using o_data_t = ostream<T>&;
    using i_resp_t = istream<resp_t>&;
    using o_resp_t = ostream<resp_t>&;
    using s_addr_t = stream<addr_t, 64>&;
    using s_data_t = stream<T, 64>&;
    using s_resp_t = stream<resp_t, 64>&;
    using internal::accessor;

    async_mmap async_mem(mem);
    accessor<i_addr_t, s_addr_t>::access(async_mem.read_addr_q_, false);
    accessor<o_data_t, s_data_t>::access(async_mem.read_data_q_, false);
    accessor<i_addr_t, s_addr_t>::access(async_mem.write_addr_q_, false);
    accessor<i_data_t, s_data_t>::access(async_mem.write_data_q_, false);
    accessor<o_resp_t, s_resp_t>::access(async_mem.write_resp_q_, false);
    internal::schedule(/*detach=*/true, async_mem);
    accessor<o_addr_t, s_addr_t>::access(async_mem.read_addr_q_, false);
    accessor<i_data_t, s_data_t>::access(async_mem.read_data_q_, false);
    accessor<o_addr_t, s_addr_t>::access(async_mem.write_addr_q_, false);
    accessor<o_data_t, s_data_t>::access(async_mem.write_data_q_, false);
    accessor<i_resp_t, s_resp_t>::access(async_mem.write_resp_q_, false);
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

  mmap<T>& operator[](int idx) { return mmaps_[idx]; };

  template <uint64_t offset, uint64_t length>
  mmaps<T, length> slice() {
    static_assert(offset + length < S, "invalid slice");
    mmaps<T, length> result;
    for (uint64_t i = 0; i < length; ++i) {
      result.mmaps_[i] = this->mmaps_[offset + i];
    }
    return result;
  }

  /// Reinterprets each element as @c vec_t<T,N>. Each size must be a multiple
  /// of @c N.
  template <uint64_t N>
  mmaps<vec_t<T, N>, S> vectorized() const {
    std::array<vec_t<T, N>*, S> ptrs;
    std::array<uint64_t, S> sizes;
    for (uint64_t i = 0; i < S; ++i) {
      CHECK_EQ(mmaps_[i].size() % N, 0)
          << "size of mmap<T> must be a multiple of N when vectorized as a "
             "mmap<vec_t<T, N>> (i.e., `vectorized<N>()`); got size = "
          << mmaps_[i].size() << ", N = " << N << ", but " << mmaps_[i].size()
          << " % " << N << " != 0";
      ptrs[i] = reinterpret_cast<vec_t<T, N>*>(mmaps_[i].get());
      sizes[i] = mmaps_[i].size() / N;
    }
    return mmaps<vec_t<T, N>, S>(ptrs, sizes);
  }

  /// Reinterprets each element type as @c U.
  template <typename U>
  mmaps<U, S> reinterpret() const {
    static_assert(std::is_standard_layout<T>::value,
                  "T must have standard layout");
    static_assert(std::is_standard_layout<U>::value,
                  "U must have standard layout");

    std::array<U*, S> ptrs;
    std::array<uint64_t, S> sizes;
    for (uint64_t i = 0; i < S; ++i) {
      if (sizeof(U) > sizeof(T)) {
        CHECK_EQ(sizeof(U) % sizeof(T), 0)
            << "sizeof(U) must be a multiple of sizeof(T) when mmap<T> is "
               "reinterpreted as mmap<U> (i.e., `reinterpret<U>()`); got "
               "sizeof(U) = "
            << sizeof(U) << ", sizeof(T) = " << sizeof(T);
        constexpr auto N = sizeof(U) / sizeof(T);
        CHECK_EQ(mmaps_[i].size() % N, 0)
            << "size of mmap<T> must be a multiple of N (= "
               "sizeof(U)/sizeof(T)) when reinterpreted as mmap<U> (i.e., "
               "`reinterpret<U>()`); got size = "
            << mmaps_[i].size() << ", N = " << sizeof(U) << " / " << sizeof(T)
            << " = " << N << ", but " << mmaps_[i].size() << " % " << N
            << " != 0";
      } else if (sizeof(U) < sizeof(T)) {
        CHECK_EQ(sizeof(T) % sizeof(U), 0)
            << "sizeof(T) must be a multiple of sizeof(U) when mmap<T> is "
               "reinterpreted as mmap<U> (i.e., `reinterpret<U>()`); got "
               "sizeof(T) = "
            << sizeof(T) << ", sizeof(U) = " << sizeof(U);
      }
      CHECK_EQ(reinterpret_cast<std::size_t>(mmaps_[i].get()) % alignof(U), 0)
          << "data of mmap<T> must be " << alignof(U)
          << "-byte aligned when reinterpreted as mmap<U> (i.e., "
             "`reinterpret<U>()`) because alignof(U) = "
          << alignof(U);
      ptrs[i] = reinterpret_cast<U*>(mmaps_[i].get());
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
#define TAPA_DEFINE_TAGGED_MMAP(tag, kind)               \
  template <typename T>                                  \
  class tag##_##kind : public kind<T> {                  \
    tag##_##kind(T* ptr) : kind<T>(ptr) {}               \
                                                         \
   public:                                               \
    using kind<T>::kind;                                 \
    tag##_##kind(const kind<T>& base) : kind<T>(base) {} \
                                                         \
    template <uint64_t N>                                \
    tag##_##kind<vec_t<T, N>> vectorized() const {       \
      return kind<T>::template vectorized<N>();          \
    }                                                    \
    template <typename U>                                \
    tag##_##kind<U> reinterpret() const {                \
      return kind<T>::template reinterpret<U>();         \
    }                                                    \
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
#define TAPA_DEFINE_MMAPS(tag)                                        \
  template <typename T, uint64_t S>                                   \
  class tag##_mmaps : public mmaps<T, S> {                            \
    tag##_mmaps(const std::array<T*, S>& ptrs) : mmaps<T, S>(ptrs){}; \
                                                                      \
   public:                                                            \
    using mmaps<T, S>::mmaps;                                         \
    tag##_mmaps(const mmaps<T, S>& base) : mmaps<T, S>(base) {}       \
                                                                      \
    template <uint64_t N>                                             \
    tag##_mmaps<vec_t<T, N>, S> vectorized() const {                  \
      return mmaps<T, S>::template vectorized<N>();                   \
    }                                                                 \
    template <typename U>                                             \
    tag##_mmaps<U, S> reinterpret() const {                           \
      return mmaps<T, S>::template reinterpret<U>();                  \
    }                                                                 \
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
  static_assert(!std::is_same<T, T>::value,
                "must use one of "
                "placeholder_mmap/read_only_mmap/write_only_mmap/"
                "read_write_mmap in tapa::invoke");
};
template <typename T, int64_t S>
struct accessor<mmaps<T, S>, mmaps<T, S>> {
  static_assert(!std::is_same<T, T>::value,
                "must use one of "
                "placeholder_mmaps/read_only_mmaps/write_only_mmaps/"
                "read_write_mmaps in tapa::invoke");
};

}  // namespace internal

}  // namespace tapa
