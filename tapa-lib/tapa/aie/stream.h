#pragma once

#include <array>
#include <cstdint>
#include <cstring>
#include <type_traits>
#include <utility>

#include <adf.h>
#include <aie_api/aie.hpp>
#include <aie_api/aie_adf.hpp>

#include "tapa/aie/frame.h"
#include "tapa/aie/vec.h"

// TAPA stream ports on the AI Engine. Every ADF port TAPA generates carries
// 32-bit words (`input_stream<int32>`); an element is read and written as its
// own type through a pointer cast, the way aie_api's helpers do internally.
// Only the blocking operations exist: AI Engine streams have no EoT and no
// visible occupancy, so `close`, `eot`, `peek`, `try_*`, `empty` and `full`
// fail to compile here (tapacc rejects them first, with a portable hint).

namespace tapa {
namespace internal {
namespace aie {

// 32-bit words one element occupies on an ADF port or in a frame buffer.
template <typename T>
inline constexpr int kWords = (::tapa::widthof<T>() + 31) / 32;

// Word-wise fallback for any trivially copyable whole-word element. Native
// vector specializations below retain their single-instruction fast path.
// Chess lowers std::memcpy to a byte-wise libc call; plain word loops are
// faster and do not grow program memory through forced unrolling. Bit casts
// preserve the element's object representation without type-punning pointers.
template <typename T>
struct word_io {
  using words_t = std::array<int32, kWords<T>>;

  static TAPA_AIE_INLINE T read(input_stream<int32>* s) {
    words_t words;
    for (auto& word : words) word = readincr(s);
    return __builtin_bit_cast(T, words);
  }
  static TAPA_AIE_INLINE void write(output_stream<int32>* s, const T& value) {
    auto words = __builtin_bit_cast(words_t, value);
    for (const auto word : words) writeincr(s, word);
  }
  static TAPA_AIE_INLINE T load(const int32* p) {
    words_t words;
    for (auto& word : words) word = *p++;
    return __builtin_bit_cast(T, words);
  }
  static TAPA_AIE_INLINE void store(int32* p, const T& value) {
    auto words = __builtin_bit_cast(words_t, value);
    for (const auto word : words) *p++ = word;
  }
};

template <typename T>
struct elem_io {
  static_assert(sizeof(T) % sizeof(int32) == 0 &&
                    std::is_trivially_copyable<T>::value,
                "an AI Engine stream element must be trivially copyable and "
                "a whole number of 32-bit words");
  static TAPA_AIE_INLINE T read(input_stream<int32>* s) {
    return word_io<T>::read(s);
  }
  static TAPA_AIE_INLINE void write(output_stream<int32>* s, const T& value) {
    word_io<T>::write(s, value);
  }
  static TAPA_AIE_INLINE T load(const int32* p) { return word_io<T>::load(p); }
  static TAPA_AIE_INLINE void store(int32* p, const T& value) {
    word_io<T>::store(p, value);
  }
};

template <typename E, int N>
struct elem_io<vec_t<E, N>> {
  static TAPA_AIE_INLINE vec_t<E, N> read(input_stream<int32>* s) {
    if constexpr (requires {
                    typename ::aie::detail::native_vector_type_t<E, N>;
                  }) {
      return readincr_v<N>(reinterpret_cast<input_stream<E>*>(s));
    } else {
      return word_io<vec_t<E, N>>::read(s);
    }
  }
  static TAPA_AIE_INLINE void write(output_stream<int32>* s,
                                    const vec_t<E, N>& value) {
    if constexpr (requires {
                    typename ::aie::detail::native_vector_type_t<E, N>;
                  }) {
      writeincr(reinterpret_cast<output_stream<E>*>(s), value.v);
    } else {
      word_io<vec_t<E, N>>::write(s, value);
    }
  }
  static TAPA_AIE_INLINE vec_t<E, N> load(const int32* p) {
    if constexpr (requires {
                    typename ::aie::detail::native_vector_type_t<E, N>;
                  }) {
      return ::aie::load_v<N>(reinterpret_cast<const E*>(p));
    } else {
      return word_io<vec_t<E, N>>::load(p);
    }
  }
  static TAPA_AIE_INLINE void store(int32* p, const vec_t<E, N>& value) {
    if constexpr (requires {
                    typename ::aie::detail::native_vector_type_t<E, N>;
                  }) {
      ::aie::store_v(reinterpret_cast<E*>(p), value.v);
    } else {
      word_io<vec_t<E, N>>::store(p, value);
    }
  }
};

// Keeps every later operation behind a stream write in a kernel a stream
// cycle runs through (the graph defines `TAPA_AIE_ORDERED_STREAMS` for it).
// Defining `TAPA_AIE_PHASE_ORDERED_STREAMS` suppresses these per-write fences;
// the kernel must then place its own fence between each write and read phase.
// The AI Engine compiler otherwise issues a read of another port ahead of an
// earlier write, which such a cycle turns into a deadlock where software
// simulation, running in program order, makes progress. Elsewhere the
// overlap is safe and worth keeping: the fence makes each element of a
// pipelined read-compute-write loop take about 60% longer.
TAPA_AIE_INLINE void after_write() {
#if defined(__AIENGINE__) && defined(TAPA_AIE_ORDERED_STREAMS) && \
    !defined(TAPA_AIE_PHASE_ORDERED_STREAMS)
  chess_separator_scheduler();
#endif
}

}  // namespace aie
}  // namespace internal

template <typename T>
class istream {
 public:
  TAPA_AIE_INLINE explicit istream(input_stream<int32>* port) : port_(port) {}
  TAPA_AIE_INLINE T read() { return internal::aie::elem_io<T>::read(port_); }
  TAPA_AIE_INLINE istream& operator>>(T& value) {
    value = read();
    return *this;
  }

