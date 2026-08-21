// Fixed-point types: tapa::fixed<W, I, Q, O, N> (signed) and
// tapa::ufixed<...> (unsigned), the portable form of ap_fixed / ap_ufixed.
//
// A value is an W-bit two's-complement integer scaled by 2^-(W-I): I bits
// above the binary point (the sign bit among them when signed), W - I below.
// Either count may exceed W or go negative, as the vendor allows.
//
// Q and O say what happens when a result does not fit. Q quantizes away the
// fractional bits the target cannot hold; O then brings what is left into
// range. Both mirror the vendor's ap_q_mode / ap_o_mode exactly, including
// their default (truncate toward minus infinity, wrap around).
//
// On synthesis targets these alias the vendor types instead
// (tapa/xilinx/hls/fixed.h), the way tapa::u/i do -- fixed-point arithmetic
// is something the HLS compiler implements natively, and an emulation of it
// written in C++ would synthesize into something else entirely. The enums
// below are declared for every target so that one spelling of the mode
// arguments works in both places.

#pragma once

namespace tapa {

/// What to do with fractional bits the target cannot hold. The enumerator
/// order matches the vendor's ap_q_mode, which the Xilinx alias casts to.
enum class q_mode {
  rnd,          ///< round toward plus infinity
  rnd_zero,     ///< round toward zero
  rnd_min_inf,  ///< round toward minus infinity
  rnd_inf,      ///< round away from zero
  rnd_conv,     ///< round to nearest, ties to even
  trn,          ///< truncate toward minus infinity (the default)
  trn_zero,     ///< truncate toward zero
};

/// What to do with a value that does not fit. The enumerator order matches
/// the vendor's ap_o_mode.
enum class o_mode {
  sat,       ///< clamp to the largest representable magnitude
  sat_zero,  ///< replace with zero
  sat_sym,   ///< clamp to a range symmetric about zero
  wrap,      ///< keep the low bits (the default)
  wrap_sm,   ///< sign-magnitude wrap-around
};

}  // namespace tapa

// The self-implemented layer, exactly as in tapa/base/int.h: under the
// Xilinx target this file contributes only the enums above, and
// tapa/xilinx/hls/fixed.h aliases the vendor types.
#if !defined(TAPA_TARGET_XILINX_HLS_)

#include <climits>
#include <cmath>
#include <cstdint>
#include <ostream>
#include <string>
#include <type_traits>
#include <utility>

#include "tapa/base/int.h"

