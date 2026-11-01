#pragma once

#include <aie_api/aie.hpp>

#include <algorithm>
#include <array>

#include "tapa/base/util.h"

// Every AI Engine helper below is forced inline: the chess compiler keeps
// an out-of-line call per element otherwise, which costs tens of cycles
// where the vector unit needs one.
#define TAPA_AIE_INLINE inline __attribute__((always_inline))

// `tapa::vec_t` on the AI Engine: native vector shapes use one `aie::vector`
// register and single vector instructions. Other whole-word shapes use an
// array with the same API and scalar operations. `acc += a * b` lowers to one
// `aie::mac` on native shapes: `operator*` returns a lazy product that
// `+=`/`-=` consume, and that otherwise converts to a vector. So does
// `acc += x[k] * b` with `x` a non-const native vector: the multiply
// broadcasts lane `k` of `x` in place, without extracting it.

namespace tapa {

namespace internal {

template <typename T, int N,
          bool Native =
              requires { typename ::aie::detail::native_vector_type_t<T, N>; }>
struct vec_product;

template <typename T, int N>
struct vec_product<T, N, true> {
  ::aie::vector<T, N> lhs;
  ::aie::vector<T, N> rhs;

  TAPA_AIE_INLINE ::aie::vector<T, N> eval() const {
    return ::aie::mul(lhs, rhs).template to_vector<T>();
  }
  TAPA_AIE_INLINE ::aie::vector<T, N> mac(const ::aie::vector<T, N>& acc,
                                          bool sub) const {
    decltype(::aie::mul(lhs, rhs)) a;
    a.from_vector(acc);
    a = sub ? ::aie::msc(a, lhs, rhs) : ::aie::mac(a, lhs, rhs);
    return a.template to_vector<T>();
  }
};

// `x[k] * v`, with `x[k]` a lane of an `M`-lane native vector.
template <typename T, int N, unsigned M>
struct vec_lane_product {
  ::aie::vector_elem_ref<T, M> lane;
  ::aie::vector<T, N> vec;

  TAPA_AIE_INLINE ::aie::vector<T, N> eval() const {
    return ::aie::mul(lane, vec).template to_vector<T>();
  }
  TAPA_AIE_INLINE ::aie::vector<T, N> mac(const ::aie::vector<T, N>& acc,
                                          bool sub) const {
    decltype(::aie::mul(lane, vec)) a;
    a.from_vector(acc);
    a = sub ? ::aie::msc(a, lane, vec) : ::aie::mac(a, lane, vec);
    return a.template to_vector<T>();
  }
};

template <typename T, int N>
struct vec_product<T, N, false> {
  std::array<T, N> lhs;
  std::array<T, N> rhs;

  TAPA_AIE_INLINE std::array<T, N> eval() const {
    std::array<T, N> out;
    for (int i = 0; i < N; ++i) out[i] = lhs[i] * rhs[i];
    return out;
  }
  TAPA_AIE_INLINE std::array<T, N> mac(const std::array<T, N>& acc,
                                       bool sub) const {
    std::array<T, N> out;
    for (int i = 0; i < N; ++i) {
      out[i] = sub ? acc[i] - lhs[i] * rhs[i] : acc[i] + lhs[i] * rhs[i];
    }
    return out;
  }
};

}  // namespace internal

template <typename T, int N,
          bool Native =
              requires { typename ::aie::detail::native_vector_type_t<T, N>; }>
struct vec_t;

template <typename T, int N>
struct vec_t<T, N, true> {
  static constexpr int length = N;
  static constexpr int width = widthof<T>() * N;
  using value_type = T;
  using vector_type = ::aie::vector<T, N>;

  vector_type v;

  vec_t() = default;
  TAPA_AIE_INLINE vec_t(const vector_type& x) : v(x) {}
  TAPA_AIE_INLINE vec_t(const internal::vec_product<T, N>& p) : v(p.eval()) {}
  template <unsigned M>
  TAPA_AIE_INLINE vec_t(const internal::vec_lane_product<T, N, M>& p)
      : v(p.eval()) {}
  TAPA_AIE_INLINE operator vector_type() const { return v; }

  TAPA_AIE_INLINE T operator[](int pos) const { return v.get(pos); }
  TAPA_AIE_INLINE auto operator[](int pos) { return v[pos]; }
  TAPA_AIE_INLINE T get(int pos) const { return v.get(pos); }
  TAPA_AIE_INLINE void set(int pos, T value) { v.set(value, pos); }
  TAPA_AIE_INLINE void set(T value) { v = ::aie::broadcast<T, N>(value); }
  TAPA_AIE_INLINE vec_t& operator=(T value) {
    set(value);
    return *this;
  }

