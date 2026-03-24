// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef TAPA_HOST_VEC_H_
#define TAPA_HOST_VEC_H_

#include <climits>
#include <cmath>
#include <cstdint>
#include <cstring>

#include <algorithm>
#include <array>
#include <functional>
#include <ostream>

#include "tapa/host/util.h"

namespace tapa {

template <typename T, int N>
struct vec_t : protected std::array<T, N> {
 private:
  using base_type = std::array<T, N>;

 public:
  static constexpr int length = N;
  static constexpr int width = widthof<T>() * N;

  using size_type = int;
  using typename base_type::const_iterator;
  using typename base_type::const_pointer;
  using typename base_type::const_reference;
  using typename base_type::const_reverse_iterator;
  using typename base_type::difference_type;
  using typename base_type::iterator;
  using typename base_type::pointer;
  using typename base_type::reference;
  using typename base_type::reverse_iterator;
  using typename base_type::value_type;

  constexpr const_reference operator[](size_type pos) const {
    return base_type::operator[](pos);
  }
  reference operator[](size_type pos) { return base_type::operator[](pos); }
  constexpr const_reference get(size_type pos) const { return (*this)[pos]; }
  void set(size_type pos, const T& value) { (*this)[pos] = value; }

  using base_type::base_type;
  using base_type::operator=;

  explicit vec_t(const base_type& other) : base_type(other) {}
  explicit vec_t(base_type&& other) : base_type(other) {}

  template <typename U>
  explicit operator vec_t<U, N>() const {
    vec_t<U, N> result;
    for (size_type i = 0; i < N; ++i) {
      result.set(i, static_cast<U>(get(i)));
    }
    return result;
  }

  void set(T val) { *this = val; }

  vec_t& operator=(T val) {
    for (size_type i = 0; i < N; ++i) {
      set(i, val);
    }
    return *this;
  }

#define DEFINE_OP(op)                                    \
  template <typename T2>                                 \
  vec_t<T, N>& operator op##=(const vec_t<T2, N>& rhs) { \
    for (size_type i = 0; i < N; ++i) {                  \
      set(i, get(i) op rhs[i]);                          \
    }                                                    \
    return *this;                                        \
  }                                                      \
  template <typename T2>                                 \
  vec_t<T, N>& operator op##=(const T2 & rhs) {          \
    for (size_type i = 0; i < N; ++i) {                  \
      set(i, get(i) op rhs);                             \
    }                                                    \
    return *this;                                        \
  }
  DEFINE_OP(+)
  DEFINE_OP(-)
  DEFINE_OP(*)
  DEFINE_OP(/)
  DEFINE_OP(%)
  DEFINE_OP(&)
  DEFINE_OP(|)
  DEFINE_OP(^)
  DEFINE_OP(<<)
  DEFINE_OP(>>)
#undef DEFINE_OP

#define DEFINE_OP(op)                   \
  vec_t<T, N> operator op() {           \
    for (size_type i = 0; i < N; ++i) { \
      set(i, op get(i));                \
    }                                   \
    return *this;                       \
  }
  DEFINE_OP(+)
  DEFINE_OP(-)
  DEFINE_OP(~)
#undef DEFINE_OP

#define DEFINE_OP(op)                                \
  template <typename T2>                             \
  vec_t<T, N> operator op(const vec_t<T2, N>& rhs) { \
    vec_t<T, N> result;                              \
    for (size_type i = 0; i < N; ++i) {              \
      result.set(i, get(i) op rhs[i]);               \
    }                                                \
    return result;                                   \
  }                                                  \
  template <typename T2>                             \
  vec_t<T, N> operator op(const T2 & rhs) {          \
    vec_t<T, N> result;                              \
    for (size_type i = 0; i < N; ++i) {              \
      result.set(i, get(i) op rhs);                  \
    }                                                \
    return result;                                   \
  }
  DEFINE_OP(+)
  DEFINE_OP(-)
  DEFINE_OP(*)
  DEFINE_OP(/)
  DEFINE_OP(%)
  DEFINE_OP(&)
  DEFINE_OP(|)
  DEFINE_OP(^)
  DEFINE_OP(<<)
  DEFINE_OP(>>)
#undef DEFINE_OP

  /// Shifts all elements left by 1, discarding [0] and placing @p val at [N-1].
  void shift(const T& val) {
    for (size_type i = 1; i < N; ++i) {
      set(i - 1, get(i));
    }
    set(N - 1, val);
  }

  bool has(const T& val) {
    for (size_type i = 0; i < N; ++i) {
      if (val == get(i)) return true;
    }
    return false;
  }
};

/// Returns vec[begin:end].
template <int begin, int end, typename T, int N>
inline vec_t<T, end - begin> truncated(const vec_t<T, N>& vec) {
  static_assert(begin >= 0, "cannot truncate before 0");
  static_assert(end <= N, "cannot truncate after N");
  vec_t<T, end - begin> result;
  for (int i = 0; i < end - begin; ++i) {
    result.set(i, vec[begin + i]);
  }
  return result;
}

/// Returns vec[:length].
template <int length, typename T, int N>
inline vec_t<T, length> truncated(const vec_t<T, N>& vec) {
  return truncated<0, length>(vec);
}