namespace tapa {

template <int W, int I, q_mode Q, o_mode O, int N>
class fixed;
template <int W, int I, q_mode Q, o_mode O, int N>
class ufixed;

namespace internal {

constexpr int imax(int a, int b) { return a > b ? a : b; }
constexpr int imin(int a, int b) { return a < b ? a : b; }

// Bit `k` of the infinite two's-complement extension of `x`: above the
// stored width every bit repeats the sign, which is zero when unsigned.
template <int W, bool S>
inline bool bit_at(const int_base<W, S>& x, int k) {
  if (k < 0) return false;
  if (k < W) return x.get_bit(k);
  return S && x.get_bit(W - 1);
}

// Whether any of bits [0, k] is set, with the same extension.
template <int W, bool S>
inline bool any_up_to(const int_base<W, S>& x, int k) {
  if (k < 0) return false;
  if (k >= W && S && x.get_bit(W - 1)) return true;
  const int top = imin(k, W - 1);
  for (int b = 0; b <= top; ++b) {
    if (x.get_bit(b)) return true;
  }
  return false;
}

/// The exact multiplier of a value under a new scale: given `m` whose value
/// is m * 2^-fs, the integer q whose value q * 2^-ft is what mode @p q_arg
/// makes of it. Returned wide and unbounded -- the range is O's business.
template <int Wq, int Wm, bool Sm>
inline i<Wq> quantize(const int_base<Wm, Sm>& m, int fs, int ft, q_mode q_arg) {
  const int s = fs - ft;
  const i<Wq> wide(m);
  if (s <= 0) return i<Wq>(wide << (-s));

  // An arithmetic shift right floors, which is AP_TRN itself; every other
  // mode is that plus a carry decided by the bit below the new LSB (`qb`)
  // and whether anything below THAT is set (`r`).
  const i<Wq> trunc(wide >> imin(s, Wq - 1));
  const bool sign = Sm && m.get_bit(Wm - 1);
  const bool qb = bit_at(m, s - 1);
  const bool r = any_up_to(m, s - 2);
  bool inc = false;
  switch (q_arg) {
    case q_mode::trn:
      inc = false;
      break;
    case q_mode::rnd:
      inc = qb;
      break;
    case q_mode::rnd_zero:
      inc = qb && (sign || r);
      break;
    case q_mode::rnd_min_inf:
      inc = qb && r;
      break;
    case q_mode::rnd_inf:
      inc = qb && (!sign || r);
      break;
    case q_mode::rnd_conv:
      inc = qb && (trunc.get_bit(0) || r);
      break;
    case q_mode::trn_zero:
      inc = sign && (qb || r);
      break;
  }
  return inc ? i<Wq>(trunc + i<Wq>(1)) : trunc;
}

/// Bring the exact multiplier @p q into the W-bit range under mode O.
///
/// @param src_negative  Sign of the value being converted, which AP_WRAP
///                      with saturation bits and AP_WRAP_SM both read.
/// @param low_deleted   The bit just above the ones the target keeps, which
///                      AP_WRAP_SM reads.
template <int W, bool S, o_mode O, int N, int Wq>
inline typename std::conditional<S, i<W>, u<W>>::type overflow_adjust(
    const i<Wq>& q, bool src_negative, bool low_deleted) {
  using raw_type = typename std::conditional<S, i<W>, u<W>>::type;
  raw_type raw(q);  // the low W bits: AP_WRAP with no saturation bits

  const i<Wq> one(1);
  const i<Wq> hi = S ? i<Wq>((one << (W - 1)) - one) : i<Wq>((one << W) - one);
  const i<Wq> lo = S ? i<Wq>(-(one << (W - 1))) : i<Wq>(0);
  bool overflow = q > hi;
  bool underflow = q < lo;
  // A symmetric range gives up the one extra negative value.
  if (O == o_mode::sat_sym && S && q == lo) underflow = true;
  if (!overflow && !underflow && O != o_mode::wrap_sm) {
    if (O != o_mode::wrap || N == 0) return raw;
  }

  switch (O) {
    case o_mode::wrap: {
      if (N == 0) return raw;
      if (!overflow && !underflow) return raw;
      if (S) {
        raw.set_bit(W - 1, src_negative);
        for (int b = W - N; b <= W - 2; ++b) raw.set_bit(b, !src_negative);
      } else {
        for (int b = W - N; b <= W - 1; ++b) raw.set_bit(b, true);
      }
      return raw;
    }
    case o_mode::sat_zero:
      return raw_type(0);
    case o_mode::wrap_sm: {
      if (!S) {
        // The vendor's sign-magnitude wrap is a no-op without a sign bit.
        return raw;
      }
      if (!overflow && !underflow) return raw;
      const bool ro = raw.get_bit(W - 1);
      if (N == 0) {
        if (low_deleted != ro) {
          raw = raw_type(~raw);
          raw.set_bit(W - 1, low_deleted);
        }
      } else {
        if (N == 1 && src_negative != ro) {
          raw = raw_type(~raw);
        } else if (N > 1) {
          if (raw.get_bit(W - N) == src_negative) raw = raw_type(~raw);
          for (int b = W - N; b <= W - 2; ++b) raw.set_bit(b, !src_negative);
        }
        raw.set_bit(W - 1, src_negative);
      }
      return raw;
    }
    default: {  // sat and sat_sym
      if (S) {
        if (overflow) return raw_type(hi);
        return raw_type(O == o_mode::sat_sym ? i<Wq>(lo + one) : lo);
      }
      return overflow ? raw_type(~u<W>(0)) : raw_type(0);
    }
  }
}

// Result types of mixed fixed-point arithmetic, term for term the vendor's
// RType: widths widen so a sum or product is exact, and every result
// carries the DEFAULT quantization and overflow modes, not the operands'.
template <int W1, int I1, bool S1, int W2, int I2, bool S2>
struct frtype {
  static constexpr int kF1 = W1 - I1;
  static constexpr int kF2 = W2 - I2;
  static constexpr int kI =
      imax(I1 + ((S2 && !S1) ? 1 : 0), I2 + ((S1 && !S2) ? 1 : 0));
  static constexpr int kF = imax(kF1, kF2);
  static constexpr bool kS = S1 || S2;

