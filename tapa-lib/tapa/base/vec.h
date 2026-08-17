#pragma once

#include <climits>
#include <cmath>
#include <cstdint>
#include <cstring>

#include <algorithm>
#include <array>
#include <functional>
#include <ostream>
#include <type_traits>

#include "tapa/base/util.h"

// One `vec_t` for every target. The three targets used to carry three copies of
// this file whose only real difference was where the HLS pragmas went, so the
// pragmas live in macros here and the bodies are written once. On any target
// but Xilinx HLS they expand to nothing, which is why no non-HLS compiler ever
// sees an unrecognized `#pragma`.
//
// Including this header requires the target's `CHECK_GE`/`CHECK_LE` to already
// be defined; each target's `vec.h` includes its own `logging.h` first.
#if defined(TAPA_TARGET_XILINX_HLS_)
#define TAPA_VEC_INLINE _Pragma("HLS inline")
#define TAPA_VEC_UNROLL _Pragma("HLS unroll")
#define TAPA_VEC_AGGREGATE _Pragma("HLS aggregate variable = this bit")
#else
#define TAPA_VEC_INLINE
#define TAPA_VEC_UNROLL
#define TAPA_VEC_AGGREGATE
#endif

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
    TAPA_VEC_INLINE;
    TAPA_VEC_AGGREGATE;
    return base_type::operator[](pos);
  }
  reference operator[](size_type pos) {
    TAPA_VEC_INLINE;
    TAPA_VEC_AGGREGATE;
    return base_type::operator[](pos);
  }
  constexpr const_reference get(size_type pos) const {
    TAPA_VEC_INLINE;
    return (*this)[pos];
  }
  void set(size_type pos, const T& value) {
    TAPA_VEC_INLINE;
    (*this)[pos] = value;
  }

  using base_type::base_type;
  using base_type::operator=;

  explicit vec_t(const base_type& other) : base_type(other) {}
  explicit vec_t(base_type&& other) : base_type(other) {}

  template <typename U>
  explicit operator vec_t<U, N>() const {
    TAPA_VEC_INLINE;
    vec_t<U, N> result;
    for (size_type i = 0; i < N; ++i) {
      result.set(i, static_cast<U>(get(i)));
    }
    return result;
  }

  /// Sets every element to @p val.
  void set(T val) {
    TAPA_VEC_INLINE;
    *this = val;
  }

  vec_t& operator=(T val) {
    TAPA_VEC_INLINE;
    for (size_type i = 0; i < N; ++i) {
      TAPA_VEC_UNROLL;
      set(i, val);
    }
    return *this;
  }

#define TAPA_VEC_DEFINE_OP(op)                           \
  template <typename T2>                                 \
  vec_t<T, N>& operator op##=(const vec_t<T2, N>& rhs) { \
    TAPA_VEC_INLINE;                                     \
    for (size_type i = 0; i < N; ++i) {                  \
      TAPA_VEC_UNROLL;                                   \
      set(i, get(i) op rhs[i]);                          \
    }                                                    \
    return *this;                                        \
  }                                                      \
  template <typename T2>                                 \
  vec_t<T, N>& operator op##=(const T2 & rhs) {          \
    TAPA_VEC_INLINE;                                     \
    for (size_type i = 0; i < N; ++i) {                  \
      TAPA_VEC_UNROLL;                                   \
      set(i, get(i) op rhs);                             \
    }                                                    \
    return *this;                                        \
  }
  TAPA_VEC_DEFINE_OP(+)
  TAPA_VEC_DEFINE_OP(-)
  TAPA_VEC_DEFINE_OP(*)
  TAPA_VEC_DEFINE_OP(/)
  TAPA_VEC_DEFINE_OP(%)
  TAPA_VEC_DEFINE_OP(&)
  TAPA_VEC_DEFINE_OP(|)
  TAPA_VEC_DEFINE_OP(^)
  TAPA_VEC_DEFINE_OP(<<)
  TAPA_VEC_DEFINE_OP(>>)
#undef TAPA_VEC_DEFINE_OP

// Unary operators build a new vector: `-v` must not negate `v` in place.
#define TAPA_VEC_DEFINE_OP(op)          \
  vec_t<T, N> operator op() const {     \
    TAPA_VEC_INLINE;                    \
    vec_t<T, N> result;                 \
    for (size_type i = 0; i < N; ++i) { \
      TAPA_VEC_UNROLL;                  \
      result.set(i, op get(i));         \
    }                                   \
    return result;                      \
  }
  TAPA_VEC_DEFINE_OP(+)
  TAPA_VEC_DEFINE_OP(-)
  TAPA_VEC_DEFINE_OP(~)
#undef TAPA_VEC_DEFINE_OP

