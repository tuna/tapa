// Self-implemented arbitrary-width integers, tapa::u<W> (unsigned) and
// tapa::i<W> (signed), for vendor-agnostic CPU simulation. Semantics mirror
// the vendor arbitrary-precision integers this family replaces:
//   * mixed arithmetic result widths are widened so exact values fit
//     (rtype below); narrowing assignments truncate the bit pattern;
//   * comparisons follow C's usual conversions with the width standing in
//     for the rank (mixed equal-width compares go unsigned, as in C);
//     equality compares the sign/zero-extended bit patterns at max(W, 64);
//   * shifting by a negative amount shifts the other way; shift counts
//     truncate to 32 bits, matching the vendor's 32-bit count accessors;
//   * `if (x)` tests the FULL width (the vendor truncates its contextual
//     bool conversion to 64 bits while its operator! is full-width; the
//     consistent side is deliberate, not an oversight);
//   * division truncates toward zero; the remainder keeps the dividend's sign.
// On synthesis targets tapa::u/i alias the vendor types instead
// (tapa/xilinx/hls/int.h).

#pragma once

// The self-implemented layer. The tapa.h umbrella selects exactly one
// integer layer per target; under the Xilinx target this file is inert
// (tapa/xilinx/hls/int.h aliases the vendor types instead), so both
// layers being REACHED through the umbrella is fine — only this layer
// defines tapa::u/i.
#if !defined(TAPA_TARGET_XILINX_HLS_)

#include <cassert>
#include <climits>
#include <cmath>
#include <cstdint>
#include <istream>
#include <ostream>
#include <string>
#include <type_traits>
#include <utility>

namespace tapa {

template <int W, bool S>
class int_base;
template <int W>
class u;
template <int W>
class i;
template <int W, bool S>
class range_ref;
template <int W, bool S>
class bit_ref;
template <int W1, bool S1, int W2, bool S2>
class concat_ref;

// Result types of mixed-width arithmetic, per vendor semantics: the width
// rules widen so the exact result always fits; shifts keep the left width.
template <int W1, bool S1, int W2, bool S2>
struct rtype {
  static constexpr int kAdjustedW1 = W1 + (S2 && !S1 ? 1 : 0);
  static constexpr int kAdjustedW2 = W2 + (S1 && !S2 ? 1 : 0);
  static constexpr int kPlusW =
      kAdjustedW1 > kAdjustedW2 ? kAdjustedW1 : kAdjustedW2;
  static constexpr bool kSigned = S1 || S2;
  static constexpr int kModW2 = W2 + (!S2 && S1 ? 1 : 0);
  template <int W, bool S>
  using kind = std::conditional_t<S, i<W>, u<W>>;
  using mult = kind<W1 + W2, kSigned>;
  using plus = kind<kPlusW + 1, kSigned>;
  using minus = kind<kPlusW + 1, true>;
  using div = kind<W1 + S2, kSigned>;
  using mod = kind < W1<kModW2 ? W1 : kModW2, S1>;
  using logic = kind<kPlusW, kSigned>;
  using arg1 = kind<W1, S1>;
  // The common width division and remainder compute at: one bit wider than
  // both operands, so INT_MIN / -1 fits.
  using common = kind<(W1 > W2 ? W1 : W2) + 1, kSigned>;
};

namespace internal {

// The object size AND alignment the vendor gives an arbitrary-precision
// integer: the next power of two at or above ceil(W/8) bytes, for both.
//
// Both are observable. The size is the mmap element stride and the stream
// element size; the alignment sets where the value lands inside any struct
// it is a member of, which is how a packet type built from these ends up
// the wrong length. Matching only the size is not enough -- ap_uint<288>
// is 64 bytes because 36 rounds up to 64, not because its limbs need 64.
constexpr int object_bytes(int w) {
  int bytes = 1;
  while (bytes * CHAR_BIT < w) bytes *= 2;
  return bytes;
}

// Little-endian 64-bit limbs holding the two's-complement bit pattern; bits
// at or above W are kept zero. Signed values sign-extend on demand.
//
// Only the low `count` limbs are ever read or written; the rest is the
// vendor's padding and stays zero.
template <int W>
struct wide_limbs {
  static_assert(W > 64, "wide limbs are for widths above 64");
  static constexpr int count = (W + 63) / 64;
  static constexpr int bytes = object_bytes(W);
  static constexpr int stored = bytes / 8;
  static_assert(stored >= count, "padding cannot be narrower than the value");
  static constexpr uint64_t top_mask =
      W % 64 == 0 ? ~uint64_t(0) : (uint64_t(1) << (W % 64)) - 1;

  void trim() { v[count - 1] &= top_mask; }

  uint64_t& operator[](int i) { return v[i]; }
  uint64_t operator[](int i) const { return v[i]; }

  alignas(bytes) uint64_t v[stored] = {};
};

// Up to 64 bits the smallest native integer already has the vendor's size
// and alignment: 1/2/4/8 bytes is exactly object_bytes(W) there.
template <int W>
struct narrow_limbs {
  static_assert(W > 0 && W <= 64, "narrow limbs are for widths up to 64");
  using native = typename std::conditional<
      W <= 8, uint8_t,
      typename std::conditional<
          W <= 16, uint16_t,
          typename std::conditional<W <= 32, uint32_t, uint64_t>::type>::type>::
      type;
  static constexpr int count = 1;
  static constexpr uint64_t top_mask =
      W == 64 ? ~uint64_t(0) : (uint64_t(1) << (W % 64)) - 1;

  void trim() { v = native(v & native(top_mask)); }

  // The storage member stays addressable through the same [0] surface the
  // wide form offers; only index 0 exists.
  struct ref {
    narrow_limbs& s;
    operator uint64_t() const { return s.v; }
    ref& operator=(uint64_t x) {
      s.v = static_cast<native>(x);
      return *this;
    }
    ref& operator|=(uint64_t x) {
      s.v = static_cast<native>(s.v | x);
      return *this;
    }
    ref& operator&=(uint64_t x) {
      s.v = static_cast<native>(s.v & x);
      return *this;
    }
    ref& operator+=(uint64_t x) {
      s.v = static_cast<native>(s.v + x);
      return *this;
    }
    ref& operator-=(uint64_t x) {
      s.v = static_cast<native>(s.v - x);
      return *this;
    }
    ref& operator++() {
      ++s.v;
      return *this;
    }
    uint64_t operator~() const { return ~uint64_t(s.v); }
  };
  ref operator[](int) { return {*this}; }
  uint64_t operator[](int) const { return v; }