  template <int W, int I, bool S>
  using kind = typename std::conditional<
      S, fixed<W, I, q_mode::trn, o_mode::wrap, 0>,
      ufixed<W, I, q_mode::trn, o_mode::wrap, 0>>::type;

  using mult = kind<W1 + W2, I1 + I2, kS>;
  using plus = kind<kI + 1 + kF, kI + 1, kS>;
  using minus = kind<kI + 1 + kF, kI + 1, true>;
  using logic = kind<kI + kF, kI, kS>;
  using div = kind<S2 + W1 + imax(kF2, 0), S2 + I1 + kF2, kS>;
  using arg1 = kind<W1, I1, S1>;
};

struct raw_tag {};

}  // namespace internal

/// A W-bit fixed-point value with I bits above the binary point.
///
/// @tparam W  Total bits.
/// @tparam I  Bits above the binary point, sign bit included when @p S.
/// @tparam S  Signedness.
/// @tparam Q  Quantization mode.
/// @tparam O  Overflow mode.
/// @tparam N  Saturation bits, read by AP_WRAP and AP_WRAP_SM.
template <int W, int I, bool S, q_mode Q, o_mode O, int N>
class fixed_base {
  static_assert(W > 0, "a fixed-point value needs at least one bit");
  // The vendor rejects this combination too, but at run time, with a
  // message and an abort from inside whichever conversion first hit it.
  static_assert(S || O != o_mode::wrap_sm,
                "sign-magnitude wrap needs a sign bit; an unsigned "
                "fixed-point type cannot use o_mode::wrap_sm");

 public:
  static constexpr int width = W;
  static constexpr int iwidth = I;
  static constexpr int fwidth = W - I;
  static constexpr bool is_signed = S;
  static constexpr q_mode qmode = Q;
  static constexpr o_mode omode = O;
  static constexpr int nbits = N;

  using raw_type = typename std::conditional<S, i<W>, u<W>>::type;

  /// The raw bit pattern, named as the vendor names it: the value is
  /// `V * 2^-(W - I)`.
  raw_type V;

  fixed_base() : V(0) {}
  fixed_base(internal::raw_tag, const raw_type& raw) : V(raw) {}

  template <typename T,
            typename std::enable_if<std::is_integral<T>::value, int>::type = 0>
  fixed_base(T x) {  // NOLINT(google-explicit-constructor)
    assign_int(x);
  }
  fixed_base(double x) { assign_double(x); }                      // NOLINT
  fixed_base(float x) { assign_double(static_cast<double>(x)); }  // NOLINT

  template <int W2, int I2, bool S2, q_mode Q2, o_mode O2, int N2>
  fixed_base(const fixed_base<W2, I2, S2, Q2, O2, N2>& x) {  // NOLINT
    assign_fixed(x);
  }

  /// From an arbitrary-width integer, which is a fixed-point value with no
  /// fractional bits.
  template <int W2, bool S2>
  fixed_base(const int_base<W2, S2>& x) {  // NOLINT
    assign_raw<W2, 0>(x, S2 && x.get_bit(W2 - 1));
  }

