// Live parity against the vendor arbitrary-precision types.
//
// tapa::u/i, tapa::fixed/ufixed and tapa::axis are self-implemented
// stand-ins for ap_uint/ap_int, ap_fixed/ap_ufixed and ap_axiu, and
// everything a TAPA program observes in software simulation -- object size,
// member offsets, arithmetic result widths, every bit of every result -- has
// to be what the vendor produces, or CPU simulation and hardware disagree
// with nothing to say so.
//
// The other tests state the expected behaviour independently, from the
// documented rules. This one takes the vendor's own answer as the oracle:
// it builds the same value both ways, applies the same operation, and
// compares bit patterns. It is what caught ap_uint's object size for widths
// whose limb count is not a power of two (192 bits took 24 bytes here and 32
// there) -- a difference no hand-written expectation had thought to check.
//
// Where the vendor headers are not installed there is nothing to compare
// against, and `@vitis_hls//:include` is an EMPTY cc_library, so this file
// would compile, check nothing, and report PASSED. The BUILD target declares
// itself incompatible there instead and Bazel reports SKIPPED, which is what
// actually happened; the `__has_include` guard below is the backstop for a
// build that reaches this file some other way. Either way a PASS means the
// comparison ran.
//
// Regenerate the constants in host/axis_test.cpp and base/int_test.cpp from
// this test's numbers whenever a new Vitis reshapes anything -- those are
// what still pins the layout where this cannot run.

#if defined(__has_include)
#if __has_include(<ap_int.h>) && __has_include(<ap_axi_sdata.h>) && \
    __has_include(<ap_fixed.h>)
#define TAPA_VENDOR_HEADERS_AVAILABLE 1
#endif
#endif

#ifdef TAPA_VENDOR_HEADERS_AVAILABLE
#include <ap_axi_sdata.h>
#include <ap_fixed.h>
#include <ap_int.h>
#endif

#include <cstddef>
#include <cstdint>
#include <string>
#include <type_traits>
#include <vector>

#include "gtest/gtest.h"

#include "tapa.h"

namespace tapa {
namespace {

#ifndef TAPA_VENDOR_HEADERS_AVAILABLE

TEST(VendorParity, Skipped) {
  GTEST_SKIP() << "the vendor headers are not installed, so the oracle cannot "
                  "run. Reaching this is unexpected under Bazel, which marks "
                  "the target incompatible instead; layout stays pinned by "
                  "the recorded constants in host/axis_test.cpp and "
                  "base/int_test.cpp.";
}

#else

// A deterministic bit source: the same sequence on every host and every run,
// so a failure is reproducible from the test name alone.
uint64_t Next(uint64_t& state) {
  state += 0x9e3779b97f4a7c15ULL;
  uint64_t z = state;
  z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
  z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
  return z ^ (z >> 31);
}

std::vector<bool> RandomBits(int width, uint64_t& state) {
  std::vector<bool> bits(width);
  for (int b = 0; b < width; ++b) bits[b] = (Next(state) & 1) != 0;
  return bits;
}

template <typename T>
std::string TapaBits(const T& x) {
  std::string s;
  for (int b = x.length() - 1; b >= 0; --b) s += x.get_bit(b) ? '1' : '0';
  return s;
}

template <typename T>
std::string VendorBits(const T& x) {
  std::string s;
  for (int b = x.length() - 1; b >= 0; --b) s += x[b] ? '1' : '0';
  return s;
}

template <typename T>
void SetTapa(T& x, const std::vector<bool>& bits) {
  for (size_t b = 0; b < bits.size(); ++b)
    x.set_bit(static_cast<int>(b), bits[b]);
}

template <typename T>
void SetVendor(T& x, const std::vector<bool>& bits) {
  for (size_t b = 0; b < bits.size(); ++b) x[static_cast<int>(b)] = bits[b];
}

// ---------------------------------------------------------------- layout

template <int W>
void CheckIntLayout() {
  EXPECT_EQ(sizeof(u<W>), sizeof(ap_uint<W>)) << "unsigned width " << W;
  EXPECT_EQ(alignof(u<W>), alignof(ap_uint<W>)) << "unsigned width " << W;
  EXPECT_EQ(sizeof(i<W>), sizeof(ap_int<W>)) << "signed width " << W;
  EXPECT_EQ(alignof(i<W>), alignof(ap_int<W>)) << "signed width " << W;
}

TEST(VendorParity, IntegerObjectLayout) {
  // Both sides of every rounding boundary, plus the limb counts that are not
  // powers of two -- 3 limbs (192) and 5 (320) are where the two rules first
  // disagreed.
  CheckIntLayout<1>();
  CheckIntLayout<7>();
  CheckIntLayout<8>();
  CheckIntLayout<9>();
  CheckIntLayout<16>();
  CheckIntLayout<17>();
  CheckIntLayout<24>();
  CheckIntLayout<32>();
  CheckIntLayout<33>();
  CheckIntLayout<48>();
  CheckIntLayout<64>();
  CheckIntLayout<65>();
  CheckIntLayout<96>();
  CheckIntLayout<128>();
  CheckIntLayout<129>();
  CheckIntLayout<192>();
  CheckIntLayout<256>();
  CheckIntLayout<288>();
  CheckIntLayout<320>();
  CheckIntLayout<448>();
  CheckIntLayout<512>();
  CheckIntLayout<576>();
  CheckIntLayout<1024>();
}

// A struct is where a wrong alignment shows up: the size alone can agree
// while every member after the value sits somewhere else.
template <int W>
void CheckMixedStructLayout() {
  struct Tapa {
    u<W> wide;
    uint8_t tag;
  };
  struct Vendor {
    ap_uint<W> wide;
    uint8_t tag;
  };
  EXPECT_EQ(sizeof(Tapa), sizeof(Vendor)) << "width " << W;
  EXPECT_EQ(offsetof(Tapa, tag), offsetof(Vendor, tag)) << "width " << W;
}

TEST(VendorParity, IntegerInsideAStruct) {
  CheckMixedStructLayout<32>();
  CheckMixedStructLayout<64>();
  CheckMixedStructLayout<96>();
  CheckMixedStructLayout<192>();
  CheckMixedStructLayout<288>();
  CheckMixedStructLayout<512>();
}

template <int W, int WUser, int WId, int WDest>
void CheckAxisLayout() {
  using T = axis<u<W>, WUser, WId, WDest>;
  using V = ap_axiu<W, WUser, WId, WDest>;
  EXPECT_EQ(sizeof(T), sizeof(V)) << "axis width " << W;
  EXPECT_EQ(alignof(T), alignof(V)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, data), offsetof(V, data)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, keep), offsetof(V, keep)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, strb), offsetof(V, strb)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, user), offsetof(V, user)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, last), offsetof(V, last)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, id), offsetof(V, id)) << "axis width " << W;
  EXPECT_EQ(offsetof(T, dest), offsetof(V, dest)) << "axis width " << W;
  EXPECT_EQ(int{T::width_keep}, static_cast<int>(V::width_keep))
      << "axis width " << W;
  EXPECT_EQ(int{T::width_strb}, static_cast<int>(V::width_strb))
      << "axis width " << W;
}