  native v = 0;
};

template <int W>
using limbs =
    typename std::conditional<(W > 64), wide_limbs<W>, narrow_limbs<W>>::type;

// Marker for the tapa integer family (int_base and its u/i derivations):
// excludes them from the duck-typed vendor bridge below.
// Classic member-detection idiom (no void_t: some TUs compile without it).
template <typename T>
struct has_tapa_int_marker {
  template <typename U>
  static char (&probe(typename U::tapa_int_base*))[1];
  template <typename U>
  static char (&probe(...))[2];
  static constexpr bool value = sizeof(probe<T>(nullptr)) == 1;
};

// Detects the vendor integer's bit accessors. ap_int_base exposes
// length()/test()/sign(), so a source wider than 64 bits can be copied
// whole rather than funnelled through the 64-bit to_uint64().
template <typename T>
struct has_vendor_bits {
  template <typename U>
  static char (&probe(typename std::remove_reference<
                      decltype(std::declval<const U&>().test(0),
                               std::declval<const U&>().length(),
                               std::declval<const U&>().sign())>::type*))[1];
  template <typename U>
  static char (&probe(...))[2];
  static constexpr bool value = sizeof(probe<T>(nullptr)) == 1;
};

template <typename T>
struct is_int_base : std::false_type {};
template <int W, bool S>
struct is_int_base<int_base<W, S>> : std::true_type {};

// __int128 is an extension type: is_integral is false for it, yet it must
// mix like any builtin integer (native arithmetic would silently lose the
// high limbs).
template <typename T>
struct is_builtin_int
    : std::integral_constant<bool, std::is_integral<T>::value
#ifdef __SIZEOF_INT128__
                                       || std::is_same<T, __int128>::value ||
                                       std::is_same<T, unsigned __int128>::value
#endif
                             > {
};

template <typename T>
using enable_if_integral =
    typename std::enable_if<is_builtin_int<T>::value &&
                            !is_int_base<T>::value>::type;
template <typename T>
using enable_if_floating =
    typename std::enable_if<std::is_floating_point<T>::value>::type;

// Any builtin integral's tapa counterpart. The vendor widens bool as a
// 1-bit unsigned value, not an 8-bit one.
template <typename T>
struct to_base {
  using type = typename std::conditional<
      std::is_same<T, bool>::value, u<1>,
      typename std::conditional<
          std::is_signed<T>::value, i<static_cast<int>(8 * sizeof(T))>,
          u<static_cast<int>(8 * sizeof(T))>>::type>::type;
};

// Limb-array arithmetic; all operands and results have N limbs.
template <int N>
inline void add_limbs(const uint64_t (&a)[N], const uint64_t (&b)[N],
                      uint64_t (&out)[N]) {
  uint64_t carry = 0;
  for (int li = 0; li < N; ++li) {
    const uint64_t t = a[li] + b[li];
    const uint64_t c1 = t < a[li] ? 1 : 0;
    out[li] = t + carry;
    carry = c1 | (out[li] < t ? 1 : 0);
  }
}

template <int N>
inline void sub_limbs(const uint64_t (&a)[N], const uint64_t (&b)[N],
                      uint64_t (&out)[N]) {
  uint64_t borrow = 0;
  for (int li = 0; li < N; ++li) {
    // Two-step: b[li] + borrow can wrap at UINT64_MAX, losing the borrow.
    const uint64_t t = a[li] - b[li];
    const uint64_t b1 = a[li] < b[li] ? 1 : 0;
    out[li] = t - borrow;
    borrow = b1 | (t < borrow ? 1 : 0);
  }
}

template <int N>
inline void mul_limbs(const uint64_t (&a)[N], const uint64_t (&b)[N],
                      uint64_t (&out)[N]) {
  for (int li = 0; li < N; ++li) out[li] = 0;
  for (int bi = 0; bi < N; ++bi) {
    if (b[bi] == 0) continue;
    uint64_t carry = 0;
    for (int ai = 0; ai + bi < N; ++ai) {
#ifdef __SIZEOF_INT128__
      const unsigned __int128 cur =
          static_cast<unsigned __int128>(a[ai]) * b[bi] + out[ai + bi] + carry;
      out[ai + bi] = static_cast<uint64_t>(cur);
      carry = static_cast<uint64_t>(cur >> 64);
#else
      // Same 64x64 -> 128 product in 32-bit halves, for toolchains without
      // the extension. Every other use of __int128 in this header is
      // guarded, so this one must be too or the header fails to compile the
      // moment operator* is instantiated.
      const uint64_t lo_mask = 0xffffffffULL;
      const uint64_t al = a[ai] & lo_mask, ah = a[ai] >> 32;
      const uint64_t bl = b[bi] & lo_mask, bh = b[bi] >> 32;
      const uint64_t ll = al * bl, lh = al * bh, hl = ah * bl, hh = ah * bh;
      uint64_t mid = lh + hl;
      uint64_t hi = hh + (mid < lh ? (1ULL << 32) : 0) + (mid >> 32);
      uint64_t lo = ll + (mid << 32);
      if (lo < ll) ++hi;
      uint64_t sum = lo + out[ai + bi];
      if (sum < lo) ++hi;
      uint64_t total = sum + carry;
      if (total < sum) ++hi;
      out[ai + bi] = total;
      carry = hi;
#endif
    }
  }
}

template <int N>
inline void and_limbs(const uint64_t (&a)[N], const uint64_t (&b)[N],
                      uint64_t (&out)[N]) {
  for (int li = 0; li < N; ++li) out[li] = a[li] & b[li];
}
template <int N>
inline void or_limbs(const uint64_t (&a)[N], const uint64_t (&b)[N],
                     uint64_t (&out)[N]) {
  for (int li = 0; li < N; ++li) out[li] = a[li] | b[li];
}
template <int N>
inline void xor_limbs(const uint64_t (&a)[N], const uint64_t (&b)[N],
                      uint64_t (&out)[N]) {
  for (int li = 0; li < N; ++li) out[li] = a[li] ^ b[li];
}

}  // namespace internal

template <int W, bool S>
class int_base {
 public:
  using tapa_int_base = void;  // marker: see internal::has_tapa_int_marker
  static constexpr int width = W;
  static constexpr bool sign_flag = S;
  using storage = internal::limbs<W>;

  // The free division/remainder operators call the private divide() helper.
  template <int W1, bool S1, int W2, bool S2>
  friend inline typename rtype<W1, S1, W2, S2>::div operator/(
      const int_base<W1, S1>& a, const int_base<W2, S2>& b);
  template <int W1, bool S1, int W2, bool S2>
  friend inline typename rtype<W1, S1, W2, S2>::mod operator%(
      const int_base<W1, S1>& a, const int_base<W2, S2>& b);

  int_base() = default;

  template <typename T, typename = typename std::enable_if<
                            std::is_integral<T>::value>::type>
  int_base(T x) {
    set_int(x);
  }
  int_base(float x) { set_flt(x); }
  int_base(double x) { set_flt(x); }
  // Duck-typed bridge from vendor arbitrary-precision types and their slice
  // references (anything exposing to_uint64() and not already a tapa or
  // builtin type), so programs mixing kept vendor types (e.g. AXIS packet
  // payloads) interoperate without tapa including vendor headers.
  template <
      typename T,
      typename std::enable_if<
          !std::is_integral<T>::value && !std::is_floating_point<T>::value &&
              !internal::has_tapa_int_marker<T>::value,
          decltype(std::declval<const T&>().to_uint64())>::type* = nullptr>
  int_base(const T& x) {
    if constexpr (internal::has_vendor_bits<T>::value) {
      // to_uint64() is a 64-bit accessor, so an ap_uint<512> source would
      // otherwise arrive with everything above bit 63 dropped — and under
      // the Xilinx target, where tapa::u IS ap_uint, the same line is an
      // identity. Copy the bits instead.
      const int src_w = static_cast<int>(x.length());
      if (src_w > 64) {
        const int n = src_w < W ? src_w : W;
        for (int b = 0; b < n; ++b) set_bit(b, x.test(b));
        const bool fill = x.sign();  // false for unsigned vendor types
        for (int b = n; b < W; ++b) set_bit(b, fill);
        return;
      }
      // A signed source no wider than 64 bits must still sign-extend into
      // a wider tapa target: to_uint64() carries the value's low 64 bits
      // (sign-extended), but the limbs above them would stay zero.
      set_int(x.to_uint64());
      if (x.sign()) {
        for (int b = 64; b < W; ++b) set_bit(b, true);
      }
      return;
    }
    set_int(x.to_uint64());
  }
#ifdef __SIZEOF_INT128__
  int_base(__int128 x) { set_wide(x); }
  int_base(unsigned __int128 x) { set_wide(x); }
#endif