  template <int W2, int I2, bool S2, q_mode Q2, o_mode O2, int N2>
  fixed_base& operator=(const fixed_base<W2, I2, S2, Q2, O2, N2>& x) {
    assign_fixed(x);
    return *this;
  }

  // Without this, `x = 0` is ambiguous: converting the literal for the
  // template above and converting it for the implicit copy assignment are
  // both one user conversion. An exact match on the literal settles it.
  template <typename T, typename std::enable_if<std::is_arithmetic<T>::value,
                                                int>::type = 0>
  fixed_base& operator=(T x) {
    *this = fixed_base(x);
    return *this;
  }

  /// The exact value as a double, which loses bits beyond the mantissa the
  /// same way the vendor's does.
  double to_double() const { return std::ldexp(V.to_double(), -(W - I)); }
  float to_float() const { return static_cast<float>(to_double()); }
  /// Truncated toward zero, like a C++ cast of the exact value -- and like
  /// the vendor's, which does the same whatever the type's own Q mode says.
  int64_t to_int64() const {
    return static_cast<int64_t>(internal::quantize<internal::imax(W, 64) + 2>(
        V, W - I, 0, q_mode::trn_zero));
  }
  int64_t to_int() const { return to_int64(); }
  uint64_t to_uint64() const { return static_cast<uint64_t>(to_int64()); }
  int length() const { return W; }

#define TAPA_FIXED_COMPOUND(op)             \
  template <typename T>                     \
  fixed_base& operator op##=(const T & x) { \
    *this = *this op x;                     \
    return *this;                           \
  }
  TAPA_FIXED_COMPOUND(+)
  TAPA_FIXED_COMPOUND(-)
  TAPA_FIXED_COMPOUND(*)
  TAPA_FIXED_COMPOUND(/)
#undef TAPA_FIXED_COMPOUND

  /// Bit and slice access on the raw pattern, as on the vendor type.
  bool get_bit(int idx) const { return V.get_bit(idx); }
  bit_ref<W, S> operator[](int idx) & { return V[idx]; }
  bool operator[](int idx) const& { return V.get_bit(idx); }
  range_ref<W, S> operator()(int hi, int lo) & { return V(hi, lo); }
  u<W> operator()(int hi, int lo) const& { return V(hi, lo); }
  range_ref<W, S> range(int hi, int lo) & { return V(hi, lo); }
  u<W> range(int hi, int lo) const& { return V(hi, lo); }

  bool is_zero() const { return V.is_zero(); }
  bool operator!() const { return V.is_zero(); }

 private:
  template <int, int, bool, q_mode, o_mode, int>
  friend class fixed_base;

  template <typename T>
  static bool is_negative(T x) {
    return std::is_signed<T>::value && x < T(0);
  }

  template <typename T>
  void assign_int(T x) {
    // An integer is a fixed-point value with no fractional bits; from there
    // the same quantization and overflow rules apply.
    using widened =
        typename std::conditional<std::is_signed<T>::value, i<64>, u<64>>::type;
    assign_raw<64, 0>(widened(x), is_negative(x));
  }

  template <int W2, int I2, bool S2, q_mode Q2, o_mode O2, int N2>
  void assign_fixed(const fixed_base<W2, I2, S2, Q2, O2, N2>& x) {
    assign_raw<W2, W2 - I2>(x.V, S2 && x.V.get_bit(W2 - 1));
  }

  // The one path every conversion goes through: quantize the source onto
  // this scale exactly, then bring the result into range.
  template <int W2, int Fs, typename Src>
  void assign_raw(const Src& src, bool negative) {
    // Room for the source shifted onto this scale, for the target range it
    // is compared against, and for a rounding carry.
    static constexpr int kWq =
        internal::imax(W2 + internal::imax(fwidth - Fs, 0), W) + 3;
    const i<kWq> q = internal::quantize<kWq>(src, Fs, fwidth, Q);
    const bool low_deleted = internal::bit_at(src, W + Fs - fwidth);
    V = internal::overflow_adjust<W, S, O, N>(q, negative, low_deleted);
  }