// Binary operators read the left operand and leave it alone, so they are
// const: `a + b` must work when `a` is a `const vec_t`.
#define TAPA_VEC_DEFINE_OP(op)                             \
  template <typename T2>                                   \
  vec_t<T, N> operator op(const vec_t<T2, N>& rhs) const { \
    TAPA_VEC_INLINE;                                       \
    vec_t<T, N> result;                                    \
    for (size_type i = 0; i < N; ++i) {                    \
      TAPA_VEC_UNROLL;                                     \
      result.set(i, get(i) op rhs[i]);                     \
    }                                                      \
    return result;                                         \
  }                                                        \
  template <typename T2>                                   \
  vec_t<T, N> operator op(const T2 & rhs) const {          \
    TAPA_VEC_INLINE;                                       \
    vec_t<T, N> result;                                    \
    for (size_type i = 0; i < N; ++i) {                    \
      TAPA_VEC_UNROLL;                                     \
      result.set(i, get(i) op rhs);                        \
    }                                                      \
    return result;                                         \
  }
  TAPA_VEC_DEFINE_OP(+)
  TAPA_VEC_DEFINE_OP(-)
  TAPA_VEC_DEFINE_OP(*)
  TAPA_VEC_DEFINE_OP(/)
  TAPA_VEC_DEFINE_OP(%)
  TAPA_VEC_DEFINE_OP(&)
  TAPA_VEC_DEFINE_OP(|)
  TAPA_VEC_DEFINE_OP(^)
  TAPA_VEC_DEFINE_OP(<<)
  TAPA_VEC_DEFINE_OP(>>)
#undef TAPA_VEC_DEFINE_OP

  /// Shifts all elements left by 1, discarding [0] and placing @p val at [N-1].
  void shift(const T& val) {
    TAPA_VEC_INLINE;
    for (size_type i = 1; i < N; ++i) {
      TAPA_VEC_UNROLL;
      set(i - 1, get(i));
    }
    set(N - 1, val);
  }

  /// Returns whether any element equals @p val.
  ///
  /// Scans all N elements rather than returning early so HLS can unroll this
  /// into an OR tree; N is a compile-time constant, so nothing is lost on host.
  bool has(const T& val) const {
    TAPA_VEC_INLINE;
    bool result = false;
    for (size_type i = 0; i < N; ++i) {
      TAPA_VEC_UNROLL;
      if (val == get(i)) result = true;
    }
    return result;
  }
};

/// Returns vec[begin:end].
template <int begin, int end, typename T, int N>
inline vec_t<T, end - begin> truncated(const vec_t<T, N>& vec) {
  TAPA_VEC_INLINE;
  static_assert(begin >= 0, "cannot truncate before 0");
  static_assert(end <= N, "cannot truncate after N");
  vec_t<T, end - begin> result;
  for (int i = 0; i < end - begin; ++i) {
    TAPA_VEC_UNROLL;
    result.set(i, vec[begin + i]);
  }
  return result;
}

/// Returns vec[:length].
template <int length, typename T, int N>
inline vec_t<T, length> truncated(const vec_t<T, N>& vec) {
  TAPA_VEC_INLINE;
  return truncated<0, length>(vec);
}

/// Returns vec[begin:begin+length].
template <int length, typename T, int N>
inline vec_t<T, length> truncated(const vec_t<T, N>& vec, int begin) {
  TAPA_VEC_INLINE;
  static_assert(length <= N, "cannot enlarge vector");
  CHECK_GE(begin, 0) << "cannot truncate before 0";
  CHECK_LE(begin + length, N) << "cannot truncate after N";
  vec_t<T, length> result;
  for (int i = 0; i < length; ++i) {
    TAPA_VEC_UNROLL;
    result.set(i, vec[begin + i]);
  }
  return result;
}

/// Returns vec[:] + [val].
template <typename T, int N>
inline vec_t<T, N + 1> cat(const vec_t<T, N>& vec, const T& val) {
  TAPA_VEC_INLINE;
  vec_t<T, N + 1> result;
  for (int i = 0; i < N; ++i) {
    TAPA_VEC_UNROLL;
    result.set(i, vec[i]);
  }
  result.set(N, val);
  return result;
}

/// Returns [val] + vec[:].
template <typename T, int N>
inline vec_t<T, N + 1> cat(const T& val, const vec_t<T, N>& vec) {
  TAPA_VEC_INLINE;
  vec_t<T, N + 1> result;
  result.set(0, val);
  for (int i = 0; i < N; ++i) {
    TAPA_VEC_UNROLL;
    result.set(i + 1, vec[i]);
  }
  return result;
}

/// Returns v1[:] + v2[:].
template <typename T, int N1, int N2>
inline vec_t<T, N1 + N2> cat(const vec_t<T, N1>& v1, const vec_t<T, N2>& v2) {
  TAPA_VEC_INLINE;
  vec_t<T, N1 + N2> result;
  for (int i = 0; i < N1; ++i) {
    TAPA_VEC_UNROLL;
    result.set(i, v1[i]);
  }
  for (int i = 0; i < N2; ++i) {
    TAPA_VEC_UNROLL;
    result.set(i + N1, v2[i]);
  }
  return result;
}

#if __cplusplus >= 201402L
template <typename T, typename... Args>
inline auto cat(T arg, Args... args) {
  TAPA_VEC_INLINE;
  return cat(arg, cat(args...));
}
#endif  // __cplusplus >= 201402L