  // Truncating on narrowing, value-preserving on widening.
  template <int W2, bool S2>
  int_base(const int_base<W2, S2>& x) {
    if constexpr (W2 < W) {
      uint64_t buf[storage::count] = {};
      x.template extend_to<storage::count>(buf);  // sign/zero-extended
      for (int li = 0; li < storage::count; ++li) v_[li] = buf[li];
    } else {
      for (int li = 0; li < storage::count; ++li) v_[li] = x.raw()[li];
    }
    v_.trim();  // narrowing truncates
  }
  template <int W2, bool S2>
  int_base(const range_ref<W2, S2>& x);
  template <int W2, bool S2>
  int_base(const bit_ref<W2, S2>& x);
  template <int W1, bool S1, int W2, bool S2>
  int_base(const concat_ref<W1, S1, W2, S2>& x);

  int_base& operator=(const int_base&) = default;
  template <int W2, bool S2>
  int_base& operator=(const int_base<W2, S2>& x) {
    return *this = int_base(x);
  }

  // ── Inspectors ──────────────────────────────────────────────────────
  int length() const { return W; }

  bool get_bit(int idx) const {
    assert(idx >= 0 && idx < W);
    return (v_[idx >> 6] >> (idx & 63)) & 1;
  }

  int_base& set_bit(int idx, bool value) {
    assert(idx >= 0 && idx < W);
    if (value) {
      v_[idx >> 6] |= uint64_t(1) << (idx & 63);
    } else {
      v_[idx >> 6] &= ~(uint64_t(1) << (idx & 63));
    }
    return *this;
  }

  // Bit rotations within W (vendor semantics: positions wrap mod W).
  int_base& lrotate(int n) { return rotate_(W - n); }
  int_base& rrotate(int n) { return rotate_(n); }

  int_base& reverse() {
    storage t = v_;
    for (int b = 0; b < W; ++b)
      set_bit(b, t[(W - 1 - b) >> 6] >> ((W - 1 - b) & 63) & 1);
    return *this;
  }

  // ── Conversions ─────────────────────────────────────────────────────
  // Exactly one implicit integer conversion per signedness (the vendor
  // RetType), so overload resolution never sees two competing conversions.
  using RetType = typename std::conditional<S, int64_t, uint64_t>::type;
  operator RetType() const {  // NOLINT(google-explicit-constructor)
    if constexpr (S && W < 64) {
      return static_cast<int64_t>(v_[0] << (64 - W)) >> (64 - W);
    } else {
      return static_cast<RetType>(v_[0]);
    }
  }
  int64_t to_int64() const { return static_cast<int64_t>(operator RetType()); }
  // Sign-extends for signed types, like the vendor's (ap_ulong)(V) on a
  // signed storage member; to_int64() must not disagree with it.
  uint64_t to_uint64() const {
    return static_cast<uint64_t>(operator RetType());
  }
  float to_float() const { return static_cast<float>(to_double()); }
  double to_double() const {
    return S && negative() ? -magnitude().to_double_unsigned()
                           : to_double_unsigned();
  }

  // ── Slice and bit proxies ───────────────────────────────────────────
  // Mutable proxies bind only to lvalues (a proxy into a temporary would
  // dangle); const and rvalue objects read by value.
  range_ref<W, S> operator()(int hi, int lo) & {
    return range_ref<W, S>(*this, hi, lo);
  }
  u<W> operator()(int hi, int lo) const&;  // defined after u/i
  range_ref<W, S> range(int hi, int lo) & { return (*this)(hi, lo); }
  u<W> range(int hi, int lo) const&;  // defined after u/i
  template <int Hi, int Lo>
  u<Hi - Lo + 1> range() const {
    static_assert(Hi < W && Hi >= Lo && Lo >= 0,
                  "range<Hi,Lo>() out of bounds");
    return u<Hi - Lo + 1>((*this)(Hi, Lo));
  }
  bit_ref<W, S> operator[](int idx) & { return bit_ref<W, S>(*this, idx); }
  bool operator[](int idx) const& { return get_bit(idx); }

  // ── Reductions ──────────────────────────────────────────────────────
  bool and_reduce() const {
    for (int li = 0; li + 1 < storage::count; ++li)
      if (v_[li] != ~uint64_t(0)) return false;
    return v_[storage::count - 1] == storage::top_mask;
  }
  bool nand_reduce() const { return !and_reduce(); }
  bool or_reduce() const {
    for (int li = 0; li < storage::count; ++li)
      if (v_[li] != 0) return true;
    return false;
  }
  bool nor_reduce() const { return !or_reduce(); }
  bool xor_reduce() const {
    uint64_t acc = 0;
    for (int li = 0; li < storage::count; ++li) acc ^= v_[li];
    return parity(acc);  // bits >= W are zero by the storage invariant
  }
  bool xnor_reduce() const { return !xor_reduce(); }

  // ── Comparisons ─────────────────────────────────────────────────────
  //
  // The vendor applies C's usual arithmetic conversions with the declared
  // width standing in for the conversion rank, and says so in its own
  // source: "this will follow gcc rule for comparison between different
  // bitwidth and signness". Same signedness compares that way. Mixed
  // signedness compares SIGNED when both operands are narrower than an int,
  // because C promotes both to int there, and otherwise takes the
  // signedness of whichever operand is at least as wide -- so
  // `u<15> < i<10>` is a signed comparison and `u<32> < i<32>` is an
  // unsigned one, exactly as the equivalent C would be.
  template <int W2, bool S2>
  static constexpr bool compare_is_signed() {
    return S == S2               ? S
           : (W < 32 && W2 < 32) ? true
           : S                   ? !(W2 >= W)
                                 : !(W >= W2);
  }

  template <int W2, bool S2>
  int compare(const int_base<W2, S2>& x) const {
    constexpr int n =
        (W > W2 ? W : W2) / 64 + 1;  // one spare limb for the sign
    uint64_t a[n] = {};
    uint64_t b[n] = {};
    extend_to(a);
    x.template extend_to<n>(b);
    if (compare_is_signed<W2, S2>()) {
      const bool an = a[n - 1] >> 63;
      const bool bn = b[n - 1] >> 63;
      if (an != bn) return an ? -1 : 1;
    }
    for (int li = n - 1; li >= 0; --li) {
      if (a[li] != b[li]) return a[li] < b[li] ? -1 : 1;
    }
    return 0;
  }

  // Equality compares bit patterns the way the vendor does: each operand
  // is sign-extended (when signed) or zero-extended (when unsigned) into
  // max(W, W2, 64) bits, and the patterns compare. The 64-bit floor is the
  // vendor's native path: `i<64>(-1) == u<64>(~0)` is true, but
  // `i<32>(-1) == u<32>(0xffffffff)` is false, because the widened
  // patterns (0xff..ff vs 0xffffffff) differ.
  template <int W2, bool S2>
  int compare_values(const int_base<W2, S2>& x) const {
    constexpr int EW = (W > W2 ? W : W2) < 64 ? 64 : (W > W2 ? W : W2);
    constexpr int n = (EW + 63) / 64;
    uint64_t a[n] = {};
    uint64_t b[n] = {};
    extend_to(a);
    x.template extend_to<n>(b);
    if (EW % 64 != 0) {  // mask the extension back to the widened width
      const uint64_t top = (uint64_t(1) << (EW % 64)) - 1;
      a[n - 1] &= top;
      b[n - 1] &= top;
    }
    for (int li = n - 1; li >= 0; --li) {
      if (a[li] != b[li]) return a[li] < b[li] ? -1 : 1;
    }
    return 0;
  }