TEST(VendorParity, AxisPacketLayout) {
  CheckAxisLayout<8, 0, 0, 0>();
  CheckAxisLayout<12, 0, 0, 0>();
  CheckAxisLayout<32, 0, 0, 0>();
  CheckAxisLayout<64, 0, 0, 0>();
  CheckAxisLayout<512, 0, 0, 0>();
  CheckAxisLayout<32, 4, 3, 2>();
  CheckAxisLayout<64, 8, 0, 0>();
  CheckAxisLayout<128, 0, 5, 0>();
  CheckAxisLayout<288, 2, 0, 0>();
}

TEST(VendorParity, AxisDefaultState) {
  const axis<u<32>, 2, 0, 0> mine;
  const ap_axiu<32, 2, 0, 0> theirs;
  EXPECT_EQ(TapaBits(mine.keep), VendorBits(theirs.keep));
  EXPECT_EQ(TapaBits(mine.strb), VendorBits(theirs.strb));
  EXPECT_EQ(TapaBits(mine.last), VendorBits(theirs.last));
  EXPECT_EQ(TapaBits(mine.get_user()), VendorBits(theirs.user));

  axis<u<512> > wide_mine;
  ap_axiu<512, 0, 0, 0> wide_theirs;
  wide_mine.keep = 0;
  wide_theirs.keep = 0;
  wide_mine.keep_all();
  wide_theirs.keep_all();
  EXPECT_EQ(TapaBits(wide_mine.keep), VendorBits(wide_theirs.keep));
}

// ------------------------------------------------------------ arithmetic

// Result widths come from the vendor's RType rules; getting one wrong
// truncates a product or an intermediate sum with no other symptom.
TEST(VendorParity, MixedArithmeticResultWidths) {
  // `ap_int_base::width` is a static const with no out-of-class definition,
  // so it has to be read as a value; passing it to EXPECT_EQ by reference
  // would odr-use it and fail to link.
#define TAPA_CHECK_WIDTH(expr_tapa, expr_vendor) \
  EXPECT_EQ(int{decltype(expr_tapa)::width},     \
            int{decltype(expr_vendor)::width})   \
      << #expr_tapa
  u<12> a12;
  u<20> b20;
  i<12> s12;
  i<20> t20;
  ap_uint<12> va12;
  ap_uint<20> vb20;
  ap_int<12> vs12;
  ap_int<20> vt20;

  TAPA_CHECK_WIDTH(a12 + b20, va12 + vb20);
  TAPA_CHECK_WIDTH(a12 - b20, va12 - vb20);
  TAPA_CHECK_WIDTH(a12 * b20, va12 * vb20);
  TAPA_CHECK_WIDTH(a12 / b20, va12 / vb20);
  TAPA_CHECK_WIDTH(a12 % b20, va12 % vb20);
  TAPA_CHECK_WIDTH(a12 & b20, va12 & vb20);
  TAPA_CHECK_WIDTH(a12 | b20, va12 | vb20);
  TAPA_CHECK_WIDTH(a12 ^ b20, va12 ^ vb20);

  TAPA_CHECK_WIDTH(s12 + t20, vs12 + vt20);
  TAPA_CHECK_WIDTH(s12 - t20, vs12 - vt20);
  TAPA_CHECK_WIDTH(s12 * t20, vs12 * vt20);
  TAPA_CHECK_WIDTH(s12 / t20, vs12 / vt20);
  TAPA_CHECK_WIDTH(s12 % t20, vs12 % vt20);

  // Mixed signedness is where the width rules stop being symmetric.
  TAPA_CHECK_WIDTH(a12 + t20, va12 + vt20);
  TAPA_CHECK_WIDTH(s12 + b20, vs12 + vb20);
  TAPA_CHECK_WIDTH(a12 - t20, va12 - vt20);
  TAPA_CHECK_WIDTH(s12 * b20, vs12 * vb20);
  TAPA_CHECK_WIDTH(a12 / t20, va12 / vt20);
  TAPA_CHECK_WIDTH(s12 % b20, vs12 % vb20);
  TAPA_CHECK_WIDTH(a12 & t20, va12 & vt20);
#undef TAPA_CHECK_WIDTH
}