 private:
  input_stream<int32>* port_;
};

template <typename T>
class ostream {
 public:
  TAPA_AIE_INLINE explicit ostream(output_stream<int32>* port) : port_(port) {}
  TAPA_AIE_INLINE void write(const T& value) {
    internal::aie::elem_io<T>::write(port_, value);
    internal::aie::after_write();
  }
  TAPA_AIE_INLINE ostream& operator<<(const T& value) {
    write(value);
    return *this;
  }

 private:
  output_stream<int32>* port_;
};

template <typename T, uint64_t S>
class istreams {
 public:
  template <typename... Ports>
  explicit istreams(Ports... ports) : ports_{istream<T>(ports)...} {
    static_assert(sizeof...(Ports) == S, "one ADF port per channel");
  }
  TAPA_AIE_INLINE istream<T>& operator[](int pos) { return ports_[pos]; }

 private:
  istream<T> ports_[S];
};

template <typename T, uint64_t S>
class ostreams {
 public:
  template <typename... Ports>
  explicit ostreams(Ports... ports) : ports_{ostream<T>(ports)...} {
    static_assert(sizeof...(Ports) == S, "one ADF port per channel");
  }
  TAPA_AIE_INLINE ostream<T>& operator[](int pos) { return ports_[pos]; }

 private:
  ostream<T> ports_[S];
};

namespace aie {

// A `[[tapa::aie_frame(N)]]` stream port: the stream arrives as frames of
// `N` elements in tile memory (async ping-pong buffers), and `read()` walks
// the current frame, acquiring the next buffer every `N` reads. tapacc
// rewrites the port's type to `frame_t<...>` in the AI Engine variant only.
template <typename T, int N>
class iframe {
 public:
  using buffer = input_frame<int32, N * internal::aie::kWords<T>>;
  TAPA_AIE_INLINE explicit iframe(buffer& buf) : buf_(buf) {}
  TAPA_AIE_INLINE T read() {
    if (pos_ == 0) {
      buf_.acquire();
      ptr_ = buf_.data();
    }
    const T value = internal::aie::elem_io<T>::load(ptr_);
    ptr_ += internal::aie::kWords<T>;
    if (++pos_ == N) {
      buf_.release();
      pos_ = 0;
    }
    return value;
  }
  TAPA_AIE_INLINE iframe& operator>>(T& value) {
    value = read();
    return *this;
  }
  // A final frame may be padded; unread padding is discarded on release.
  ~iframe() {
    if (pos_ != 0) buf_.release();
  }