  template <int W2, bool S2>
  bool operator==(const int_base<W2, S2>& x) const {
    return compare_values(x) == 0;
  }
  template <int W2, bool S2>
  bool operator!=(const int_base<W2, S2>& x) const {
    return compare_values(x) != 0;
  }
  template <int W2, bool S2>
  bool operator<(const int_base<W2, S2>& x) const {
    return compare(x) < 0;
  }
  template <int W2, bool S2>
  bool operator<=(const int_base<W2, S2>& x) const {
    return compare(x) <= 0;
  }
  template <int W2, bool S2>
  bool operator>(const int_base<W2, S2>& x) const {
    return compare(x) > 0;
  }
  template <int W2, bool S2>
  bool operator>=(const int_base<W2, S2>& x) const {
    return compare(x) >= 0;
  }

  // ── Unary ───────────────────────────────────────────────────────────
  int_base operator+() const { return *this; }
  int_base operator~() const {
    int_base r;
    for (int li = 0; li < storage::count; ++li) r.v_[li] = ~v_[li];
    r.v_.trim();
    return r;
  }
  bool operator!() const { return !or_reduce(); }
  explicit operator bool() const { return or_reduce(); }
  // The vendor member form: radix defaults to 2, and sign = false prints
  // the unsigned bit pattern. Defined after tapa::to_string below.
  std::string to_string(int base = 2, bool sign = S) const;
  typename rtype<1, false, W, S>::minus operator-() const {
    typename rtype<1, false, W, S>::minus zero(0);
    return zero - *this;
  }

  // ── Compound assignment (bodies defined after the operators) ────────
  // +,-,*,&,|,^ take this width: two's complement is a ring homomorphism,
  // so converting the RHS here first equals vendor compute-then-truncate.
  // /,% and shifts are templates: their semantics depend on the RHS's own
  // width or signedness.
  int_base& operator+=(int_base x);
  int_base& operator-=(int_base x);
  int_base& operator*=(int_base x);
  int_base& operator&=(int_base x);
  int_base& operator|=(int_base x);
  int_base& operator^=(int_base x);
  template <int W2, bool S2>
  int_base& operator/=(const int_base<W2, S2>& x);
  template <int W2, bool S2>
  int_base& operator%=(const int_base<W2, S2>& x);
  template <int W2, bool S2>
  int_base& operator<<=(const int_base<W2, S2>& x);
  template <int W2, bool S2>
  int_base& operator>>=(const int_base<W2, S2>& x);
  // Builtin counts (e.g. x <<= 3) route through the matching tapa type.
  template <typename T, typename = internal::enable_if_integral<T>>
  int_base& operator<<=(T x) {
    return *this <<= typename internal::to_base<T>::type(x);
  }
  template <typename T, typename = internal::enable_if_integral<T>>
  int_base& operator>>=(T x) {
    return *this >>= typename internal::to_base<T>::type(x);
  }
  template <typename T, typename = internal::enable_if_integral<T>>
  int_base& operator/=(T x) {
    return *this /= typename internal::to_base<T>::type(x);
  }
  template <typename T, typename = internal::enable_if_integral<T>>
  int_base& operator%=(T x) {
    return *this %= typename internal::to_base<T>::type(x);
  }

  int_base& operator++() { return *this += int_base(1); }
  int_base operator++(int) {
    int_base t = *this;
    ++*this;
    return t;
  }
  int_base& operator--() { return *this -= int_base(1); }
  int_base operator--(int) {
    int_base t = *this;
    --*this;
    return t;
  }

  // The absolute value (as an unsigned-pattern copy; public: free
  // operators and to_string build on it).
  int_base magnitude() const {
    if (!S || !negative()) return *this;
    int_base t = *this;
    t.negate();
    return t;
  }

  // ── Storage access for the operators and the proxies ────────────────
  const storage& raw() const { return v_; }
  storage& raw() { return v_; }
  bool negative() const { return get_bit(W - 1); }
  bool is_zero() const { return !or_reduce(); }

  // The value's magnitude as a shift count: unsigned counts never
  // reverse (only a signed operand's sign bit can), and counts whose
  // magnitude does not fit below the width saturate, since any count
  // >= W produces the same saturated shift result.
  // The vendor reads shift counts through its 32-bit accessors, so the
  // count's magnitude truncates to 32 bits before the shift: a count of
  // 2**32 shifts by 0, not by "a lot". Saturation past the width still
  // applies after the truncation.
  uint32_t shift_count() const {
    const int_base m = S && negative() ? magnitude() : *this;
    return static_cast<uint32_t>(m.v_[0]);
  }

  // Zero- or sign-extended copy into n limbs; n >= storage::count.
  template <int N>
  void extend_to(uint64_t (&out)[N]) const {
    static_assert(N >= storage::count, "extension target too narrow");
    for (int li = 0; li < N; ++li) out[li] = li < storage::count ? v_[li] : 0;
    if (S && negative()) {
      if constexpr (W % 64 != 0) out[W / 64] |= ~uint64_t(0) << (W % 64);
      for (int li = (W + 63) / 64; li < N; ++li) out[li] = ~uint64_t(0);
    }
  }

 private:
  storage v_;

  template <typename T>
  void set_int(T x) {
    v_[0] = static_cast<uint64_t>(x);
    const bool neg = std::is_signed<T>::value && x < 0;
    for (int li = 1; li < storage::count; ++li) v_[li] = neg ? ~uint64_t(0) : 0;
    v_.trim();
  }

#ifdef __SIZEOF_INT128__
  template <typename Wide>
  void set_wide(Wide x) {
    static_assert(sizeof(Wide) == 16, "wide ctor is for __int128");
    const unsigned __int128 bits = static_cast<unsigned __int128>(x);
    v_[0] = static_cast<uint64_t>(bits);
    if (storage::count > 1) v_[1] = static_cast<uint64_t>(bits >> 64);
    for (int li = 2; li < storage::count; ++li)
      v_[li] = x < 0 ? ~uint64_t(0) : 0;
    v_.trim();
  }
#endif

  template <typename T>
  void set_flt(T x) {  // truncation toward zero, at any width
    const bool neg = std::isfinite(x) && x < 0;
    long double m =
        neg ? -static_cast<long double>(x) : static_cast<long double>(x);
    if (!std::isfinite(x)) m = 0;
    // Decompose the magnitude limb by limb; fmod is exact, so the pattern
    // below 2^(64*count) lands exactly and the final mask truncates the
    // rest, matching the vendor's modulo-W construction.
    for (int li = 0; li < storage::count; ++li) {
      v_[li] = static_cast<uint64_t>(std::fmod(m, 18446744073709551616.0L));
      m = std::floor(m / 18446744073709551616.0L);
    }
    if (neg) {
      negate();
    } else {
      v_.trim();
    }
  }

  u<W> read_range(int hi, int lo) const;  // defined after u/i