template <int W>
void CheckBinaryOps(uint64_t& state, int rounds) {
  for (int r = 0; r < rounds; ++r) {
    const std::vector<bool> ba = RandomBits(W, state);
    const std::vector<bool> bb = RandomBits(W, state);
    u<W> a;
    u<W> b;
    ap_uint<W> va;
    ap_uint<W> vb;
    SetTapa(a, ba);
    SetTapa(b, bb);
    SetVendor(va, ba);
    SetVendor(vb, bb);
    ASSERT_EQ(TapaBits(a), VendorBits(va)) << "width " << W << " round " << r;
    ASSERT_EQ(TapaBits(b), VendorBits(vb)) << "width " << W << " round " << r;

    const std::string where = "width " + std::to_string(W) + " round " +
                              std::to_string(r) + " a=" + TapaBits(a) +
                              " b=" + TapaBits(b);

    EXPECT_EQ(TapaBits(u<W + 1>(a + b)), VendorBits(ap_uint<W + 1>(va + vb)))
        << "add, " << where;
    EXPECT_EQ(TapaBits(u<W + 1>(a - b)), VendorBits(ap_uint<W + 1>(va - vb)))
        << "sub, " << where;
    EXPECT_EQ(TapaBits(u<2 * W>(a * b)), VendorBits(ap_uint<2 * W>(va * vb)))
        << "mul, " << where;
    EXPECT_EQ(TapaBits(u<W>(a & b)), VendorBits(ap_uint<W>(va & vb)))
        << "and, " << where;
    EXPECT_EQ(TapaBits(u<W>(a | b)), VendorBits(ap_uint<W>(va | vb)))
        << "or, " << where;
    EXPECT_EQ(TapaBits(u<W>(a ^ b)), VendorBits(ap_uint<W>(va ^ vb)))
        << "xor, " << where;
    EXPECT_EQ(TapaBits(u<W>(~a)), VendorBits(ap_uint<W>(~va)))
        << "not, " << where;

    if (!b.is_zero()) {
      EXPECT_EQ(TapaBits(u<W>(a / b)), VendorBits(ap_uint<W>(va / vb)))
          << "div, " << where;
      EXPECT_EQ(TapaBits(u<W>(a % b)), VendorBits(ap_uint<W>(va % vb)))
          << "mod, " << where;
    }

    // Reductions and the truth test.
    EXPECT_EQ(a.and_reduce(), va.and_reduce()) << "and_reduce, " << where;
    EXPECT_EQ(a.or_reduce(), va.or_reduce()) << "or_reduce, " << where;
    EXPECT_EQ(a.xor_reduce(), va.xor_reduce()) << "xor_reduce, " << where;

    // Equality compares the sign/zero-extended patterns at max(W, 64);
    // ordering follows C's conversions with the width as the rank.
    EXPECT_EQ(a == b, va == vb) << "eq, " << where;
    EXPECT_EQ(a < b, va < vb) << "lt, " << where;
    EXPECT_EQ(a <= b, va <= vb) << "le, " << where;
    EXPECT_EQ(a > b, va > vb) << "gt, " << where;
    EXPECT_EQ(a >= b, va >= vb) << "ge, " << where;

    // Shifts, including the negative amounts that reverse direction.
    for (int sh : {0, 1, 3, W / 2, W - 1, W, -1, -(W / 2)}) {
      EXPECT_EQ(TapaBits(u<W>(a << sh)), VendorBits(ap_uint<W>(va << sh)))
          << "shl " << sh << ", " << where;
      EXPECT_EQ(TapaBits(u<W>(a >> sh)), VendorBits(ap_uint<W>(va >> sh)))
          << "shr " << sh << ", " << where;
    }
  }
}

template <int W>
void CheckSignedOps(uint64_t& state, int rounds) {
  for (int r = 0; r < rounds; ++r) {
    const std::vector<bool> ba = RandomBits(W, state);
    const std::vector<bool> bb = RandomBits(W, state);
    i<W> a;
    i<W> b;
    ap_int<W> va;
    ap_int<W> vb;
    SetTapa(a, ba);
    SetTapa(b, bb);
    SetVendor(va, ba);
    SetVendor(vb, bb);

    const std::string where = "signed width " + std::to_string(W) + " round " +
                              std::to_string(r) + " a=" + TapaBits(a) +
                              " b=" + TapaBits(b);

    EXPECT_EQ(TapaBits(i<W + 1>(a + b)), VendorBits(ap_int<W + 1>(va + vb)))
        << "add, " << where;
    EXPECT_EQ(TapaBits(i<W + 1>(a - b)), VendorBits(ap_int<W + 1>(va - vb)))
        << "sub, " << where;
    EXPECT_EQ(TapaBits(i<2 * W>(a * b)), VendorBits(ap_int<2 * W>(va * vb)))
        << "mul, " << where;
    EXPECT_EQ(TapaBits(i<W>(-a)), VendorBits(ap_int<W>(-va)))
        << "neg, " << where;
    if (!b.is_zero()) {
      // Division truncates toward zero and the remainder keeps the dividend's
      // sign -- C rules, not floored ones.
      EXPECT_EQ(TapaBits(i<W>(a / b)), VendorBits(ap_int<W>(va / vb)))
          << "div, " << where;
      EXPECT_EQ(TapaBits(i<W>(a % b)), VendorBits(ap_int<W>(va % vb)))
          << "mod, " << where;
    }
    EXPECT_EQ(a < b, va < vb) << "lt, " << where;
    EXPECT_EQ(a >= b, va >= vb) << "ge, " << where;
    // An arithmetic right shift replicates the sign bit.
    for (int sh : {1, 3, W - 1, -1}) {
      EXPECT_EQ(TapaBits(i<W>(a >> sh)), VendorBits(ap_int<W>(va >> sh)))
          << "shr " << sh << ", " << where;
    }
  }
}