  void assign_double(double x);
};

/// Signed fixed-point, the portable ap_fixed.
template <int W, int I, q_mode Q = q_mode::trn, o_mode O = o_mode::wrap,
          int N = 0>
class fixed : public fixed_base<W, I, true, Q, O, N> {
 public:
  using base = fixed_base<W, I, true, Q, O, N>;
  using base::base;
  using base::operator=;
  fixed() = default;
  fixed(const base& x) : base(x) {}  // NOLINT(google-explicit-constructor)
};

/// Unsigned fixed-point, the portable ap_ufixed.
template <int W, int I, q_mode Q = q_mode::trn, o_mode O = o_mode::wrap,
          int N = 0>
class ufixed : public fixed_base<W, I, false, Q, O, N> {
 public:
  using base = fixed_base<W, I, false, Q, O, N>;
  using base::base;
  using base::operator=;
  ufixed() = default;
  ufixed(const base& x) : base(x) {}  // NOLINT(google-explicit-constructor)
};

namespace internal {

// Both operands rescaled to a common fractional width in one wide integer,
// which is where every exact arithmetic result is computed.
template <int Wr, int Fr, int W, int I, bool S, q_mode Q, o_mode O, int N>
inline i<Wr> aligned(const fixed_base<W, I, S, Q, O, N>& x) {
  return i<Wr>(i<Wr>(x.V) << (Fr - (W - I)));
}

template <typename Result, int Wr>
inline Result from_wide(const i<Wr>& raw) {
  return Result(raw_tag{}, typename Result::raw_type(raw));
}

}  // namespace internal

#define TAPA_FIXED_BINARY_ARITH(op, member)                                   \
  template <int W1, int I1, bool S1, q_mode Q1, o_mode O1, int N1, int W2,    \
            int I2, bool S2, q_mode Q2, o_mode O2, int N2>                    \
  inline typename internal::frtype<W1, I1, S1, W2, I2, S2>::member            \
  operator op(const fixed_base<W1, I1, S1, Q1, O1, N1>& lhs,                  \
              const fixed_base<W2, I2, S2, Q2, O2, N2>& rhs) {                \
    using result = typename internal::frtype<W1, I1, S1, W2, I2, S2>::member; \
    constexpr int kFr = result::fwidth;                                       \
    constexpr int kWr = result::width + 2;                                    \
    return internal::from_wide<result>(i<kWr>(internal::aligned<kWr, kFr>(    \
        lhs) op internal::aligned<kWr, kFr>(rhs)));                           \
  }

TAPA_FIXED_BINARY_ARITH(+, plus)
TAPA_FIXED_BINARY_ARITH(-, minus)
#undef TAPA_FIXED_BINARY_ARITH

// The product is exact in W1 + W2 bits with F1 + F2 fractional ones, so the
// raw patterns multiply directly with no rescaling at all.
template <int W1, int I1, bool S1, q_mode Q1, o_mode O1, int N1, int W2, int I2,
          bool S2, q_mode Q2, o_mode O2, int N2>
inline typename internal::frtype<W1, I1, S1, W2, I2, S2>::mult operator*(
    const fixed_base<W1, I1, S1, Q1, O1, N1>& lhs,
    const fixed_base<W2, I2, S2, Q2, O2, N2>& rhs) {
  using result = typename internal::frtype<W1, I1, S1, W2, I2, S2>::mult;
  constexpr int kWr = W1 + W2 + 2;
  return internal::from_wide<result>(i<kWr>(i<kWr>(lhs.V) * i<kWr>(rhs.V)));
}