namespace internal {

/// A stream on the left of `<<` means insertion, not an elementwise shift.
///
/// Without this, `operator<<(const T2&, const vec_t&)` below binds an
/// `ostringstream` exactly while the stream-insertion overload needs a
/// derived-to-base conversion, so the shift wins and `os << vec` fails to
/// compile for every stream type that is not exactly `std::ostream&`.
template <typename T>
using enable_if_not_stream =
    typename std::enable_if<!std::is_base_of<std::ios_base, T>::value>::type;

}  // namespace internal

#define TAPA_VEC_DEFINE_OP(op)                                      \
  template <typename T, int N, typename T2,                         \
            typename = internal::enable_if_not_stream<T2>>          \
  vec_t<T, N> operator op(const T2 & lhs, const vec_t<T, N>& rhs) { \
    TAPA_VEC_INLINE;                                                \
    vec_t<T, N> result;                                             \
    for (int i = 0; i < N; ++i) {                                   \
      TAPA_VEC_UNROLL;                                              \
      result.set(i, lhs op rhs[i]);                                 \
    }                                                               \
    return result;                                                  \
  }
TAPA_VEC_DEFINE_OP(+)
TAPA_VEC_DEFINE_OP(-)
TAPA_VEC_DEFINE_OP(*)
TAPA_VEC_DEFINE_OP(/)
TAPA_VEC_DEFINE_OP(%)
TAPA_VEC_DEFINE_OP(&)
TAPA_VEC_DEFINE_OP(|)
TAPA_VEC_DEFINE_OP(^)
TAPA_VEC_DEFINE_OP(<<)
TAPA_VEC_DEFINE_OP(>>)
#undef TAPA_VEC_DEFINE_OP

/// Returns a vector of @p N elements all equal to @p val.
template <int N, typename T>
vec_t<T, N> make_vec(T val) {
  TAPA_VEC_INLINE;
  vec_t<T, N> result;
  result.set(val);
  return result;
}

#define TAPA_VEC_DEFINE_FUNC(func)    \
  template <typename T, int N>        \
  vec_t<T, N> func(vec_t<T, N> vec) { \
    TAPA_VEC_INLINE;                  \
    for (int i = 0; i < N; ++i) {     \
      TAPA_VEC_UNROLL;                \
      vec.set(i, std::func(vec[i]));  \
    }                                 \
    return vec;                       \
  }
TAPA_VEC_DEFINE_FUNC(exp)
TAPA_VEC_DEFINE_FUNC(exp2)
TAPA_VEC_DEFINE_FUNC(expm1)
TAPA_VEC_DEFINE_FUNC(log)
TAPA_VEC_DEFINE_FUNC(log10)
TAPA_VEC_DEFINE_FUNC(log1p)
TAPA_VEC_DEFINE_FUNC(log2)
#undef TAPA_VEC_DEFINE_FUNC

#define TAPA_VEC_DEFINE_FUNC(func)                                   \
  template <typename T, int N>                                       \
  vec_t<T, N> func(const vec_t<T, N>& lhs, const vec_t<T, N>& rhs) { \
    TAPA_VEC_INLINE;                                                 \
    vec_t<T, N> result;                                              \
    for (int i = 0; i < N; ++i) {                                    \
      TAPA_VEC_UNROLL;                                               \
      result.set(i, std::func(lhs[i], rhs[i]));                      \
    }                                                                \
    return result;                                                   \
  }                                                                  \
  template <typename T, int N>                                       \
  vec_t<T, N> func(const T& lhs, const vec_t<T, N>& rhs) {           \
    TAPA_VEC_INLINE;                                                 \
    return func(make_vec<N>(lhs), rhs);                              \
  }                                                                  \
  template <typename T, int N>                                       \
  vec_t<T, N> func(const vec_t<T, N>& lhs, const T& rhs) {           \
    TAPA_VEC_INLINE;                                                 \
    return func(lhs, make_vec<N>(rhs));                              \
  }
TAPA_VEC_DEFINE_FUNC(max)
TAPA_VEC_DEFINE_FUNC(min)
#undef TAPA_VEC_DEFINE_FUNC

#define TAPA_VEC_DEFINE_FUNC(func, op)                                    \
  template <typename T>                                                   \
  T func(const vec_t<T, 1>& vec) {                                        \
    TAPA_VEC_INLINE;                                                      \
    return vec[0];                                                        \
  }                                                                       \
  template <typename T, int N>                                            \
  T func(const vec_t<T, N>& vec) {                                        \
    TAPA_VEC_INLINE;                                                      \
    return func(truncated<N / 2>(vec)) op func(truncated<N / 2, N>(vec)); \
  }
TAPA_VEC_DEFINE_FUNC(sum, +)
TAPA_VEC_DEFINE_FUNC(product, *)
#undef TAPA_VEC_DEFINE_FUNC

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

// Expansion already happened above; nothing downstream should see these.
#undef TAPA_VEC_INLINE
#undef TAPA_VEC_UNROLL
#undef TAPA_VEC_AGGREGATE