TEST(VendorParity, UnsignedArithmetic) {
  uint64_t state = 0x1234;
  CheckBinaryOps<8>(state, 24);
  CheckBinaryOps<13>(state, 24);
  CheckBinaryOps<32>(state, 24);
  CheckBinaryOps<64>(state, 24);
  CheckBinaryOps<65>(state, 16);
  CheckBinaryOps<96>(state, 16);
  CheckBinaryOps<128>(state, 16);
  CheckBinaryOps<192>(state, 8);
  CheckBinaryOps<256>(state, 8);
}

TEST(VendorParity, SignedArithmetic) {
  uint64_t state = 0x5678;
  CheckSignedOps<8>(state, 24);
  CheckSignedOps<13>(state, 24);
  CheckSignedOps<32>(state, 24);
  CheckSignedOps<64>(state, 24);
  CheckSignedOps<65>(state, 16);
  CheckSignedOps<128>(state, 16);
  CheckSignedOps<192>(state, 8);
}

// Comparison across widths and signedness, which is where the two most
// nearly diverged: ordering follows C's usual arithmetic conversions with
// the declared width as the rank, and equality does not follow them at all.
// Neither rule is guessable, and a same-signedness test cannot see either.
template <int W1, bool S1, int W2, bool S2>
void CheckMixedCompare(uint64_t& state, int rounds) {
  using mine1 = typename std::conditional<S1, i<W1>, u<W1> >::type;
  using mine2 = typename std::conditional<S2, i<W2>, u<W2> >::type;
  using theirs1 = typename std::conditional<S1, ap_int<W1>, ap_uint<W1> >::type;
  using theirs2 = typename std::conditional<S2, ap_int<W2>, ap_uint<W2> >::type;
  for (int r = 0; r < rounds; ++r) {
    const std::vector<bool> ba = RandomBits(W1, state);
    const std::vector<bool> bb = RandomBits(W2, state);
    mine1 a;
    mine2 b;
    theirs1 va;
    theirs2 vb;
    SetTapa(a, ba);
    SetTapa(b, bb);
    SetVendor(va, ba);
    SetVendor(vb, bb);
    const std::string where = std::string(S1 ? "i" : "u") + std::to_string(W1) +
                              " vs " + (S2 ? "i" : "u") + std::to_string(W2) +
                              " round " + std::to_string(r) +
                              " a=" + TapaBits(a) + " b=" + TapaBits(b);
    EXPECT_EQ(a == b, va == vb) << "eq " << where;
    EXPECT_EQ(a != b, va != vb) << "ne " << where;
    EXPECT_EQ(a < b, va < vb) << "lt " << where;
    EXPECT_EQ(a <= b, va <= vb) << "le " << where;
    EXPECT_EQ(a > b, va > vb) << "gt " << where;
    EXPECT_EQ(a >= b, va >= vb) << "ge " << where;
  }
}

TEST(VendorParity, MixedComparison) {
  uint64_t state = 0x77;
  // Both below 32 bits, where C promotes to int and the comparison goes
  // signed however the operands are spelled.
  CheckMixedCompare<8, true, 8, false>(state, 60);
  CheckMixedCompare<8, false, 8, true>(state, 60);
  CheckMixedCompare<15, false, 10, true>(state, 60);
  CheckMixedCompare<10, true, 15, false>(state, 60);
  // At and above 32, where the wider-or-equal operand's signedness wins.
  CheckMixedCompare<32, true, 32, false>(state, 60);
  CheckMixedCompare<32, false, 32, true>(state, 60);
  CheckMixedCompare<32, true, 8, false>(state, 60);
  CheckMixedCompare<8, false, 32, true>(state, 60);
  CheckMixedCompare<40, true, 32, false>(state, 60);
  CheckMixedCompare<32, false, 40, true>(state, 60);
  // Past one limb, where the vendor's narrow and wide implementations part
  // company.
  CheckMixedCompare<96, true, 64, false>(state, 40);
  CheckMixedCompare<64, false, 96, true>(state, 40);
  CheckMixedCompare<128, true, 128, false>(state, 40);
  CheckMixedCompare<192, false, 65, true>(state, 40);
  // Same signedness, for completeness.
  CheckMixedCompare<12, true, 20, true>(state, 40);
  CheckMixedCompare<12, false, 20, false>(state, 40);
  CheckMixedCompare<96, true, 40, true>(state, 40);
}

// ----------------------------------------------------------- bit access

// Reading a slice wider than 64 bits used to truncate here; nothing else in
// the API makes that visible.
template <int W>
void CheckRanges(uint64_t& state, int rounds) {
  for (int r = 0; r < rounds; ++r) {
    const std::vector<bool> bits = RandomBits(W, state);
    u<W> a;
    ap_uint<W> va;
    SetTapa(a, bits);
    SetVendor(va, bits);
    const u<W>& ca = a;
    const ap_uint<W>& cva = va;

    for (int lo = 0; lo < W; lo += (W / 7) + 1) {
      for (int hi = lo; hi < W; hi += (W / 5) + 1) {
        const std::string where =
            "width " + std::to_string(W) + " [" + std::to_string(hi) + ":" +
            std::to_string(lo) + "] round " + std::to_string(r);
        // The result is right-aligned in the PARENT width on both sides.
        EXPECT_EQ(TapaBits(u<W>(ca(hi, lo))),
                  VendorBits(ap_uint<W>(cva(hi, lo))))
            << "range, " << where;
        EXPECT_EQ(TapaBits(u<W>(ca.range(hi, lo))),
                  VendorBits(ap_uint<W>(cva.range(hi, lo))))
            << "range(), " << where;
        // The mutable proxy assigns the same bits back.
        u<W> lhs;
        ap_uint<W> vlhs;
        SetTapa(lhs, bits);
        SetVendor(vlhs, bits);
        lhs(hi, lo) = ~u<W>(0);
        vlhs(hi, lo) = ~ap_uint<W>(0);
        EXPECT_EQ(TapaBits(lhs), VendorBits(vlhs)) << "range assign, " << where;
      }
    }

    for (int b = 0; b < W; b += (W / 9) + 1) {
      EXPECT_EQ(ca[b], static_cast<bool>(cva[b]))
          << "bit " << b << " width " << W;
    }
  }
}