/// Returns vec[begin:begin+length].
template <int length, typename T, int N>
inline vec_t<T, length> truncated(const vec_t<T, N>& vec, int begin) {
  static_assert(length <= N, "cannot enlarge vector");
  CHECK_GE(begin, 0) << "cannot truncate before 0";
  CHECK_LE(begin + length, N) << "cannot truncate after N";
  vec_t<T, length> result;
  for (int i = 0; i < length; ++i) {
    result.set(i, vec[begin + i]);
  }
  return result;
}

/// Returns vec[:] + [val].
template <typename T, int N>
inline vec_t<T, N + 1> cat(const vec_t<T, N>& vec, const T& val) {
  vec_t<T, N + 1> result;
  for (int i = 0; i < N; ++i) {
    result.set(i, vec[i]);
  }
  result.set(N, val);
  return result;
}

/// Returns [val] + vec[:].
template <typename T, int N>
inline vec_t<T, N + 1> cat(const T& val, const vec_t<T, N>& vec) {
  vec_t<T, N + 1> result;
  result.set(0, val);
  for (int i = 0; i < N; ++i) {
    result.set(i + 1, vec[i]);
  }
  return result;
}

/// Returns v1[:] + v2[:].
template <typename T, int N1, int N2>
inline vec_t<T, N1 + N2> cat(const vec_t<T, N1>& v1, const vec_t<T, N2>& v2) {
  vec_t<T, N1 + N2> result;
  for (int i = 0; i < N1; ++i) {
    result.set(i, v1[i]);
  }
  for (int i = 0; i < N2; ++i) {
    result.set(i + N1, v2[i]);
  }
  return result;
}

template <typename T, typename... Args>
inline auto cat(T arg, Args... args) {
  return cat(arg, cat(args...));
}

#define DEFINE_OP(op)                                               \
  template <typename T, int N, typename T2>                         \
  vec_t<T, N> operator op(const T2 & lhs, const vec_t<T, N>& rhs) { \
    vec_t<T, N> result;                                             \
    for (int i = 0; i < N; ++i) {                                   \
      result.set(i, lhs op rhs[i]);                                 \
    }                                                               \
    return result;                                                  \
  }
DEFINE_OP(+)
DEFINE_OP(-)
DEFINE_OP(*)
DEFINE_OP(/)
DEFINE_OP(%)
DEFINE_OP(&)
DEFINE_OP(|)
DEFINE_OP(^)
DEFINE_OP(<<)
DEFINE_OP(>>)
#undef DEFINE_OP

template <int N, typename T>
vec_t<T, N> make_vec(T val) {
  vec_t<T, N> result;
  result.set(val);
  return result;
}

#define DEFINE_FUNC(func)             \
  template <typename T, int N>        \
  vec_t<T, N> func(vec_t<T, N> vec) { \
    for (int i = 0; i < N; ++i) {     \
      vec.set(i, std::func(vec[i]));  \
    }                                 \
    return vec;                       \
  }
DEFINE_FUNC(exp)
DEFINE_FUNC(exp2)
DEFINE_FUNC(expm1)
DEFINE_FUNC(log)
DEFINE_FUNC(log10)
DEFINE_FUNC(log1p)
DEFINE_FUNC(log2)
#undef DEFINE_FUNC

#define DEFINE_FUNC(func)                                            \
  template <typename T, int N>                                       \
  vec_t<T, N> func(const vec_t<T, N>& lhs, const vec_t<T, N>& rhs) { \
    vec_t<T, N> result;                                              \
    for (int i = 0; i < N; ++i) {                                    \
      result.set(i, std::func(lhs[i], rhs[i]));                      \
    }                                                                \
    return result;                                                   \
  }                                                                  \
  template <typename T, int N>                                       \
  vec_t<T, N> func(const T& lhs, const vec_t<T, N>& rhs) {           \
    return func(make_vec<N>(lhs), rhs);                              \
  }                                                                  \
  template <typename T, int N>                                       \
  vec_t<T, N> func(const vec_t<T, N>& lhs, const T& rhs) {           \
    return func(lhs, make_vec<N>(rhs));                              \
  }
DEFINE_FUNC(max)
DEFINE_FUNC(min)
#undef DEFINE_FUNC

#define DEFINE_FUNC(func, op)                                             \
  template <typename T>                                                   \
  T func(const vec_t<T, 1>& vec) {                                        \
    return vec[0];                                                        \
  }                                                                       \
  template <typename T, int N>                                            \
  T func(const vec_t<T, N>& vec) {                                        \
    return func(truncated<N / 2>(vec)) op func(truncated<N / 2, N>(vec)); \
  }
DEFINE_FUNC(sum, +)
DEFINE_FUNC(product, *)
#undef DEFINE_FUNC

template <typename T, int N>
inline std::ostream& operator<<(std::ostream& os, const vec_t<T, N>& obj) {
  os << "{";
  for (int i = 0; i < N; ++i) {
    if (i > 0) os << ", ";
    os << "[" << i << "]: " << obj[i];
  }
  return os << "}";
}

}  // namespace tapa

#endif  // TAPA_HOST_VEC_H_