 private:
  buffer& buf_;
  const int32* ptr_ = nullptr;
  int pos_ = 0;
};

template <typename T, int N>
class oframe {
 public:
  using buffer = output_frame<int32, N * internal::aie::kWords<T>>;
  TAPA_AIE_INLINE explicit oframe(buffer& buf) : buf_(buf) {}
  TAPA_AIE_INLINE void write(const T& value) {
    if (pos_ == 0) {
      buf_.acquire();
      ptr_ = buf_.data();
    }
    internal::aie::elem_io<T>::store(ptr_, value);
    ptr_ += internal::aie::kWords<T>;
    if (++pos_ == N) {
      buf_.release();
      pos_ = 0;
    }
  }
  TAPA_AIE_INLINE oframe& operator<<(const T& value) {
    write(value);
    return *this;
  }
  // A final partial frame is zero-padded and released when the task returns.
  ~oframe() {
    if (pos_ != 0) {
      std::memset(buf_.data() + pos_ * internal::aie::kWords<T>, 0,
                  (N - pos_) * internal::aie::kWords<T> * sizeof(int32));
      buf_.release();
    }
  }

 private:
  buffer& buf_;
  int32* ptr_ = nullptr;
  int pos_ = 0;
};

template <typename Port, int N>
struct frame_of;
template <typename T, int N>
struct frame_of<tapa::istream<T>, N> {
  using type = iframe<T, N>;
};
template <typename T, int N>
struct frame_of<tapa::ostream<T>, N> {
  using type = oframe<T, N>;
};
template <typename Port, int N>
using frame_t = typename frame_of<Port, N>::type;

// The channels of an `istreams`/`ostreams` port, as the generated kernel
// wrapper passes them.
template <typename... Ports>
struct pack {
  explicit pack(Ports... p) : ports{p...} {}
  template <typename Array, size_t... I>
  Array make(std::index_sequence<I...>) const {
    return Array(ports[I]...);
  }
  typename std::common_type<Ports...>::type ports[sizeof...(Ports)];
};

// A compile-time scalar argument (an invoke-site constant), as its raw bits.
struct bits {
  uint64_t value;
};

// How one task parameter is built from what the kernel wrapper received.
template <typename Param>
struct arg {
  Param value;
  explicit arg(Param v) : value(v) {}
  explicit arg(bits b) {
    static_assert(sizeof(Param) <= sizeof(b.value), "scalar wider than 64");
    std::memcpy(&value, &b.value, sizeof(Param));
  }
  TAPA_AIE_INLINE Param get() { return value; }
};
template <typename T>
struct arg<tapa::istream<T>&> {
  tapa::istream<T> value;
  explicit arg(input_stream<int32>* port) : value(port) {}
  TAPA_AIE_INLINE tapa::istream<T>& get() { return value; }
};
template <typename T>
struct arg<tapa::ostream<T>&> {
  tapa::ostream<T> value;
  explicit arg(output_stream<int32>* port) : value(port) {}
  TAPA_AIE_INLINE tapa::ostream<T>& get() { return value; }
};
template <typename T, uint64_t S>
struct arg<tapa::istreams<T, S>&> {
  tapa::istreams<T, S> value;
  template <typename... Ports>
  explicit arg(const pack<Ports...>& p)
      : value(p.template make<tapa::istreams<T, S>>(
            std::index_sequence_for<Ports...>{})) {}
  TAPA_AIE_INLINE tapa::istreams<T, S>& get() { return value; }
};
template <typename T, uint64_t S>
struct arg<tapa::ostreams<T, S>&> {
  tapa::ostreams<T, S> value;
  template <typename... Ports>
  explicit arg(const pack<Ports...>& p)
      : value(p.template make<tapa::ostreams<T, S>>(
            std::index_sequence_for<Ports...>{})) {}
  TAPA_AIE_INLINE tapa::ostreams<T, S>& get() { return value; }
};
template <typename T, int N>
struct arg<iframe<T, N>&> {
  iframe<T, N> value;
  explicit arg(typename iframe<T, N>::buffer& buf) : value(buf) {}
  TAPA_AIE_INLINE iframe<T, N>& get() { return value; }
};
template <typename T, int N>
struct arg<oframe<T, N>&> {
  oframe<T, N> value;
  explicit arg(typename oframe<T, N>::buffer& buf) : value(buf) {}
  TAPA_AIE_INLINE oframe<T, N>& get() { return value; }
};

// Calls a task with its ports built from the kernel wrapper's ADF ports,
// in parameter order. The holders are temporaries of the full expression,
// so they outlive the call.
template <typename... Params, typename... Raw>
TAPA_AIE_INLINE void invoke(void (*task)(Params...), Raw&&... raw) {
  static_assert(sizeof...(Params) == sizeof...(Raw),
                "one kernel argument per task parameter");
  task(arg<Params>(static_cast<Raw&&>(raw)).get()...);
}

}  // namespace aie
}  // namespace tapa