TEST(VendorParity, RangeAndBitAccess) {
  uint64_t state = 0x9abc;
  CheckRanges<32>(state, 4);
  CheckRanges<64>(state, 4);
  CheckRanges<96>(state, 4);
  CheckRanges<128>(state, 4);
  CheckRanges<192>(state, 2);
  CheckRanges<256>(state, 2);
}

TEST(VendorParity, Concatenation) {
  uint64_t state = 0xdef0;
  for (int r = 0; r < 16; ++r) {
    const std::vector<bool> ba = RandomBits(40, state);
    const std::vector<bool> bb = RandomBits(24, state);
    u<40> a;
    u<24> b;
    ap_uint<40> va;
    ap_uint<24> vb;
    SetTapa(a, ba);
    SetTapa(b, bb);
    SetVendor(va, ba);
    SetVendor(vb, bb);
    EXPECT_EQ(TapaBits(u<64>((a, b))), VendorBits(ap_uint<64>((va, vb))))
        << "concat round " << r;
  }
}

// ---------------------------------------------------------- conversions

TEST(VendorParity, ConversionsFromAndToNative) {
  const int64_t values[] = {0,
                            1,
                            -1,
                            2,
                            -2,
                            127,
                            -128,
                            255,
                            256,
                            -256,
                            4095,
                            -4096,
                            65535,
                            -65536,
                            0x7fffffffLL,
                            -0x80000000LL,
                            0x123456789abcdefLL,
                            -0x123456789abcdefLL};
  for (const int64_t v : values) {
    EXPECT_EQ(TapaBits(u<12>(v)), VendorBits(ap_uint<12>(v))) << "u12 " << v;
    EXPECT_EQ(TapaBits(i<12>(v)), VendorBits(ap_int<12>(v))) << "i12 " << v;
    EXPECT_EQ(TapaBits(u<64>(v)), VendorBits(ap_uint<64>(v))) << "u64 " << v;
    EXPECT_EQ(TapaBits(i<64>(v)), VendorBits(ap_int<64>(v))) << "i64 " << v;
    EXPECT_EQ(TapaBits(u<96>(v)), VendorBits(ap_uint<96>(v))) << "u96 " << v;
    EXPECT_EQ(TapaBits(i<96>(v)), VendorBits(ap_int<96>(v))) << "i96 " << v;

    EXPECT_EQ(i<12>(v).to_int64(), ap_int<12>(v).to_int64())
        << "to_int64 " << v;
    EXPECT_EQ(u<12>(v).to_uint64(), ap_uint<12>(v).to_uint64())
        << "to_uint64 " << v;
    EXPECT_EQ(i<40>(v).to_int64(), ap_int<40>(v).to_int64())
        << "to_int64 " << v;
    EXPECT_EQ(i<96>(v).to_int64(), ap_int<96>(v).to_int64())
        << "to_int64 " << v;
  }

  const double reals[] = {0.0,     1.0,      -1.0, 2.5, -2.5,
                          1023.75, -1023.75, 1e6,  -1e6};
  for (const double d : reals) {
    EXPECT_EQ(TapaBits(i<32>(d)), VendorBits(ap_int<32>(d)))
        << "from double " << d;
    EXPECT_EQ(TapaBits(u<32>(d)), VendorBits(ap_uint<32>(d)))
        << "from double " << d;
    EXPECT_DOUBLE_EQ(i<32>(d).to_double(), ap_int<32>(d).to_double())
        << "to_double " << d;
  }
}

// ---------------------------------------------------------- fixed point

template <int W, int I, bool S, q_mode Q, o_mode O, int N>
struct fx {
  using mine = typename std::conditional<S, fixed<W, I, Q, O, N>,
                                         ufixed<W, I, Q, O, N> >::type;
  using theirs = typename std::conditional<
      S,
      ap_fixed<W, I, static_cast<ap_q_mode>(Q), static_cast<ap_o_mode>(O), N>,
      ap_ufixed<W, I, static_cast<ap_q_mode>(Q), static_cast<ap_o_mode>(O),
                N> >::type;
};

template <typename Mine, typename Theirs>
void SetPair(Mine& mine, Theirs& theirs, const std::vector<bool>& bits) {
  for (size_t b = 0; b < bits.size(); ++b) {
    mine.V.set_bit(static_cast<int>(b), bits[b]);
    theirs.V[static_cast<int>(b)] = bits[b];
  }
}

// Every conversion in one place: build the same source both ways, assign it
// into the target type, compare the raw patterns. Quantization and overflow
// are what this exercises, so the source is deliberately wider and finer
// than the target.
template <int W, int I, bool S, q_mode Q, o_mode O, int N, int W2, int I2,
          bool S2>
void CheckConvert(uint64_t& state, int rounds, const char* what) {
  using target = fx<W, I, S, Q, O, N>;
  using source = fx<W2, I2, S2, q_mode::trn, o_mode::wrap, 0>;
  for (int r = 0; r < rounds; ++r) {
    const std::vector<bool> bits = RandomBits(W2, state);
    typename source::mine src_mine;
    typename source::theirs src_theirs;
    SetPair(src_mine, src_theirs, bits);
    ASSERT_EQ(TapaBits(src_mine.V), VendorBits(src_theirs.V));

    typename target::mine dst_mine(src_mine);
    typename target::theirs dst_theirs(src_theirs);
    EXPECT_EQ(TapaBits(dst_mine.V), VendorBits(dst_theirs.V))
        << what << " round " << r << " src=" << TapaBits(src_mine.V);
  }
}