  int_base& rotate_(int n) {
    const int m = ((n % W) + W) % W;
    const storage t = v_;
    for (int b = 0; b < W; ++b) {
      const int src = (b + m) % W;
      set_bit(b, t[src >> 6] >> (src & 63) & 1);
    }
    return *this;
  }

  static bool parity(uint64_t x) {
    x ^= x >> 32;
    x ^= x >> 16;
    x ^= x >> 8;
    x ^= x >> 4;
    x ^= x >> 2;
    x ^= x >> 1;
    return x & 1;
  }

  double to_double_unsigned() const {
    double d = 0;
    for (int li = storage::count - 1; li >= 0; --li)
      d = d * 18446744073709551616.0 + v_[li];
    return d;
  }
  void negate() {
    for (int li = 0; li < storage::count; ++li) v_[li] = ~uint64_t(v_[li]);
    uint64_t carry = 1;
    for (int li = 0; li < storage::count && carry != 0; ++li) {
      ++v_[li];
      carry = v_[li] == 0 ? 1 : 0;
    }
    v_.trim();
  }

  void shl_storage(uint64_t sh) {
    if (sh >= uint64_t(W)) {
      *this = int_base(0);
      return;
    }
    storage out{};
    const int ls = static_cast<int>(sh >> 6);
    const int bs = static_cast<int>(sh & 63);
    for (int li = storage::count - 1; li >= ls; --li) {
      uint64_t x = v_[li - ls] << bs;
      if (bs != 0 && li - ls >= 1) x |= v_[li - ls - 1] >> (64 - bs);
      out[li] = x;
    }
    v_ = out;
    v_.trim();
  }
  void shr_storage(uint64_t sh) {
    if (sh >= uint64_t(W)) {
      *this = S && negative() ? int_base(-1) : int_base(0);
      return;
    }
    storage out{};
    const int ls = static_cast<int>(sh >> 6);
    const int bs = static_cast<int>(sh & 63);
    for (int li = 0; li < storage::count; ++li) {  // logical limb shift
      uint64_t x = 0;
      if (li + ls < storage::count) x = v_[li + ls] >> bs;
      if (bs != 0) {
        const int hi = li + ls + 1;
        // Both arms spelled uint64_t: for narrow storage `v_[hi]` is a
        // proxy and `0` is an int, so the conditional's composite type
        // would be int and `<< (64 - bs)` would shift past its width —
        // undefined, and -fsanitize=shift aborts on it.
        const uint64_t hi_limb =
            hi < storage::count ? static_cast<uint64_t>(v_[hi]) : uint64_t(0);
        x |= hi_limb << (64 - bs);
      }
      out[li] = x;
    }
    if (S && negative()) {  // arithmetic: fill the sh vacated top bits
      for (int b = W - 1; b >= W - static_cast<int>(sh); --b)
        out[b >> 6] |= uint64_t(1) << (b & 63);
    }
    v_ = out;
  }

  // C-semantics division at this width: both operands must already be
  // converted here (value-preserving) by the free operators.
  int_base divide(const int_base& d, int_base* rem_out) const {
    int_base q;
    int_base r;
    if (!d.is_zero()) {
      const int_base a = magnitude();
      const int_base b = d.magnitude();
      for (int bit = W - 1; bit >= 0; --bit) {
        r.shl_storage(1);
        if (a.get_bit(bit)) r.set_bit(0, true);
        if (geq(r, b)) {
          r.subtract(b);
          q.set_bit(bit, true);
        }
      }
      if (negative() != d.negative()) q.negate();
      if (rem_out != nullptr) {
        if (negative()) r.negate();
        *rem_out = r;
      }
    } else if (rem_out != nullptr) {
      *rem_out = r;
    }
    return q;
  }

  // Both operands non-negative here: plain unsigned limb compare.
  static bool geq(const int_base& a, const int_base& b) {
    for (int li = storage::count - 1; li >= 0; --li) {
      if (a.v_[li] != b.v_[li]) return a.v_[li] > b.v_[li];
    }
    return true;
  }
  void subtract(const int_base& b) {
    uint64_t borrow = 0;
    for (int li = 0; li < storage::count; ++li) {
      const uint64_t t = v_[li] - b.v_[li];
      const uint64_t b1 = v_[li] < b.v_[li] ? 1 : 0;
      v_[li] = t - borrow;
      borrow = b1 | (t < borrow ? 1 : 0);
    }
  }
};

/// Unsigned arbitrary-width integer: tapa::u<32> is a 32-bit unsigned value.
template <int W>
class u : public int_base<W, false> {
 public:
  using int_base<W, false>::int_base;

  // Explicitly forwarded from any tapa::u/i: relying on the inherited
  // constructor here loses to the RetType user conversion in overload
  // resolution for some argument forms, silently truncating to 64 bits.
  template <int W2, bool S2>
  u(const int_base<W2, S2>& x) : int_base<W, false>(x) {}
};

/// Signed arbitrary-width integer: tapa::i<32> is a 32-bit signed value.
template <int W>
class i : public int_base<W, true> {
 public:
  using int_base<W, true>::int_base;