  friend TAPA_AIE_INLINE vec_t operator+(const vec_t& a, const vec_t& b) {
    return ::aie::add(a.v, b.v);
  }
  friend TAPA_AIE_INLINE vec_t operator-(const vec_t& a, const vec_t& b) {
    return ::aie::sub(a.v, b.v);
  }
  friend TAPA_AIE_INLINE vec_t operator+(const vec_t& a, T b) {
    return ::aie::add(a.v, b);
  }
  friend TAPA_AIE_INLINE vec_t operator-(const vec_t& a, T b) {
    return ::aie::sub(a.v, b);
  }
  friend TAPA_AIE_INLINE internal::vec_product<T, N> operator*(const vec_t& a,
                                                               const vec_t& b) {
    return {a.v, b.v};
  }
  friend TAPA_AIE_INLINE internal::vec_product<T, N> operator*(const vec_t& a,
                                                               T b) {
    return {a.v, ::aie::broadcast<T, N>(b)};
  }
  friend TAPA_AIE_INLINE internal::vec_product<T, N> operator*(T a,
                                                               const vec_t& b) {
    return {::aie::broadcast<T, N>(a), b.v};
  }
  template <unsigned M>
  friend TAPA_AIE_INLINE internal::vec_lane_product<T, N, M> operator*(
      ::aie::vector_elem_ref<T, M> a, const vec_t& b) {
    return {a, b.v};
  }
  template <unsigned M>
  friend TAPA_AIE_INLINE internal::vec_lane_product<T, N, M> operator*(
      const vec_t& a, ::aie::vector_elem_ref<T, M> b) {
    return {b, a.v};
  }
  TAPA_AIE_INLINE vec_t operator-() const { return ::aie::neg(v); }

  TAPA_AIE_INLINE vec_t& operator+=(const vec_t& o) {
    return *this = *this + o;
  }
  TAPA_AIE_INLINE vec_t& operator-=(const vec_t& o) {
    return *this = *this - o;
  }
  TAPA_AIE_INLINE vec_t& operator*=(const vec_t& o) {
    return *this = *this * o;
  }
  TAPA_AIE_INLINE vec_t& operator+=(T o) { return *this = *this + o; }
  TAPA_AIE_INLINE vec_t& operator-=(T o) { return *this = *this - o; }
  TAPA_AIE_INLINE vec_t& operator*=(T o) { return *this = *this * o; }
  TAPA_AIE_INLINE vec_t& operator+=(const internal::vec_product<T, N>& p) {
    v = p.mac(v, /*sub=*/false);
    return *this;
  }
  TAPA_AIE_INLINE vec_t& operator-=(const internal::vec_product<T, N>& p) {
    v = p.mac(v, /*sub=*/true);
    return *this;
  }
  template <unsigned M>
  TAPA_AIE_INLINE vec_t& operator+=(
      const internal::vec_lane_product<T, N, M>& p) {
    v = p.mac(v, /*sub=*/false);
    return *this;
  }
  template <unsigned M>
  TAPA_AIE_INLINE vec_t& operator-=(
      const internal::vec_lane_product<T, N, M>& p) {
    v = p.mac(v, /*sub=*/true);
    return *this;
  }
};

template <typename T, int N>
struct vec_t<T, N, false> {
  static constexpr int length = N;
  static constexpr int width = widthof<T>() * N;
  using value_type = T;
  using vector_type = std::array<T, N>;

  vector_type v;

  vec_t() = default;
  TAPA_AIE_INLINE vec_t(const vector_type& x) : v(x) {}
  TAPA_AIE_INLINE vec_t(const internal::vec_product<T, N>& p) : v(p.eval()) {}
  TAPA_AIE_INLINE operator vector_type() const { return v; }

  TAPA_AIE_INLINE T operator[](int pos) const { return v[pos]; }
  TAPA_AIE_INLINE auto operator[](int pos) { return v[pos]; }
  TAPA_AIE_INLINE T get(int pos) const { return v[pos]; }
  TAPA_AIE_INLINE void set(int pos, T value) { v[pos] = value; }
  TAPA_AIE_INLINE void set(T value) { v.fill(value); }
  TAPA_AIE_INLINE vec_t& operator=(T value) {
    set(value);
    return *this;
  }