// Division scales the dividend up by the divisor's fractional width and
// then divides the raw patterns, truncating toward zero -- the vendor does
// exactly this, and the result width is chosen so it is exact.
template <int W1, int I1, bool S1, q_mode Q1, o_mode O1, int N1, int W2, int I2,
          bool S2, q_mode Q2, o_mode O2, int N2>
inline typename internal::frtype<W1, I1, S1, W2, I2, S2>::div operator/(
    const fixed_base<W1, I1, S1, Q1, O1, N1>& lhs,
    const fixed_base<W2, I2, S2, Q2, O2, N2>& rhs) {
  using result = typename internal::frtype<W1, I1, S1, W2, I2, S2>::div;
  constexpr int kF2 = W2 - I2;
  constexpr int kWr = W1 + internal::imax(kF2, 0) + W2 + 3;
  const i<kWr> num(i<kWr>(lhs.V) << internal::imax(kF2, 0));
  const i<kWr> den(rhs.V);
  return internal::from_wide<result>(i<kWr>(num / den));
}

// The vendor compares by widening whichever operand has the coarser
// fractional width and then comparing the RAW integers. Transcribed rather
// than reasoned about: comparing the exact values instead would be more
// sensible and would not be what a design synthesizes to.
#define TAPA_FIXED_COMPARE(op)                                                 \
  template <int W1, int I1, bool S1, q_mode Q1, o_mode O1, int N1, int W2,     \
            int I2, bool S2, q_mode Q2, o_mode O2, int N2>                     \
  inline bool operator op(const fixed_base<W1, I1, S1, Q1, O1, N1>& lhs,       \
                          const fixed_base<W2, I2, S2, Q2, O2, N2>& rhs) {     \
    constexpr int kF1 = W1 - I1;                                               \
    constexpr int kF2 = W2 - I2;                                               \
    if constexpr (kF1 == kF2) {                                                \
      return lhs.V op rhs.V;                                                   \
    } else if constexpr (kF1 > kF2) {                                          \
      return lhs                                                               \
          .V op fixed_base<internal::imax(W2 + kF1 - kF2, 1), I2, S2, Q2, O2,  \
                           N2>(rhs)                                            \
          .V;                                                                  \
    } else {                                                                   \
      return fixed_base<internal::imax(W1 + kF2 - kF1 + 1, 1), I1 + 1, S1, Q1, \
                        O1, N1>(lhs)                                           \
          .V op rhs.V;                                                         \
    }                                                                          \
  }

TAPA_FIXED_COMPARE(==)
TAPA_FIXED_COMPARE(!=)
TAPA_FIXED_COMPARE(<)
TAPA_FIXED_COMPARE(<=)
TAPA_FIXED_COMPARE(>)
TAPA_FIXED_COMPARE(>=)
#undef TAPA_FIXED_COMPARE

template <int W, int I, bool S, q_mode Q, o_mode O, int N>
inline typename internal::frtype<W, I, S, W, I, S>::minus operator-(
    const fixed_base<W, I, S, Q, O, N>& x) {
  using result = typename internal::frtype<W, I, S, W, I, S>::minus;
  constexpr int kWr = result::width + 2;
  return internal::from_wide<result>(i<kWr>(-i<kWr>(x.V)));
}

namespace internal {

// An operand that is not fixed-point, seen as one. The vendor widens
// `ufixed<32,8> * 2` as if the 2 were a 32-bit SIGNED value with 32 integer
// bits -- the width of the C type, not of the literal -- and an ap_uint<4>
// as a 4-bit unsigned one. Same rule here, so the result widths agree.
template <typename T, typename = void>
struct as_fixed {};

template <typename T>
struct as_fixed<T, typename std::enable_if<std::is_integral<T>::value>::type> {
  // The vendor widens bool as a 1-bit unsigned value, not an 8-bit one.
  static constexpr int kW =
      std::is_same<T, bool>::value ? 1 : static_cast<int>(sizeof(T)) * CHAR_BIT;
  using type = fixed_base<kW, kW, std::is_signed<T>::value, q_mode::trn,
                          o_mode::wrap, 0>;
};

template <int W>
struct as_fixed<u<W>> {
  using type = fixed_base<W, W, false, q_mode::trn, o_mode::wrap, 0>;
};

template <int W>
struct as_fixed<i<W>> {
  using type = fixed_base<W, W, true, q_mode::trn, o_mode::wrap, 0>;
};

}  // namespace internal