  template <int W2, bool S2>
  i(const int_base<W2, S2>& x) : int_base<W, true>(x) {}
};

// Slice reads by value (u<64> is not a dependent type, so these are
// defined here, where it is complete).
template <int W, bool S>
inline u<W> int_base<W, S>::read_range(int hi, int lo) const {
  u<W> v(0);
  for (int b = lo; b <= hi; ++b) v.set_bit(b - lo, get_bit(b));
  return v;
}
template <int W, bool S>
inline u<W> int_base<W, S>::operator()(int hi, int lo) const& {
  return read_range(hi, lo);
}
template <int W, bool S>
inline u<W> int_base<W, S>::range(int hi, int lo) const& {
  return read_range(hi, lo);
}

/// Text form of @p x in @p base (2, 8, 10, or 16). Matches the vendor
/// ostream output: non-decimal bases carry the radix prefix (0b/0o/0x,
/// also on zero), and a negative signed value prints as '-' + prefix +
/// magnitude. (The vendor reads std::cout's flags instead of the target
/// stream's; that bug is deliberately not reproduced — the parity claim
/// covers the format, not that.)
template <int W, bool S>
inline std::string to_string(const int_base<W, S>& x, int base = 10) {
  switch (base) {
    case 10:
      break;
    case 16:
      if (x.is_zero()) return "0x0";
      break;
    case 8:
      if (x.is_zero()) return "0o0";
      break;
    case 2:
      if (x.is_zero()) return "0b0";
      break;
    default:
      assert(false && "to_string base must be 2, 8, 10, or 16");
      base = 10;
  }
  if (x.is_zero()) return "0";
  std::string digits;
  if (S && x.negative()) digits += '-';
  // The magnitude's bit pattern must print unsigned, or RetType re-sign-
  // extends its top bit.
  u<W> mag(x.magnitude());
  if (base == 10) {
    if (W <= 64) return digits + std::to_string(static_cast<uint64_t>(mag));
    const uint64_t chunk = 1000000000000000000ULL;
    std::string parts;
    while (!mag.is_zero()) {
      uint64_t rem = mag % chunk;
      mag /= chunk;
      for (int i = 0; i < 18; ++i) {
        parts += char('0' + rem % 10);
        rem /= 10;
      }
    }
    while (parts.size() > 1 && parts.back() == '0') parts.pop_back();
    return digits + std::string(parts.rbegin(), parts.rend());
  }
  digits += base == 16 ? "0x" : base == 8 ? "0o" : "0b";
  const int bits = base == 16 ? 4 : base == 8 ? 3 : 1;
  const char* alphabet = "0123456789abcdef";
  std::string parts;
  for (int b = 0; b < W; b += bits) {
    unsigned digit = 0;
    for (int k = bits - 1; k >= 0; --k) {
      const int idx = b + k;
      digit = digit * 2 + (idx < W && mag.get_bit(idx) ? 1 : 0);
    }
    parts += alphabet[digit];
  }
  while (parts.size() > 1 && parts.back() == '0') parts.pop_back();
  digits += std::string(parts.rbegin(), parts.rend());
  return digits;
}

template <int W, bool S>
inline std::ostream& operator<<(std::ostream& os, const int_base<W, S>& x) {
  const std::ios_base::fmtflags ff = os.flags();
  if (ff & std::ios_base::hex) return os << to_string(x, 16);
  if (ff & std::ios_base::oct) return os << to_string(x, 8);
  return os << to_string(x);
}

template <int W, bool S>
inline std::string int_base<W, S>::to_string(int base, bool sign) const {
  return sign ? tapa::to_string(*this, base)
              : tapa::to_string(u<W>(*this), base);
}

/// Reads one token into @p x: the stream's basefield selects the radix,
/// with 0x/0o/0b prefixes overriding it, like the vendor's operator>>.
/// Values truncate to W bits; a leading '-' negates.
template <int W, bool S>
inline std::istream& operator>>(std::istream& in, int_base<W, S>& x) {
  std::string str;
  in >> str;
  const std::ios_base::fmtflags basefield =
      in.flags() & std::ios_base::basefield;
  int base = basefield == std::ios_base::oct   ? 8
             : basefield == std::ios_base::hex ? 16
                                               : 10;
  size_t pos = 0;
  bool neg = false;
  if (pos < str.size() && str[pos] == '-') {
    neg = true;
    ++pos;
  }
  if (pos + 1 < str.size() && str[pos] == '0') {
    const char c = str[pos + 1];
    if (c == 'x' || c == 'X') {
      base = 16;
      pos += 2;
    } else if (c == 'b' || c == 'B') {
      base = 2;
      pos += 2;
    } else if (c == 'o' || c == 'O') {
      base = 8;
      pos += 2;
    }
  }
  u<W> v(0);
  bool any = false;
  for (; pos < str.size(); ++pos) {
    const char c = str[pos];
    int digit;
    if (c >= '0' && c <= '9') {
      digit = c - '0';
    } else if (c >= 'a' && c <= 'f') {
      digit = c - 'a' + 10;
    } else if (c >= 'A' && c <= 'F') {
      digit = c - 'A' + 10;
    } else {
      break;
    }
    if (digit >= base) break;
    v = v * u<W>(base) + u<W>(digit);
    any = true;
  }
  if (!any) in.setstate(std::ios_base::failbit);
  x = int_base<W, S>(v);
  if (neg) x = -x;  // widened by one bit, then truncated back to W
  return in;
}

// ── Slice, bit and concatenation proxies ─────────────────────────────────

// Bits [lo, hi] of an int_base, as an lvalue. A read yields the selected
// bits right-aligned in the parent width, matching the vendor range
// reference (which converts to ap_int_base<parent width, false>, not to a
// 64-bit value); writes cover every selected bit, clearing (or
// sign-extending) what the source does not fill.
template <int W, bool S>
class range_ref {
 public:
  range_ref(int_base<W, S>& ref, int hi, int lo) : ref_(ref), hi_(hi), lo_(lo) {
    assert(hi >= lo && lo >= 0 && hi < W);
  }

  operator uint64_t() const {
    return static_cast<uint64_t>(value());
  }  // NOLINT
  template <int W2, bool S2>
  operator int_base<W2, S2>() const {  // NOLINT
    return int_base<W2, S2>(value());
  }

  // The low hi-lo+1 bits of @p bits land in bits [lo, hi]; bits above the
  // 64-bit source are cleared.
  range_ref& operator=(uint64_t bits) {
    for (int off = 0; off <= hi_ - lo_; ++off) {
      ref_.set_bit(lo_ + off, off < 64 ? ((bits >> off) & 1) != 0 : false);
    }
    return *this;
  }
  template <int W2, bool S2>
  range_ref& operator=(const int_base<W2, S2>& x) {
    for (int off = 0; off <= hi_ - lo_; ++off) {
      // Beyond the source width: zero-extend unsigned, sign-extend signed.
      const bool bit = off >= W2 ? (S2 && x.negative()) : x.get_bit(off);
      ref_.set_bit(lo_ + off, bit);
    }
    return *this;
  }
  // Both spellings of a proxy-to-proxy copy go through the full-width
  // value(), never through uint64_t: `a(255, 0) = b(255, 0)` must copy
  // all 256 bits, as the vendor's ap_range_ref does.
  range_ref& operator=(const range_ref& o) { return *this = o.value(); }
  template <int W2, bool S2>
  range_ref& operator=(const range_ref<W2, S2>& o) {
    return *this = o.value();
  }
  template <int W2, bool S2>
  range_ref& operator|=(const int_base<W2, S2>& x) {
    return *this = value() | x;
  }
  template <int W2, bool S2>
  range_ref& operator&=(const int_base<W2, S2>& x) {
    return *this = value() & x;
  }

  u<W> value() const {
    u<W> v(0);
    for (int b = lo_; b <= hi_; ++b) v.set_bit(b - lo_, ref_.get_bit(b));
    return v;
  }

 private:
  int_base<W, S>& ref_;
  int hi_;
  int lo_;
};

// One bit of an int_base, as an lvalue.
template <int W, bool S>
class bit_ref {
 public:
  bit_ref(int_base<W, S>& ref, int idx) : ref_(ref), idx_(idx) {
    assert(idx >= 0 && idx < W);
  }

  operator bool() const { return ref_.get_bit(idx_); }  // NOLINT
  bit_ref& operator=(uint64_t value) {
    ref_.set_bit(idx_, (value & 1) != 0);
    return *this;
  }
  bit_ref& operator|=(bool b) { return *this = ref_.get_bit(idx_) | b; }
  bit_ref& operator&=(bool b) { return *this = ref_.get_bit(idx_) & b; }
  bit_ref& operator^=(bool b) { return *this = ref_.get_bit(idx_) ^ b; }
  template <int W2, bool S2>
  bit_ref& operator=(const int_base<W2, S2>& x) {
    // The vendor's ap_bit_ref assigns `val != 0`, so an even non-zero
    // source such as a 2-bit mask sets the bit; taking bit 0 would clear
    // it and diverge from synthesis.
    ref_.set_bit(idx_, !x.is_zero());
    return *this;
  }
  bit_ref& operator=(const bit_ref& o) {
    ref_.set_bit(idx_, static_cast<bool>(o));
    return *this;
  }

 private:
  int_base<W, S>& ref_;
  int idx_;
};

template <int W1, bool S1, int W2, bool S2>
inline u<W1 + W2> concat_value(const int_base<W1, S1>& hi,
                               const int_base<W2, S2>& lo);

// hi_ bits over lo_ bits, as an lvalue of width W1 + W2.
template <int W1, bool S1, int W2, bool S2>
class concat_ref {
 public:
  concat_ref(int_base<W1, S1>& hi, int_base<W2, S2>& lo) : hi_(hi), lo_(lo) {}