// Every quantization mode, against a source with more fractional bits than
// the target keeps.
TEST(VendorParity, FixedQuantizationModes) {
  uint64_t state = 0x11;
#define TAPA_CHECK_Q(q)                                                 \
  CheckConvert<12, 6, true, q_mode::q, o_mode::wrap, 0, 24, 6, true>(   \
      state, 40, "signed " #q);                                         \
  CheckConvert<12, 6, false, q_mode::q, o_mode::wrap, 0, 24, 6, false>( \
      state, 40, "unsigned " #q);                                       \
  CheckConvert<12, 6, true, q_mode::q, o_mode::wrap, 0, 24, 6, false>(  \
      state, 40, "signed from unsigned " #q);                           \
  CheckConvert<12, 6, false, q_mode::q, o_mode::wrap, 0, 24, 6, true>(  \
      state, 40, "unsigned from signed " #q);                           \
  CheckConvert<9, 9, true, q_mode::q, o_mode::wrap, 0, 20, 9, true>(    \
      state, 40, "to whole " #q);
  TAPA_CHECK_Q(trn)
  TAPA_CHECK_Q(trn_zero)
  TAPA_CHECK_Q(rnd)
  TAPA_CHECK_Q(rnd_zero)
  TAPA_CHECK_Q(rnd_min_inf)
  TAPA_CHECK_Q(rnd_inf)
  TAPA_CHECK_Q(rnd_conv)
#undef TAPA_CHECK_Q
}

// Every overflow mode, against a source with a wider integer part.
TEST(VendorParity, FixedOverflowModes) {
  uint64_t state = 0x22;
#define TAPA_CHECK_O(o, n)                                             \
  CheckConvert<10, 5, true, q_mode::trn, o_mode::o, n, 20, 14, true>(  \
      state, 40, "signed " #o "/" #n);                                 \
  CheckConvert<10, 5, true, q_mode::rnd, o_mode::o, n, 20, 14, true>(  \
      state, 40, "rounded signed " #o "/" #n);                         \
  CheckConvert<10, 5, true, q_mode::trn, o_mode::o, n, 20, 14, false>( \
      state, 40, "signed from unsigned " #o "/" #n);
  // Unsigned targets, for the modes an unsigned value can have: the vendor
  // aborts at run time on ap_ufixed with AP_WRAP_SM, and tapa::ufixed
  // refuses it at compile time.
#define TAPA_CHECK_O_UNSIGNED(o, n)                                     \
  CheckConvert<10, 5, false, q_mode::trn, o_mode::o, n, 20, 14, false>( \
      state, 40, "unsigned " #o "/" #n);                                \
  CheckConvert<10, 5, false, q_mode::rnd, o_mode::o, n, 20, 14, true>(  \
      state, 40, "unsigned from signed " #o "/" #n);
  TAPA_CHECK_O(wrap, 0)
  TAPA_CHECK_O(wrap, 1)
  TAPA_CHECK_O(wrap, 3)
  TAPA_CHECK_O(sat, 0)
  TAPA_CHECK_O(sat_zero, 0)
  TAPA_CHECK_O(sat_sym, 0)
  TAPA_CHECK_O(wrap_sm, 0)
  TAPA_CHECK_O(wrap_sm, 1)
  TAPA_CHECK_O(wrap_sm, 3)
  TAPA_CHECK_O_UNSIGNED(wrap, 0)
  TAPA_CHECK_O_UNSIGNED(wrap, 1)
  TAPA_CHECK_O_UNSIGNED(wrap, 3)
  TAPA_CHECK_O_UNSIGNED(sat, 0)
  TAPA_CHECK_O_UNSIGNED(sat_zero, 0)
  TAPA_CHECK_O_UNSIGNED(sat_sym, 0)
#undef TAPA_CHECK_O_UNSIGNED
#undef TAPA_CHECK_O
}

// Shapes the arithmetic never produces but a user can still write: no
// fractional bits, no integer bits, and a binary point outside the word.
TEST(VendorParity, FixedUnusualShapes) {
  uint64_t state = 0x33;
  CheckConvert<12, 12, true, q_mode::rnd, o_mode::sat, 0, 20, 6, true>(
      state, 40, "whole");
  CheckConvert<12, 0, true, q_mode::rnd, o_mode::sat, 0, 20, 6, true>(
      state, 40, "fraction");
  CheckConvert<12, 18, true, q_mode::rnd, o_mode::sat, 0, 20, 6, true>(
      state, 40, "point above the word");
  CheckConvert<12, -4, true, q_mode::rnd, o_mode::sat, 0, 20, 6, true>(
      state, 40, "point below the word");
  CheckConvert<1, 1, true, q_mode::rnd, o_mode::sat, 0, 20, 6, true>(state, 40,
                                                                     "one bit");
  CheckConvert<40, 20, true, q_mode::rnd_conv, o_mode::sat_sym, 0, 80, 40,
               true>(state, 20, "wide");
}

TEST(VendorParity, FixedObjectLayout) {
  EXPECT_EQ(sizeof(ufixed<32, 8>), sizeof(ap_ufixed<32, 8>));
  EXPECT_EQ(alignof(ufixed<32, 8>), alignof(ap_ufixed<32, 8>));
  EXPECT_EQ(sizeof(fixed<18, 4>), sizeof(ap_fixed<18, 4>));
  EXPECT_EQ(sizeof(fixed<64, 32>), sizeof(ap_fixed<64, 32>));
  EXPECT_EQ(sizeof(fixed<96, 32>), sizeof(ap_fixed<96, 32>));
  EXPECT_EQ(alignof(fixed<96, 32>), alignof(ap_fixed<96, 32>));
}

TEST(VendorParity, FixedArithmeticResultWidths) {
  ufixed<32, 8> a;
  fixed<18, 4> b;
  ap_ufixed<32, 8> va;
  ap_fixed<18, 4> vb;
#define TAPA_CHECK_FX_WIDTH(mine, theirs)                               \
  EXPECT_EQ(int{decltype(mine)::width}, int{decltype(theirs)::width})   \
      << #mine " width";                                                \
  EXPECT_EQ(int{decltype(mine)::iwidth}, int{decltype(theirs)::iwidth}) \
      << #mine " iwidth"
  TAPA_CHECK_FX_WIDTH(a + b, va + vb);
  TAPA_CHECK_FX_WIDTH(a - b, va - vb);
  TAPA_CHECK_FX_WIDTH(a * b, va * vb);
  TAPA_CHECK_FX_WIDTH(a / b, va / vb);
  TAPA_CHECK_FX_WIDTH(b + a, vb + va);
  TAPA_CHECK_FX_WIDTH(b - a, vb - va);
  TAPA_CHECK_FX_WIDTH(b / a, vb / va);
#undef TAPA_CHECK_FX_WIDTH
}

template <int W1, int I1, bool S1, int W2, int I2, bool S2>
void CheckFixedArithmetic(uint64_t& state, int rounds) {
  using lhs = fx<W1, I1, S1, q_mode::trn, o_mode::wrap, 0>;
  using rhs = fx<W2, I2, S2, q_mode::trn, o_mode::wrap, 0>;
  for (int r = 0; r < rounds; ++r) {
    typename lhs::mine a;
    typename lhs::theirs va;
    typename rhs::mine b;
    typename rhs::theirs vb;
    SetPair(a, va, RandomBits(W1, state));
    SetPair(b, vb, RandomBits(W2, state));
    const std::string where = std::to_string(W1) + "," + std::to_string(I1) +
                              " op " + std::to_string(W2) + "," +
                              std::to_string(I2) + " round " +
                              std::to_string(r);

    EXPECT_EQ(TapaBits((a + b).V), VendorBits((va + vb).V)) << "add " << where;
    EXPECT_EQ(TapaBits((a - b).V), VendorBits((va - vb).V)) << "sub " << where;
    EXPECT_EQ(TapaBits((a * b).V), VendorBits((va * vb).V)) << "mul " << where;
    if (!b.is_zero()) {
      EXPECT_EQ(TapaBits((a / b).V), VendorBits((va / vb).V))
          << "div " << where;
    }
    EXPECT_EQ(a == b, va == vb) << "eq " << where;
    EXPECT_EQ(a < b, va < vb) << "lt " << where;
    EXPECT_EQ(a <= b, va <= vb) << "le " << where;
    EXPECT_EQ(a > b, va > vb) << "gt " << where;
    EXPECT_EQ(a >= b, va >= vb) << "ge " << where;
  }
}

TEST(VendorParity, FixedArithmetic) {
  uint64_t state = 0x44;
  CheckFixedArithmetic<12, 6, false, 12, 6, false>(state, 40);
  CheckFixedArithmetic<12, 6, true, 12, 6, true>(state, 40);
  CheckFixedArithmetic<12, 6, true, 10, 2, false>(state, 40);
  CheckFixedArithmetic<12, 6, false, 10, 2, true>(state, 40);
  CheckFixedArithmetic<16, 16, true, 8, 0, true>(state, 40);
  CheckFixedArithmetic<32, 8, false, 18, 4, true>(state, 20);
  CheckFixedArithmetic<24, 30, true, 20, -2, false>(state, 20);
}

// Mixing a plain integer into fixed-point arithmetic: the vendor widens as
// if the integer were a fixed-point value of its C type's width, all of it
// above the binary point, so an `int` costs 32 bits of result even when the
// value is 2. Result widths and values both have to agree, or a design that
// scales by a literal gets different hardware.
TEST(VendorParity, FixedArithmeticWithNativeIntegers) {
  ufixed<32, 8, q_mode::rnd, o_mode::sat> a = 1.5;
  ap_ufixed<32, 8, AP_RND, AP_SAT> va = 1.5;
  fixed<18, 4> b = -2.25;
  ap_fixed<18, 4> vb = -2.25;

#define TAPA_CHECK_MIXED(expr_mine, expr_theirs)                  \
  EXPECT_EQ(int{decltype(expr_mine)::width},                      \
            int{decltype(expr_theirs)::width})                    \
      << #expr_mine " width";                                     \
  EXPECT_EQ(int{decltype(expr_mine)::iwidth},                     \
            int{decltype(expr_theirs)::iwidth})                   \
      << #expr_mine " iwidth";                                    \
  EXPECT_EQ(TapaBits((expr_mine).V), VendorBits((expr_theirs).V)) \
      << #expr_mine " value"

  TAPA_CHECK_MIXED(a * 2, va * 2);
  TAPA_CHECK_MIXED(2 * a, 2 * va);
  TAPA_CHECK_MIXED(a + 1, va + 1);
  TAPA_CHECK_MIXED(1 + a, 1 + va);
  TAPA_CHECK_MIXED(a - 1, va - 1);
  TAPA_CHECK_MIXED(a / 2, va / 2);
  TAPA_CHECK_MIXED(b * 2, vb * 2);
  TAPA_CHECK_MIXED(b + 3, vb + 3);
  TAPA_CHECK_MIXED(b - 3, vb - 3);
  // The C type's width is what counts, not the literal's value.
  TAPA_CHECK_MIXED(a * static_cast<short>(2), va * static_cast<short>(2));
  TAPA_CHECK_MIXED(a * static_cast<char>(2), va * static_cast<char>(2));
  TAPA_CHECK_MIXED(a * 2LL, va * 2LL);
  TAPA_CHECK_MIXED(a * 2u, va * 2u);
  // An arbitrary-width integer keeps its own width.
  TAPA_CHECK_MIXED(a * u<4>(3), va * ap_uint<4>(3));
  TAPA_CHECK_MIXED(b * i<5>(-3), vb * ap_int<5>(-3));
#undef TAPA_CHECK_MIXED
}

TEST(VendorParity, FixedCompoundAssignmentAndNativeComparison) {
  ufixed<32, 8, q_mode::rnd, o_mode::sat> a = 1.5;
  ap_ufixed<32, 8, AP_RND, AP_SAT> va = 1.5;
  ufixed<32, 8, q_mode::rnd, o_mode::sat> b = 1.0;
  ap_ufixed<32, 8, AP_RND, AP_SAT> vb = 1.0;

  b += a;
  vb += va;
  EXPECT_EQ(TapaBits(b.V), VendorBits(vb.V)) << "+=";
  b *= 2;
  vb *= 2;
  EXPECT_EQ(TapaBits(b.V), VendorBits(vb.V)) << "*=";
  b -= a;
  vb -= va;
  EXPECT_EQ(TapaBits(b.V), VendorBits(vb.V)) << "-=";
  b /= 2;
  vb /= 2;
  EXPECT_EQ(TapaBits(b.V), VendorBits(vb.V)) << "/=";

  for (const double d : {0.0, 1.0, 1.5, 2.0, -1.0}) {
    EXPECT_EQ(a == d, va == d) << "== " << d;
    EXPECT_EQ(a != d, va != d) << "!= " << d;
    EXPECT_EQ(a < d, va < d) << "< " << d;
    EXPECT_EQ(a <= d, va <= d) << "<= " << d;
    EXPECT_EQ(a > d, va > d) << "> " << d;
    EXPECT_EQ(a >= d, va >= d) << ">= " << d;
  }
  for (const int n : {0, 1, 2, -1}) {
    EXPECT_EQ(a > n, va > n) << "> " << n;
    EXPECT_EQ(a == n, va == n) << "== " << n;
  }
}

// Casting a fixed-point value to an integer truncates toward zero, and does
// so whatever the type's own quantization mode says -- the mode governs
// conversions between fixed-point types, not this.
TEST(VendorParity, FixedToInteger) {
  const double values[] = {0.0,  2.75, -2.75, 2.25, -2.25, 2.5,
                           -2.5, 7.0,  -7.0,  0.5,  -0.5};
  for (const double d : values) {
    EXPECT_EQ((fixed<16, 8>(d)).to_int64(), (ap_fixed<16, 8>(d)).to_int64())
        << "trn " << d;
    EXPECT_EQ((fixed<16, 8, q_mode::rnd>(d)).to_int64(),
              (ap_fixed<16, 8, AP_RND>(d)).to_int64())
        << "rnd " << d;
    EXPECT_EQ((ufixed<16, 8>(d < 0 ? -d : d)).to_int64(),
              (ap_ufixed<16, 8>(d < 0 ? -d : d)).to_int64())
        << "unsigned " << d;
  }
}

TEST(VendorParity, FixedFromAndToNative) {
  const double reals[] = {0.0,
                          1.0,
                          -1.0,
                          0.5,
                          -0.5,
                          0.25,
                          -0.25,
                          1.0 / 3,
                          -7.75,
                          1023.5,
                          -1023.5,
                          1e9,
                          -1e9,
                          1e-9,
                          -1e-9,
                          3.14159265358979,
                          -2.718281828459045,
                          65535.999,
                          -65536.001};
  for (const double d : reals) {
#define TAPA_CHECK_FROM_DOUBLE(W, I, S, Q, O)                       \
  {                                                                 \
    using P = fx<W, I, S, q_mode::Q, o_mode::O, 0>;                 \
    const P::mine mine(d);                                          \
    const P::theirs theirs(d);                                      \
    EXPECT_EQ(TapaBits(mine.V), VendorBits(theirs.V))               \
        << "from double " << d << " into " #W "," #I " " #Q " " #O; \
    EXPECT_DOUBLE_EQ(mine.to_double(), theirs.to_double())          \
        << "to double " << d;                                       \
  }
    TAPA_CHECK_FROM_DOUBLE(32, 16, true, trn, wrap)
    TAPA_CHECK_FROM_DOUBLE(32, 16, true, rnd, sat)
    TAPA_CHECK_FROM_DOUBLE(32, 8, false, rnd, sat)
    TAPA_CHECK_FROM_DOUBLE(16, 8, true, rnd_conv, sat_sym)
    TAPA_CHECK_FROM_DOUBLE(12, 20, true, rnd, wrap)
    TAPA_CHECK_FROM_DOUBLE(12, -2, true, rnd, sat)
    TAPA_CHECK_FROM_DOUBLE(64, 32, true, rnd, sat)
#undef TAPA_CHECK_FROM_DOUBLE
  }

  const int64_t ints[] = {0,    1,     -1,     7,       -7,        255,
                          -256, 65535, -65536, 1 << 20, -(1 << 20)};
  for (const int64_t n : ints) {
#define TAPA_CHECK_FROM_INT(W, I, S, Q, O)                       \
  {                                                              \
    using P = fx<W, I, S, q_mode::Q, o_mode::O, 0>;              \
    const P::mine mine(n);                                       \
    const P::theirs theirs(n);                                   \
    EXPECT_EQ(TapaBits(mine.V), VendorBits(theirs.V))            \
        << "from int " << n << " into " #W "," #I " " #Q " " #O; \
  }
    TAPA_CHECK_FROM_INT(32, 16, true, trn, wrap)
    TAPA_CHECK_FROM_INT(32, 16, true, rnd, sat)
    TAPA_CHECK_FROM_INT(12, 6, false, rnd, sat)
    TAPA_CHECK_FROM_INT(16, 24, true, trn, wrap)
#undef TAPA_CHECK_FROM_INT
  }
}

#endif  // TAPA_VENDOR_HEADERS_AVAILABLE

}  // namespace
}  // namespace tapa