  friend TAPA_AIE_INLINE vec_t operator+(const vec_t& a, const vec_t& b) {
    vec_t out;
    for (int i = 0; i < N; ++i) out.v[i] = a.v[i] + b.v[i];
    return out;
  }
  friend TAPA_AIE_INLINE vec_t operator-(const vec_t& a, const vec_t& b) {
    vec_t out;
    for (int i = 0; i < N; ++i) out.v[i] = a.v[i] - b.v[i];
    return out;
  }
  friend TAPA_AIE_INLINE vec_t operator+(const vec_t& a, T b) {
    vec_t out;
    for (int i = 0; i < N; ++i) out.v[i] = a.v[i] + b;
    return out;
  }
  friend TAPA_AIE_INLINE vec_t operator-(const vec_t& a, T b) {
    vec_t out;
    for (int i = 0; i < N; ++i) out.v[i] = a.v[i] - b;
    return out;
  }
  friend TAPA_AIE_INLINE internal::vec_product<T, N> operator*(const vec_t& a,
                                                               const vec_t& b) {
    return {a.v, b.v};
  }
  friend TAPA_AIE_INLINE internal::vec_product<T, N> operator*(const vec_t& a,
                                                               T b) {
    vec_t broadcast;
    broadcast.set(b);
    return {a.v, broadcast.v};
  }
  friend TAPA_AIE_INLINE internal::vec_product<T, N> operator*(T a,
                                                               const vec_t& b) {
    vec_t broadcast;
    broadcast.set(a);
    return {broadcast.v, b.v};
  }
  TAPA_AIE_INLINE vec_t operator-() const {
    vec_t out;
    for (int i = 0; i < N; ++i) out.v[i] = -v[i];
    return out;
  }

  TAPA_AIE_INLINE vec_t& operator+=(const vec_t& o) {
    return *this = *this + o;
  }
  TAPA_AIE_INLINE vec_t& operator-=(const vec_t& o) {
    return *this = *this - o;
  }
  TAPA_AIE_INLINE vec_t& operator*=(const vec_t& o) {
    return *this = *this * o;
  }
  TAPA_AIE_INLINE vec_t& operator+=(T o) { return *this = *this + o; }
  TAPA_AIE_INLINE vec_t& operator-=(T o) { return *this = *this - o; }
  TAPA_AIE_INLINE vec_t& operator*=(T o) { return *this = *this * o; }
  TAPA_AIE_INLINE vec_t& operator+=(const internal::vec_product<T, N>& p) {
    v = p.mac(v, /*sub=*/false);
    return *this;
  }
  TAPA_AIE_INLINE vec_t& operator-=(const internal::vec_product<T, N>& p) {
    v = p.mac(v, /*sub=*/true);
    return *this;
  }
};

template <typename To, typename From>
TAPA_AIE_INLINE
    typename std::enable_if<sizeof(To) == sizeof(From), To>::type  //
    bit_cast(From from) noexcept {
  return __builtin_bit_cast(To, from);
}

template <int N, typename T>
TAPA_AIE_INLINE vec_t<T, N> make_vec(T value) {
  if constexpr (requires {
                  typename ::aie::detail::native_vector_type_t<T, N>;
                }) {
    return ::aie::broadcast<T, N>(value);
  } else {
    vec_t<T, N> result;
    result.set(value);
    return result;
  }
}

template <typename T, int N>
TAPA_AIE_INLINE T sum(const vec_t<T, N>& vec) {
  if constexpr (requires {
                  typename ::aie::detail::native_vector_type_t<T, N>;
                }) {
    return ::aie::reduce_add(vec.v);
  } else {
    T out{};
    for (int i = 0; i < N; ++i) out += vec.v[i];
    return out;
  }
}

template <typename T, int N>
TAPA_AIE_INLINE vec_t<T, N> max(const vec_t<T, N>& a, const vec_t<T, N>& b) {
  if constexpr (requires {
                  typename ::aie::detail::native_vector_type_t<T, N>;
                }) {
    return ::aie::max(a.v, b.v);
  } else {
    vec_t<T, N> out;
    for (int i = 0; i < N; ++i) out.v[i] = std::max(a.v[i], b.v[i]);
    return out;
  }
}

template <typename T, int N>
TAPA_AIE_INLINE vec_t<T, N> min(const vec_t<T, N>& a, const vec_t<T, N>& b) {
  if constexpr (requires {
                  typename ::aie::detail::native_vector_type_t<T, N>;
                }) {
    return ::aie::min(a.v, b.v);
  } else {
    vec_t<T, N> out;
    for (int i = 0; i < N; ++i) out.v[i] = std::min(a.v[i], b.v[i]);
    return out;
  }
}

}  // namespace tapa