// Arithmetic against a plain integer or a tapa::u/i, in either order.
#define TAPA_FIXED_MIXED_ARITH(op)                                             \
  template <int W1, int I1, bool S1, q_mode Q1, o_mode O1, int N1, typename T, \
            typename U = typename internal::as_fixed<T>::type>                 \
  inline auto operator op(const fixed_base<W1, I1, S1, Q1, O1, N1>& lhs,       \
                          const T& rhs)                                        \
      ->decltype(lhs op std::declval<const U&>()) {                            \
    return lhs op U(rhs);                                                      \
  }                                                                            \
  template <int W1, int I1, bool S1, q_mode Q1, o_mode O1, int N1, typename T, \
            typename U = typename internal::as_fixed<T>::type>                 \
  inline auto operator op(const T& lhs,                                        \
                          const fixed_base<W1, I1, S1, Q1, O1, N1>& rhs)       \
      ->decltype(std::declval<const U&>() op rhs) {                            \
    return U(lhs) op rhs;                                                      \
  }

TAPA_FIXED_MIXED_ARITH(+)
TAPA_FIXED_MIXED_ARITH(-)
TAPA_FIXED_MIXED_ARITH(*)
TAPA_FIXED_MIXED_ARITH(/)
#undef TAPA_FIXED_MIXED_ARITH

// Comparison against a plain number: the vendor declares an overload per
// integral C type that wraps the operand as a fixed-point of that type's
// width and compares raw integers EXACTLY, plus a double overload for
// floating operands. Mirror both: integral goes through as_fixed (the
// double path loses bits past the 53-bit mantissa and flips comparisons
// against int64 literals), floating goes through double.
#define TAPA_FIXED_NATIVE_COMPARE(op)                                          \
  template <int W, int I, bool S, q_mode Q, o_mode O, int N, typename T,       \
            typename std::enable_if<std::is_integral<T>::value, int>::type =   \
                0>                                                             \
  inline bool operator op(const fixed_base<W, I, S, Q, O, N>& lhs, T rhs) {    \
    return lhs op typename internal::as_fixed<T>::type(rhs);                   \
  }                                                                            \
  template <int W, int I, bool S, q_mode Q, o_mode O, int N, typename T,       \
            typename std::enable_if<std::is_integral<T>::value, int>::type =   \
                0>                                                             \
  inline bool operator op(T lhs, const fixed_base<W, I, S, Q, O, N>& rhs) {    \
    return typename internal::as_fixed<T>::type(lhs) op rhs;                   \
  }                                                                            \
  template <int W, int I, bool S, q_mode Q, o_mode O, int N, typename T,       \
            typename std::enable_if<std::is_floating_point<T>::value,          \
                                    int>::type = 0>                            \
  inline bool operator op(const fixed_base<W, I, S, Q, O, N>& lhs, T rhs) {    \
    return lhs.to_double() op static_cast<double>(rhs);                        \
  }                                                                            \
  template <int W, int I, bool S, q_mode Q, o_mode O, int N, typename T,       \
            typename std::enable_if<std::is_floating_point<T>::value,          \
                                    int>::type = 0>                            \
  inline bool operator op(T lhs, const fixed_base<W, I, S, Q, O, N>& rhs) {    \
    return static_cast<double>(lhs) op rhs.to_double();                        \
  }