  u<W1 + W2> value() const { return concat_value(hi_, lo_); }

  concat_ref& operator=(uint64_t bits) {
    for (int d = 0; d < W1 + W2; ++d) {
      const bool bit = d < 64 ? ((bits >> d) & 1) != 0 : false;
      if (d < W2) {
        lo_.set_bit(d, bit);
      } else {
        hi_.set_bit(d - W2, bit);
      }
    }
    return *this;
  }
  template <int W3, bool S3>
  concat_ref& operator=(const int_base<W3, S3>& x) {
    // Via a temporary of the full width: bounds-safe extension of any
    // source (the vendor semantic: zero/sign-extend into the halves),
    // so every bit past the source width is already the extension.
    const u<W1 + W2> tmp(x);
    for (int d = 0; d < W1 + W2; ++d) {
      const bool bit = tmp.get_bit(d);
      if (d < W2) {
        lo_.set_bit(d, bit);
      } else {
        hi_.set_bit(d - W2, bit);
      }
    }
    return *this;
  }
  concat_ref& operator=(const concat_ref& o) { return *this = o.value(); }

 private:
  int_base<W1, S1>& hi_;
  int_base<W2, S2>& lo_;
};

// Proxy-taking constructors (defined now that the proxies are complete).
// Slice expressions evaluate at the parent's full width, like the vendor's
// ap_range_ref. Without these overloads, template argument deduction
// ignores the implicit conversions and overload resolution lands on the
// BUILTIN operators through uint64_t: `a(255,0) == b(255,0)` would compare
// only the low 64 bits of each slice.
#define TAPA_RANGE_REF_OP(op)                                                  \
  template <int W, bool S, int W2, bool S2>                                    \
  inline auto operator op(const range_ref<W, S>& a,                            \
                          const range_ref<W2, S2>& b)                          \
      ->decltype(a.value() op b.value()) {                                     \
    return a.value() op b.value();                                             \
  }                                                                            \
  template <int W, bool S, int W2, bool S2>                                    \
  inline auto operator op(const range_ref<W, S>& a, const int_base<W2, S2>& b) \
      ->decltype(a.value() op b) {                                             \
    return a.value() op b;                                                     \
  }                                                                            \
  template <int W, bool S, int W2, bool S2>                                    \
  inline auto operator op(const int_base<W, S>& a, const range_ref<W2, S2>& b) \
      ->decltype(a op b.value()) {                                             \
    return a op b.value();                                                     \
  }                                                                            \
  template <int W, bool S, typename T,                                         \
            typename = internal::enable_if_integral<T>>                        \
  inline auto operator op(const range_ref<W, S>& a, T b)                       \
      ->decltype(a.value() op b) {                                             \
    return a.value() op b;                                                     \
  }                                                                            \
  template <int W, bool S, typename T,                                         \
            typename = internal::enable_if_integral<T>>                        \
  inline auto operator op(T a, const range_ref<W, S>& b)                       \
      ->decltype(a op b.value()) {                                             \
    return a op b.value();                                                     \
  }

TAPA_RANGE_REF_OP(==)
TAPA_RANGE_REF_OP(!=)
TAPA_RANGE_REF_OP(<)
TAPA_RANGE_REF_OP(<=)
TAPA_RANGE_REF_OP(>)
TAPA_RANGE_REF_OP(>=)
TAPA_RANGE_REF_OP(+)
TAPA_RANGE_REF_OP(-)
TAPA_RANGE_REF_OP(*)
TAPA_RANGE_REF_OP(/)
TAPA_RANGE_REF_OP(%)
TAPA_RANGE_REF_OP(&)
TAPA_RANGE_REF_OP(|)
TAPA_RANGE_REF_OP(^)
TAPA_RANGE_REF_OP(<<)
TAPA_RANGE_REF_OP(>>)
#undef TAPA_RANGE_REF_OP

template <int W, bool S>
template <int W2, bool S2>
int_base<W, S>::int_base(const range_ref<W2, S2>& x) : int_base(x.value()) {}

template <int W, bool S>
template <int W2, bool S2>
int_base<W, S>::int_base(const bit_ref<W2, S2>& x)
    : int_base(static_cast<bool>(x) ? 1 : 0) {}

template <int W, bool S>
template <int W1, bool S1, int W2, bool S2>
int_base<W, S>::int_base(const concat_ref<W1, S1, W2, S2>& x)
    : int_base(x.value()) {}

/// Read-only concatenation value.
template <int W1, bool S1, int W2, bool S2>
inline u<W1 + W2> concat_value(const int_base<W1, S1>& hi,
                               const int_base<W2, S2>& lo) {
  u<W1 + W2> v(0);
  for (int b = 0; b < W2; ++b) v.set_bit(b, lo.get_bit(b));
  for (int b = 0; b < W1; ++b) v.set_bit(b + W2, hi.get_bit(b));
  return v;
}

/// Concatenation, hi bits from @p hi and lo bits from @p lo: a writable
/// proxy for lvalues, a value for const or temporary operands.
template <int W1, bool S1, int W2, bool S2>
inline concat_ref<W1, S1, W2, S2> concat(int_base<W1, S1>& hi,
                                         int_base<W2, S2>& lo) {
  return concat_ref<W1, S1, W2, S2>(hi, lo);
}
template <int W1, bool S1, int W2, bool S2>
inline u<W1 + W2> concat(const int_base<W1, S1>& hi,
                         const int_base<W2, S2>& lo) {
  return concat_value(hi, lo);
}

/// The vendor concatenation spelling: `(a, b)`. Concatenating slice
/// proxies has no static width and is unsupported (the built-in comma
/// would silently discard the left operand).
template <int W1, bool S1, int W2, bool S2>
inline concat_ref<W1, S1, W2, S2> operator,(int_base<W1, S1>& hi,
                                            int_base<W2, S2>& lo) {
  return concat_ref<W1, S1, W2, S2>(hi, lo);
}
template <int W1, bool S1, int W2, bool S2>
inline u<W1 + W2> operator,(const int_base<W1, S1>& hi,
                            const int_base<W2, S2>& lo) {
  return concat_value(hi, lo);
}

// `(a, b, c)` would silently degrade to the built-in comma — evaluating
// `(a, b)` for its side effects and yielding just `c` — because no
// operator matches a concat_result on the left. Make it loud instead.
template <int W1, bool S1, int W2, bool S2, typename T>
inline void operator,(const concat_ref<W1, S1, W2, S2>&, const T&) = delete;
template <int W1, bool S1, int W2, bool S2, typename T>
inline void operator,(const T&, const concat_ref<W1, S1, W2, S2>&) = delete;

// ── Arithmetic between tapa types ────────────────────────────────────────

#define TAPA_INT_BIN_OP(op, rt, combine)                      \
  template <int W1, bool S1, int W2, bool S2>                 \
  inline typename rtype<W1, S1, W2, S2>::rt operator op(      \
      const int_base<W1, S1>& a, const int_base<W2, S2>& b) { \
    using R = typename rtype<W1, S1, W2, S2>::rt;             \
    constexpr int n = internal::limbs<R::width>::count;       \
    uint64_t xa[n] = {};                                      \
    uint64_t ya[n] = {};                                      \
    uint64_t out[n] = {};                                     \
    R(a).template extend_to<n>(xa);                           \
    R(b).template extend_to<n>(ya);                           \
    internal::combine(xa, ya, out);                           \
    R r;                                                      \
    for (int li = 0; li < n; ++li) r.raw()[li] = out[li];     \
    r.raw().trim();                                           \
    return r;                                                 \
  }

TAPA_INT_BIN_OP(+, plus, add_limbs)
TAPA_INT_BIN_OP(-, minus, sub_limbs)
TAPA_INT_BIN_OP(*, mult, mul_limbs)
TAPA_INT_BIN_OP(&, logic, and_limbs)
TAPA_INT_BIN_OP(|, logic, or_limbs)
TAPA_INT_BIN_OP(^, logic, xor_limbs)

#undef TAPA_INT_BIN_OP

// Division and remainder compute at a common width that holds both operands
// exactly; the returned rtype widths then fit the exact values.
template <int W1, bool S1, int W2, bool S2>
inline typename rtype<W1, S1, W2, S2>::div operator/(
    const int_base<W1, S1>& a, const int_base<W2, S2>& b) {
  using C = typename rtype<W1, S1, W2, S2>::common;
  return C(C(a).divide(C(b), nullptr));
}

template <int W1, bool S1, int W2, bool S2>
inline typename rtype<W1, S1, W2, S2>::mod operator%(
    const int_base<W1, S1>& a, const int_base<W2, S2>& b) {
  using C = typename rtype<W1, S1, W2, S2>::common;
  C rem;
  C(a).divide(C(b), &rem);
  return C(rem);
}

// Shifts keep the left operand's width; a negative count shifts the other way.
template <int W1, bool S1, int W2, bool S2>
inline typename rtype<W1, S1, W2, S2>::arg1 operator<<(
    const int_base<W1, S1>& a, const int_base<W2, S2>& b) {
  typename rtype<W1, S1, W2, S2>::arg1 r(a);
  r <<= b;
  return r;
}
template <int W1, bool S1, int W2, bool S2>
inline typename rtype<W1, S1, W2, S2>::arg1 operator>>(
    const int_base<W1, S1>& a, const int_base<W2, S2>& b) {
  typename rtype<W1, S1, W2, S2>::arg1 r(a);
  r >>= b;
  return r;
}

// ── Mixed with builtin integral operands ─────────────────────────────────

#define TAPA_INT_BUILTIN_OP(op)                                 \
  template <int W, bool S, typename T,                          \
            typename = internal::enable_if_integral<T>>         \
  inline auto operator op(const int_base<W, S>& a, T b)         \
      ->decltype(a op typename internal::to_base<T>::type(b)) { \
    return a op typename internal::to_base<T>::type(b);         \
  }                                                             \
  template <int W, bool S, typename T,                          \
            typename = internal::enable_if_integral<T>>         \
  inline auto operator op(T a, const int_base<W, S>& b)         \
      ->decltype(typename internal::to_base<T>::type(a) op b) { \
    return typename internal::to_base<T>::type(a) op b;         \
  }

TAPA_INT_BUILTIN_OP(+)
TAPA_INT_BUILTIN_OP(-)
TAPA_INT_BUILTIN_OP(*)
TAPA_INT_BUILTIN_OP(/)
TAPA_INT_BUILTIN_OP(%)
TAPA_INT_BUILTIN_OP(&)
TAPA_INT_BUILTIN_OP(|)
TAPA_INT_BUILTIN_OP(^)
TAPA_INT_BUILTIN_OP(<<)
TAPA_INT_BUILTIN_OP(>>)

#undef TAPA_INT_BUILTIN_OP

#define TAPA_INT_BUILTIN_REL(op)                          \
  template <int W, bool S, typename T,                    \
            typename = internal::enable_if_integral<T>>   \
  inline bool operator op(const int_base<W, S>& a, T b) { \
    return a op typename internal::to_base<T>::type(b);   \
  }                                                       \
  template <int W, bool S, typename T,                    \
            typename = internal::enable_if_integral<T>>   \
  inline bool operator op(T a, const int_base<W, S>& b) { \
    return typename internal::to_base<T>::type(a) op b;   \
  }

TAPA_INT_BUILTIN_REL(==)
TAPA_INT_BUILTIN_REL(!=)
TAPA_INT_BUILTIN_REL(<)
TAPA_INT_BUILTIN_REL(<=)
TAPA_INT_BUILTIN_REL(>)
TAPA_INT_BUILTIN_REL(>=)

#undef TAPA_INT_BUILTIN_REL

// Floating mixing truncates the tapa operand to its 64-bit RetType first,
// then computes in the floating type (vendor behavior).
#define TAPA_INT_FLOAT_OP(op)                                               \
  template <int W, bool S, typename T,                                      \
            typename = internal::enable_if_floating<T>>                     \
  inline T operator op(const int_base<W, S>& a, T b) {                      \
    return static_cast<T>(static_cast<typename int_base<W, S>::RetType>(a)) \
        op b;                                                               \
  }                                                                         \
  template <int W, bool S, typename T,                                      \
            typename = internal::enable_if_floating<T>>                     \
  inline T operator op(T a, const int_base<W, S>& b) {                      \
    return a op static_cast<T>(                                             \
        static_cast<typename int_base<W, S>::RetType>(b));                  \
  }

TAPA_INT_FLOAT_OP(+)
TAPA_INT_FLOAT_OP(-)
TAPA_INT_FLOAT_OP(*)

#undef TAPA_INT_FLOAT_OP

// ── Compound assignment bodies ───────────────────────────────────────────

template <int W, bool S>
inline int_base<W, S>& int_base<W, S>::operator+=(int_base x) {
  return *this = *this + x;
}
template <int W, bool S>
inline int_base<W, S>& int_base<W, S>::operator-=(int_base x) {
  return *this = *this - x;
}
template <int W, bool S>
inline int_base<W, S>& int_base<W, S>::operator*=(int_base x) {
  return *this = *this * x;
}
template <int W, bool S>
inline int_base<W, S>& int_base<W, S>::operator&=(int_base x) {
  return *this = *this & x;
}
template <int W, bool S>
inline int_base<W, S>& int_base<W, S>::operator|=(int_base x) {
  return *this = *this | x;
}
template <int W, bool S>
inline int_base<W, S>& int_base<W, S>::operator^=(int_base x) {
  return *this = *this ^ x;
}
template <int W, bool S>
template <int W2, bool S2>
inline int_base<W, S>& int_base<W, S>::operator/=(const int_base<W2, S2>& x) {
  return *this = *this / x;
}
template <int W, bool S>
template <int W2, bool S2>
inline int_base<W, S>& int_base<W, S>::operator%=(const int_base<W2, S2>& x) {
  return *this = *this % x;
}
template <int W, bool S>
template <int W2, bool S2>
inline int_base<W, S>& int_base<W, S>::operator<<=(const int_base<W2, S2>& x) {
  // Only a SIGNED count can be negative; an unsigned count with its top
  // bit set is a large positive shift (vendor rule).
  if (S2 && x.negative()) {
    shr_storage(x.shift_count());
  } else {
    shl_storage(x.shift_count());
  }
  return *this;
}
template <int W, bool S>
template <int W2, bool S2>
inline int_base<W, S>& int_base<W, S>::operator>>=(const int_base<W2, S2>& x) {
  if (S2 && x.negative()) {
    shl_storage(x.shift_count());
  } else {
    shr_storage(x.shift_count());
  }
  return *this;
}

}  // namespace tapa

#endif  // !TAPA_TARGET_XILINX_HLS_