TAPA_FIXED_NATIVE_COMPARE(==)
TAPA_FIXED_NATIVE_COMPARE(!=)
TAPA_FIXED_NATIVE_COMPARE(<)
TAPA_FIXED_NATIVE_COMPARE(<=)
TAPA_FIXED_NATIVE_COMPARE(>)
TAPA_FIXED_NATIVE_COMPARE(>=)
#undef TAPA_FIXED_NATIVE_COMPARE

// Against a tapa::u/i, through the fixed-point path rather than through
// double: no precision is lost and the widths are already comparable.
#define TAPA_FIXED_INT_COMPARE(op)                                            \
  template <int W, int I, bool S, q_mode Q, o_mode O, int N, int W2, bool S2> \
  inline bool operator op(const fixed_base<W, I, S, Q, O, N>& lhs,            \
                          const int_base<W2, S2>& rhs) {                      \
    return lhs op typename internal::as_fixed<                                \
        typename std::conditional<S2, i<W2>, u<W2>>::type>::type(rhs);        \
  }                                                                           \
  template <int W, int I, bool S, q_mode Q, o_mode O, int N, int W2, bool S2> \
  inline bool operator op(const int_base<W2, S2>& lhs,                        \
                          const fixed_base<W, I, S, Q, O, N>& rhs) {          \
    return typename internal::as_fixed<                                       \
        typename std::conditional<S2, i<W2>, u<W2>>::type>::type(lhs) op rhs; \
  }

TAPA_FIXED_INT_COMPARE(==)
TAPA_FIXED_INT_COMPARE(!=)
TAPA_FIXED_INT_COMPARE(<)
TAPA_FIXED_INT_COMPARE(<=)
TAPA_FIXED_INT_COMPARE(>)
TAPA_FIXED_INT_COMPARE(>=)
#undef TAPA_FIXED_INT_COMPARE

template <int W, int I, bool S, q_mode Q, o_mode O, int N>
inline std::ostream& operator<<(std::ostream& os,
                                const fixed_base<W, I, S, Q, O, N>& x) {
  return os << x.to_double();
}

// The double conversion, out of line because it needs the class complete.
//
// A finite double is an exact binary fraction: frexp splits it into a
// 53-bit integer and a power of two, which is a fixed-point value like any
// other and goes down the same path. Only the extreme exponents need care,
// where the shift would be wider than any integer worth building: the value
// is then so far out of range that its low bits are all zero, which is what
// wrapping keeps and saturating ignores.
template <int W, int I, bool S, q_mode Q, o_mode O, int N>
inline void fixed_base<W, I, S, Q, O, N>::assign_double(double x) {
  if (!(x != 0)) {  // zero, and NaN, for which the vendor also stores zero
    V = raw_type(0);
    return;
  }
  int exp = 0;
  const double mant = std::frexp(x, &exp);  // |mant| in [0.5, 1)
  const int64_t bits = static_cast<int64_t>(std::ldexp(mant, 53));
  const int fs = 53 - exp;  // value == bits * 2^-fs
  const bool negative = x < 0;

  // Below this the shift is bounded by the widths the type already carries.
  constexpr int kWq = internal::imax(W, 64) + 70;
  if (fwidth - fs >= kWq - 64) {
    // Astronomically large: every retained bit is zero, and the sign
    // decides which end a saturating mode clamps to.
    const i<kWq> huge =
        negative ? i<kWq>(-1) << (kWq - 2) : i<kWq>(1) << (kWq - 2);
    V = internal::overflow_adjust<W, S, O, N>(huge, negative, false);
    return;
  }
  const i<64> src(bits);
  const i<kWq> q = internal::quantize<kWq>(src, fs, fwidth, Q);
  const bool low_deleted = internal::bit_at(src, W + fs - fwidth);
  V = internal::overflow_adjust<W, S, O, N>(q, negative, low_deleted);
}

}  // namespace tapa

#endif  // !defined(TAPA_TARGET_XILINX_HLS_)
