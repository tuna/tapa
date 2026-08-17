// Semantics contract for tapa::u/i: the portable stand-ins for the vendor
// arbitrary-precision integers. Every expectation below is hand-computed
// from the vendor rules (widened arithmetic, truncating assignment,
// C-conversion comparisons, pattern equality at max(W, 64), C-division,
// reversing shifts with 32-bit-truncated counts).

#include "tapa/base/int.h"
#include "tapa/base/util.h"

#include <gtest/gtest.h>

#include <cmath>
#include <cstdint>
#include <sstream>
#include <string>
#include <vector>

namespace {

TEST(IntTest, DefaultInitIsZero) {
  tapa::u<8> a;
  tapa::i<16> b;
  EXPECT_EQ(a, 0);
  EXPECT_EQ(b, 0);
}

TEST(IntTest, AssignmentTruncates) {
  tapa::u<8> a = 300;  // 300 mod 256
  EXPECT_EQ(a, 44);
  tapa::i<8> b = 200;  // wraps to -56
  EXPECT_EQ(b, -56);
  tapa::u<8> c = -1;
  EXPECT_EQ(c, 255);
  tapa::u<8> d = tapa::u<16>(0x1ff);
  EXPECT_EQ(d, 0xff);
}

TEST(IntTest, WideningKeepsValue) {
  tapa::i<8> a = -5;
  tapa::u<16> b(a);
  tapa::i<16> c(a);
  EXPECT_EQ(b, 65531);
  EXPECT_EQ(c, -5);
}

TEST(IntTest, PlusWidensToFit) {
  tapa::u<8> a = 200, b = 100;
  tapa::u<9> c = a + b;
  EXPECT_EQ(c, 300);
  tapa::i<4> d = 7, e = 1;
  tapa::i<5> f = d + e;
  EXPECT_EQ(f, 8);
}

TEST(IntTest, MixedSignednessWidens) {
  tapa::u<8> a = 100;
  tapa::i<8> b = -1;
  auto c = a + b;  // i<10>
  EXPECT_EQ(c, 99);
}

TEST(IntTest, MinusIsAlwaysSigned) {
  tapa::u<8> a = 1, b = 2;
  auto c = a - b;  // i<9>
  EXPECT_EQ(c, -1);
}

TEST(IntTest, MultWidensToFit) {
  tapa::i<4> a = -8, b = -8;
  auto c = a * b;  // i<8>
  EXPECT_EQ(c, 64);
  tapa::u<4> d = 15, e = 15;
  auto f = d * e;  // u<8>
  EXPECT_EQ(f, 225);
}

TEST(IntTest, DivisionTruncatesTowardZero) {
  tapa::i<8> a = -7, b = 2, c = 7, d = -2;
  EXPECT_EQ(a / b, -3);
  EXPECT_EQ(a % b, -1);
  EXPECT_EQ(c / d, -3);
  EXPECT_EQ(c % d, 1);
}

TEST(IntTest, DivisionOfExtremes) {
  tapa::i<8> min = -128, one = -1;
  auto q = min / one;  // needs 9 bits
  EXPECT_EQ(q, 128);
}

TEST(IntTest, DivisionByUnsignedWorks) {
  tapa::u<8> a = 7;
  tapa::i<8> b = -2;
  EXPECT_EQ(a / b, -3);  // u/i: div_w = 8+1, signed
  EXPECT_EQ(a % b, 1);
}

TEST(IntTest, NegativeShiftReverses) {
  tapa::u<8> a = 0b100;
  tapa::i<8> minus_one = -1;
  EXPECT_EQ(a << minus_one, 0b10);
  EXPECT_EQ(a >> minus_one, 0b1000);  // >> -1 shifts left
  tapa::i<8> x = -16;
  x >>= 2;  // arithmetic fill
  EXPECT_EQ(x, -4);
}

TEST(IntTest, ShiftPastWidth) {
  tapa::u<8> a = 1;
  a <<= 8;
  EXPECT_EQ(a, 0);
  tapa::i<8> b = -1;
  b >>= 100;
  EXPECT_EQ(b, -1);
}

TEST(IntTest, ShiftKeepsLeftWidth) {
  tapa::u<8> a = 0xff;
  auto b = a << 4;  // still 8 bits
  EXPECT_EQ(b, 0xf0);
}

// Mixed-signedness comparison follows C's usual arithmetic conversions with
// the declared width as the rank, which is what the vendor implements and
// says so in its own source. NOT the left operand's signedness: that is what
// this test asserted before tapa/vendor_parity_test.cpp compared the two
// directly and found them disagreeing on half its cases.
TEST(IntTest, ComparisonFollowsCConversions) {
  // Below 32 bits C promotes both operands to int, so the comparison is
  // signed whichever side the unsigned operand is on.
  tapa::i<8> s = -1;
  tapa::u<8> t = 1;
  EXPECT_TRUE(s < t);
  tapa::u<8> big = 255;
  tapa::i<8> neg = -1;
  EXPECT_TRUE(big > neg);

  // At 32 bits and above the wider-or-equal operand's signedness wins, so
  // the signed value converts and the comparison is unsigned.
  tapa::i<32> wide_neg = -1;
  tapa::u<32> wide_zero = 0;
  EXPECT_FALSE(wide_neg < wide_zero);
  EXPECT_TRUE(wide_zero < wide_neg);
  // A signed operand wider than the unsigned one keeps the comparison
  // signed, exactly as `(int64_t)-1 < 0u` does.
  tapa::i<40> wider_neg = -1;
  EXPECT_TRUE(wider_neg < wide_zero);

  // Equality compares the widened bit patterns after sign/zero-extension
  // into max(width, 64) bits: at 64 bits and wider a negative signed value
  // equals the all-ones unsigned pattern, while at narrower widths the
  // widened patterns differ (the vendor's native 64-bit path), even though
  // C would convert and call `-1 == 0xffffffffu` true.
  EXPECT_FALSE(wide_neg == tapa::u<32>(0xffffffffu));
  EXPECT_TRUE(tapa::i<64>(-1) == tapa::u<64>(~uint64_t(0)));
  EXPECT_FALSE(tapa::i<8>(-1) == tapa::u<8>(255));
  EXPECT_TRUE(tapa::i<8>(-1) == tapa::i<32>(-1));
  // The sentinel case: a negative signed value against the all-ones
  // unsigned pattern of a wider-or-equal width.
  EXPECT_TRUE(tapa::i<128>(-1) == (tapa::u<128>(0) - 1));
  EXPECT_TRUE(tapa::u<8>(255) == tapa::u<32>(255));
}

TEST(IntTest, SliceRead) {
  tapa::u<64> a = 0x0000ff00deadbeefULL;
  tapa::u<14> hi = a(63, 50);
  tapa::u<32> lo = a(31, 0);
  EXPECT_EQ(hi, 0);
  EXPECT_EQ(lo, 0xdeadbeef);
  tapa::u<18> row = a(49, 32);
  EXPECT_EQ(row, 0xff00);
  EXPECT_EQ((a.range<7, 4>()), 0xe);
}

TEST(IntTest, SliceWrite) {
  tapa::u<64> a = 0;
  a(31, 0) = 0xdeadbeef;
  EXPECT_EQ(a, 0xdeadbeef);
  tapa::u<14> s = 0x155;
  a(63, 50) = s;
  EXPECT_EQ(a, 0x05540000deadbeefULL);  // (0x155 << 50) | 0xdeadbeef
}

TEST(IntTest, SliceAsIndex) {
  tapa::u<64> addr = 0x10;
  int array[16];
  array[addr(17, 1)] = 0x42;  // 0x10 >> 1 = 8
  EXPECT_EQ(array[8], 0x42);
}

TEST(IntTest, BitRef) {
  tapa::u<8> a = 0;
  a[3] = 1;
  EXPECT_EQ(a, 8);
  EXPECT_EQ(a[3], 1);
  a[3] = tapa::u<8>(0);
  EXPECT_EQ(a, 0);
  tapa::i<8> b = -1;
  EXPECT_EQ(b[7], 1);
}

TEST(IntTest, Reductions) {
  tapa::u<8> ones = 0xff;
  EXPECT_TRUE(ones.and_reduce());
  EXPECT_FALSE(ones.nand_reduce());
  tapa::u<8> mixed = 0b101;
  EXPECT_EQ(mixed.xor_reduce(), 0);
  tapa::u<8> odd = 0b111;
  EXPECT_EQ(odd.xor_reduce(), 1);
  EXPECT_TRUE(tapa::u<8>(0x10).or_reduce());
  EXPECT_TRUE(tapa::u<8>(0).nor_reduce());
  EXPECT_TRUE(tapa::u<8>(0b1111).xnor_reduce());
}

TEST(IntTest, LfsrPattern) {
  // The bandwidth test taps xor of masked feedback into bit 0.
  tapa::u<16> v = 0x2d;
  bool feedback = (v & tapa::u<16>(0x2d)).xor_reduce();
  v >>= 1;
  v.set_bit(0, feedback);
  EXPECT_EQ(v, 0x16);
}

TEST(IntTest, Concatenation) {
  tapa::u<8> hi = 3, lo = 5;
  auto cat = (hi, lo);
  tapa::u<16> v = cat;
  EXPECT_EQ(v, 0x0305);
  cat = tapa::u<16>(0x0102);
  EXPECT_EQ(hi, 1);
  EXPECT_EQ(lo, 2);
  tapa::u<4> a = 0xf, b = 0x0;
  tapa::u<8> w = tapa::concat(a, b);
  EXPECT_EQ(w, 0xf0);
}

TEST(IntTest, CompoundTruncates) {
  tapa::u<8> x = 255;
  x += 1;
  EXPECT_EQ(x, 0);
  tapa::u<8> y = 0;
  y -= 1;
  EXPECT_EQ(y, 255);
}

TEST(IntTest, IncrementDecrement) {
  tapa::u<8> x = 254;
  EXPECT_EQ(++x, 255);
  EXPECT_EQ(x++, 255);
  EXPECT_EQ(x, 0);
  tapa::i<8> y = 0;
  EXPECT_EQ(--y, -1);
}

TEST(IntTest, Unary) {
  EXPECT_EQ(-tapa::u<8>(1), -1);
  EXPECT_EQ(~tapa::u<8>(0), 255);
  EXPECT_TRUE(!tapa::u<8>(0));
  EXPECT_EQ(+tapa::u<8>(7), 7);
  EXPECT_EQ((-tapa::i<8>(-128)).to_int64(), 128);  // needs i<9>
}

TEST(IntTest, Conversions) {
  EXPECT_EQ(static_cast<int>(tapa::u<16>(300)), 300);
  EXPECT_EQ(static_cast<int64_t>(tapa::i<8>(-1)), -1);
  EXPECT_EQ(tapa::u<8>(300).to_uint64(), 44);
  EXPECT_EQ(tapa::i<4>(-8).to_int64(), -8);
  EXPECT_EQ(tapa::u<8>(100).length(), 8);
  EXPECT_EQ(std::to_string(static_cast<int>(tapa::u<8>(100))), "100");
}

TEST(IntTest, Reverse) {
  tapa::u<8> a = 1;
  EXPECT_EQ(a.reverse(), 0x80);
}

TEST(IntTest, FloatInterop) {
  EXPECT_EQ(tapa::i<8>(3.7), 3);
  EXPECT_EQ(tapa::i<8>(-3.7), -3);
  EXPECT_DOUBLE_EQ(tapa::u<8>(100) * 2.0, 200.0);
  EXPECT_DOUBLE_EQ(1.5 + tapa::i<4>(2), 3.5);
  EXPECT_DOUBLE_EQ(tapa::u<100>(1).to_double() * 0, 0.0);
}

TEST(IntTest, WideArithmeticBeyond64) {
  tapa::u<100> a = 1;
  a <<= 99;
  EXPECT_DOUBLE_EQ(a.to_double(), 6.3382530011411470e29);
  tapa::u<100> b = a - 1;  // 99 ones
  EXPECT_FALSE(b.and_reduce());
  EXPECT_TRUE(tapa::u<100>(~tapa::u<100>(0)).and_reduce());
  tapa::u<128> m = tapa::u<64>(0xffffffffffffffffULL);
  tapa::u<128> sq = m * m;
  EXPECT_EQ(tapa::u<64>(sq(127, 64)), 0xfffffffffffffffeULL);
  EXPECT_EQ(tapa::u<64>(sq(63, 0)), 1);
}

TEST(IntTest, WideSignedDivision) {
  tapa::i<100> a = -1;
  a <<= 80;  // -2^80
  tapa::i<100> q = a / tapa::i<100>(-7);
  EXPECT_DOUBLE_EQ(q.to_double(), 1.7270368851637560e23);  // floor(2^80 / 7)
  EXPECT_EQ(a % tapa::i<100>(-7), -4);                     // -(2^80 mod 7)
}

TEST(IntTest, SubtractionBorrowAcrossMaxLimb) {
  // b[li] + borrow wraps at UINT64_MAX; the outgoing borrow must survive.
  tapa::u<128> b = ~tapa::u<128>(0);
  b -= tapa::u<128>(0xfffffffffffffffeULL);  // b = 2^128 - 2^64 + 1
  tapa::u<128> a = 0;
  auto d = a - b;  // i<129>: -(2^128 - 2^64 + 1)
  EXPECT_EQ(d + b, 0);
  tapa::u<129> big = 1;
  big <<= 128;                                // 2^128
  EXPECT_EQ(big % b, 0xffffffffffffffffULL);  // 2^64 - 1
}

TEST(IntTest, UnsignedShiftCountWithTopBitIsPositive) {
  tapa::i<16> a = -2;
  tapa::u<8> sh = 128;    // top bit set, but unsigned: a large positive count
  EXPECT_EQ(a << sh, 0);  // 128 >= 16
}

TEST(IntTest, ShiftCountTruncatesTo32Bits) {
  // The vendor reads shift counts through its 32-bit accessors, so a wide
  // count truncates: 2**64 shifts by 0, not by "a lot".
  tapa::u<8> one = 1;
  tapa::i<66> huge = 1;
  huge <<= 64;  // 2^64
  EXPECT_EQ(one << huge, one);
  // A count that is not a multiple of 2**32 keeps its low 32 bits.
  tapa::i<66> three = huge + 3;
  EXPECT_EQ(one << three, 8);
  tapa::i<65> nhuge = -1;
  nhuge <<= 64;  // -(2^64): >> reverses to a left shift by (2^64 mod 2**32)
  tapa::u<8> v = 128;
  EXPECT_EQ(v >> nhuge, v);
}

TEST(IntTest, FloatConstructionBeyond64Bits) {
  tapa::i<100> a = -3.7;
  EXPECT_EQ(a.to_int64(), -3);
  tapa::u<100> b = std::ldexp(1.0, 80);      // 2^80
  EXPECT_EQ(tapa::u<32>(b(95, 64)), 65536);  // bit 80
}

TEST(IntTest, ConcatWideningFromNarrowSource) {
  // Assigning a narrow source must zero/sign-extend, not read past it.
  tapa::u<65> hi, lo;
  (hi, lo) = tapa::u<8>(1);
  EXPECT_EQ(hi, 0);
  EXPECT_EQ(lo, 1);
  (hi, lo) = tapa::i<8>(-1);  // sign-extends across both halves
  EXPECT_EQ(hi.to_uint64(), 0xffffffffffffffffULL);
  EXPECT_EQ(hi[64], 1);
  EXPECT_EQ(lo.to_uint64(), 0xffffffffffffffffULL);
  EXPECT_EQ(lo[64], 1);
}

TEST(IntTest, RangeWriteClearsAllSelectedBits) {
  tapa::u<100> a = ~tapa::u<100>(0);
  a(99, 0) = tapa::u<100>(0);
  EXPECT_FALSE(a.or_reduce());
  a(99, 64) = tapa::u<8>(3);  // only 8 bits; the rest clear
  EXPECT_EQ(tapa::u<64>(a(63, 0)), 0);
  EXPECT_EQ(tapa::u<64>(a(99, 64)), 3);
}

namespace {

// Mirrors the slice of ap_int_base's interface the vendor bridge uses, so
// the >64-bit path is covered without depending on the vendor headers.
template <int W, bool S>
class FakeVendorInt {
 public:
  explicit FakeVendorInt(std::vector<bool> bits) : bits_(std::move(bits)) {}
  int length() const { return W; }
  bool test(int i) const { return i < int(bits_.size()) && bits_[i]; }
  bool sign() const { return S && test(W - 1); }
  uint64_t to_uint64() const {
    uint64_t v = 0;
    for (int b = 0; b < 64 && b < W; ++b)
      if (test(b)) v |= uint64_t(1) << b;
    // The vendor's 64-bit accessor sign-extends a narrow signed value.
    if (S && W < 64 && sign()) {
      for (int b = W; b < 64; ++b) v |= uint64_t(1) << b;
    }
    return v;
  }

 private:
  std::vector<bool> bits_;
};

}  // namespace

TEST(IntTest, WideVendorValuesCrossTheBridgeWhole) {
  // to_uint64() is a 64-bit accessor; a 128-bit vendor source must not be
  // truncated to its low limb on the way in.
  FakeVendorInt<128, false> src(std::vector<bool>(128, true));
  tapa::u<128> got = src;
  EXPECT_TRUE(got.and_reduce());

  // A narrow signed vendor source still sign-extends.
  std::vector<bool> neg(96, true);
  FakeVendorInt<96, true> negative(neg);
  tapa::i<128> widened = negative;
  EXPECT_EQ(widened.to_int64(), -1);

  // <= 64 bits keeps the existing to_uint64() path.
  FakeVendorInt<32, false> narrow(std::vector<bool>(32, true));
  EXPECT_EQ(tapa::u<32>(narrow), 0xffffffffu);

  // A signed source that fits 64 bits must still sign-extend into a wider
  // tapa target: the limbs above bit 63 cannot come from to_uint64().
  FakeVendorInt<32, true> neg32(std::vector<bool>(32, true));
  tapa::i<100> widened32 = neg32;
  EXPECT_EQ(widened32, tapa::i<100>(-1));
}

TEST(IntTest, BitAssignmentUsesTheWholeSourceValue) {
  // ap_bit_ref assigns `val != 0`; taking bit 0 would clear the bit for an
  // even non-zero source such as a 2-bit mask.
  tapa::u<8> flags = 0;
  tapa::u<4> hit = 2;
  flags[3] = hit;
  EXPECT_EQ(flags, 8);
  tapa::u<4> zero = 0;
  flags[3] = zero;
  EXPECT_EQ(flags, 0);
}

TEST(IntTest, SlicesWiderThan64BitsKeepEveryBit) {
  // The vendor's ap_range_ref converts to ap_int_base<parent width>, so a
  // >64-bit slice must not come back with its high bits zeroed; a silent
  // truncation here would make CPU simulation disagree with synthesis.
  tapa::u<512> mem = 0;
  mem(255, 0) = ~tapa::u<256>(0);
  tapa::u<256> fifo = mem(255, 0);
  EXPECT_TRUE(fifo.and_reduce());

  // Same through the const reader and the compile-time-width API.
  const tapa::u<512> frozen = mem;
  EXPECT_TRUE(tapa::u<256>(frozen(255, 0)).and_reduce());
  EXPECT_TRUE((mem.range<255, 0>().and_reduce()));

  // ... and through a proxy-to-proxy copy, same width and across widths.
  tapa::u<512> dst = 0;
  dst(255, 0) = mem(255, 0);
  EXPECT_TRUE(tapa::u<256>(dst(255, 0)).and_reduce());
  tapa::u<300> narrow = 0;
  narrow(255, 0) = mem(255, 0);
  EXPECT_TRUE(tapa::u<256>(narrow(255, 0)).and_reduce());
}

TEST(IntTest, ToUint64SignExtendsLikeTheVendor) {
  // ap_int_base::to_uint64() is (ap_ulong)(V) on a signed storage member,
  // so it sign-extends and agrees with to_int64().
  EXPECT_EQ(tapa::i<8>(-1).to_uint64(), 0xffffffffffffffffULL);
  EXPECT_EQ(tapa::i<8>(-1).to_int64(), -1);
  EXPECT_EQ(tapa::u<8>(0xff).to_uint64(), 0xffULL);
}

TEST(IntTest, ProxyToProxyAssignment) {
  tapa::u<8> a = 0, b = 0xff;
  a[0] = b[0];
  EXPECT_EQ(a, 1);
  tapa::u<8> c = 0, d = 0xab;
  c(3, 0) = d(3, 0);
  EXPECT_EQ(c, 0xb);
  tapa::u<8> e = 0, f = 0, g = 0, h = 0xab;
  (e, f) = (g, h);
  EXPECT_EQ(f, 0xab);
  EXPECT_EQ(e, 0);
}

TEST(IntTest, ConstAccessIsReadOnly) {
  const tapa::u<8> x = 5;
  EXPECT_EQ(x[0], 1);
  EXPECT_EQ(x[2], 1);
  EXPECT_EQ(x(3, 0), 5);
  EXPECT_EQ(x.range(7, 4), 0);
  EXPECT_EQ((x.range<3, 1>()), 2);
}

TEST(IntTest, RvalueProxyReadsByValue) {
  // A proxy into a temporary would dangle; reads must return values.
  EXPECT_EQ(tapa::u<8>(1)[0], 1);
  EXPECT_EQ(tapa::u<8>(0xab)(3, 0), 0xb);
}

TEST(IntTest, ConcatOfConstOperands) {
  const tapa::u<8> a = 3, b = 5;
  auto v = (a, b);
  EXPECT_EQ(v, 0x0305);
  EXPECT_EQ(tapa::concat(a, b), 0x0305);
}

TEST(IntTest, BooleanContextUsesFullWidth) {
  tapa::u<100> x = 1;
  x <<= 99;  // only bit 99 set: above the low-64 RetType
  EXPECT_TRUE(static_cast<bool>(x));
  EXPECT_TRUE(!!x);
  EXPECT_TRUE(x.or_reduce());
}

TEST(IntTest, XorReduceAcrossPartialLimb) {
  tapa::u<65> x = 2;  // bit 64 set: outside the masked low-64 window
  EXPECT_EQ(x.xor_reduce(), 1);
  EXPECT_EQ(tapa::u<100>(3).xor_reduce(), 0);
}

TEST(IntTest, Int128Interop) {
#ifdef __SIZEOF_INT128__
  tapa::u<100> x = 1;
  x <<= 99;
  unsigned __int128 one = 1;
  auto y = x + one;  // must not fall back to native __int128 arithmetic
  EXPECT_EQ(tapa::u<64>(y(99, 64)), 0x800000000ULL);
  EXPECT_EQ(tapa::u<64>(y(63, 0)), 1);
  tapa::u<128> z(one);
  EXPECT_EQ(z, 1);
  unsigned __int128 wide_native = static_cast<unsigned __int128>(1) << 100;
  tapa::u<128> from_native(wide_native);
  EXPECT_EQ(tapa::u<64>(from_native(127, 64)), 1ULL << 36);  // bit 100
#endif
}

TEST(IntTest, Rotations) {
  tapa::u<16> x = 0x0001;
  x.lrotate(1);
  EXPECT_EQ(x, 0x0002);
  tapa::u<16> y = 0x8001;
  y.rrotate(1);
  EXPECT_EQ(y, 0xc000);
  y.rrotate(16);  // full-width rotate is identity
  EXPECT_EQ(y, 0xc000);
  // The bandwidth LFSR step: feedback (parity 0 clears bit 0), rotate right.
  tapa::u<16> v = 0x2d;
  v.set_bit(0, (v & tapa::u<16>(0x2d)).xor_reduce());  // 0x2d -> 0x2c
  v.rrotate(1);
  EXPECT_EQ(v, 0x16);
}

TEST(IntTest, OstreamAndToString) {
  EXPECT_EQ(tapa::to_string(tapa::u<8>(255)), "255");
  EXPECT_EQ(tapa::to_string(tapa::i<8>(-42)), "-42");
  std::ostringstream out;
  out << tapa::u<8>(12) << " " << tapa::i<4>(-8);
  EXPECT_EQ(out.str(), "12 -8");
  out << std::hex << tapa::u<16>(0xbeef);
  // Non-decimal bases carry the vendor's radix prefix.
  EXPECT_EQ(out.str(), "12 -80xbeef");
  tapa::u<100> wide = 1;
  wide <<= 99;
  EXPECT_EQ(tapa::to_string(wide), "633825300114114700748351602688");
  EXPECT_EQ(tapa::to_string(wide, 16), "0x8" + std::string(24, '0'));
  EXPECT_EQ(tapa::to_string(wide, 2), "0b1" + std::string(99, '0'));
  EXPECT_EQ(tapa::to_string(tapa::u<8>(0), 16), "0x0");
  EXPECT_EQ(tapa::to_string(tapa::u<8>(0), 2), "0b0");
  // A negative signed value prints '-' + prefix + magnitude, as the vendor.
  EXPECT_EQ(tapa::to_string(tapa::i<8>(-1), 16), "-0x1");
  EXPECT_EQ(tapa::to_string(tapa::i<8>(-1), 8), "-0o1");
  std::ostringstream hexed;
  hexed << std::hex << tapa::i<8>(-1);
  EXPECT_EQ(hexed.str(), "-0x1");
}

TEST(IntTest, MemberToStringMatchesVendorDefaults) {
  // The vendor member form: radix defaults to 2, sign to the type's.
  EXPECT_EQ(tapa::u<8>(5).to_string(), "0b101");
  EXPECT_EQ(tapa::i<8>(-1).to_string(), "-0b1");
  EXPECT_EQ(tapa::i<8>(-1).to_string(16), "-0x1");
  EXPECT_EQ(tapa::i<8>(-1).to_string(16, false), "0xff");
  EXPECT_EQ(tapa::u<8>(255).to_string(16, false), "0xff");
}

TEST(IntTest, IstreamParsesWithBasefieldAndPrefixes) {
  tapa::u<16> v;
  std::istringstream hexed("ff 10");
  hexed >> std::hex >> v;
  EXPECT_EQ(v, 0xff);
  hexed >> v;
  EXPECT_EQ(v, 0x10);
  // Prefixes override the stream's basefield.
  std::istringstream prefixed("0x1f 0b101 0o17 -8");
  prefixed >> v;
  EXPECT_EQ(v, 0x1f);
  prefixed >> v;
  EXPECT_EQ(v, 5);
  prefixed >> v;
  EXPECT_EQ(v, 017);
  tapa::i<16> sv;
  prefixed >> sv;
  EXPECT_EQ(sv, -8);
  // Wide values parse past 64 bits and truncate to W.
  tapa::u<100> wide;
  std::istringstream widetext("0xffffffffffffffffffffffffff");
  widetext >> wide;
  EXPECT_TRUE(wide.and_reduce());
  EXPECT_EQ(wide(99, 96), 0xf);
}

// Object size AND alignment are observable: the size is the mmap element
// stride and the stream element size, the alignment decides where the value
// sits inside any struct holding it. Both must be the vendor's, which gives
// an arbitrary-precision integer the next power of two at or above ceil(W/8)
// bytes, for both.
//
// The widths below deliberately include the ones where that rule and "one
// 64-bit limb per 64 bits" disagree -- 3 limbs (192 bits) and 5 (320) round
// up to 32 and 64 bytes, not 24 and 40. Every width the list used to carry
// was a case where the two rules happen to agree, which is how the
// difference went unnoticed. tapa/vendor_parity_test.cpp checks these
// against ap_uint/ap_int directly when the vendor headers are installed.
static_assert(sizeof(tapa::u<1>) == 1 && alignof(tapa::u<1>) == 1);
static_assert(sizeof(tapa::u<8>) == 1 && alignof(tapa::u<8>) == 1);
static_assert(sizeof(tapa::u<9>) == 2 && alignof(tapa::u<9>) == 2);
static_assert(sizeof(tapa::u<16>) == 2 && alignof(tapa::u<16>) == 2);
static_assert(sizeof(tapa::i<17>) == 4 && alignof(tapa::i<17>) == 4);
static_assert(sizeof(tapa::u<24>) == 4 && alignof(tapa::u<24>) == 4);
static_assert(sizeof(tapa::i<32>) == 4 && alignof(tapa::i<32>) == 4);
static_assert(sizeof(tapa::i<33>) == 8 && alignof(tapa::i<33>) == 8);
static_assert(sizeof(tapa::u<48>) == 8 && alignof(tapa::u<48>) == 8);
static_assert(sizeof(tapa::u<64>) == 8 && alignof(tapa::u<64>) == 8);
static_assert(sizeof(tapa::u<65>) == 16 && alignof(tapa::u<65>) == 16);
static_assert(sizeof(tapa::u<100>) == 16 && alignof(tapa::u<100>) == 16);
static_assert(sizeof(tapa::u<128>) == 16 && alignof(tapa::u<128>) == 16);
static_assert(sizeof(tapa::u<129>) == 32 && alignof(tapa::u<129>) == 32);
static_assert(sizeof(tapa::u<192>) == 32 && alignof(tapa::u<192>) == 32);
static_assert(sizeof(tapa::i<256>) == 32 && alignof(tapa::i<256>) == 32);
static_assert(sizeof(tapa::u<288>) == 64 && alignof(tapa::u<288>) == 64);
static_assert(sizeof(tapa::u<320>) == 64 && alignof(tapa::u<320>) == 64);
static_assert(sizeof(tapa::u<512>) == 64 && alignof(tapa::u<512>) == 64);
static_assert(sizeof(tapa::u<1024>) == 128 && alignof(tapa::u<1024>) == 128);

TEST(IntTest, SizeMatchesVendorLayout) {
  // Beyond the static asserts: values still behave after the narrow
  // storage change.
  tapa::u<9> x = 511;
  EXPECT_EQ(x, 511);
  x += 1;
  EXPECT_EQ(x, 0);
  tapa::i<17> y = -65536;
  EXPECT_EQ(y, -65536);
  EXPECT_EQ((-y), 65536);
  tapa::u<8> b = 0xff;
  b.lrotate(4);
  EXPECT_EQ(b, 0xff);
}

// Anything exposing to_uint64() (vendor ap_* types and their slice
// references) converts into tapa::u/i; tapa and builtin types are excluded
// so their own conversions keep full precision semantics.
struct VendorLike {
  uint64_t to_uint64() const { return 0xabcd; }
};
struct NotConvertible {};

TEST(IntTest, DuckTypedVendorBridge) {
  VendorLike v;
  tapa::u<32> x = v;
  EXPECT_EQ(x, 0xabcd);
  // tapa-to-tapa still uses the precise cross-width conversion:
  tapa::u<16> y(tapa::i<8>(-5));
  EXPECT_EQ(y, 65531);
}

TEST(IntTest, WidthofIntegration) {
  EXPECT_EQ(tapa::widthof<tapa::u<13>>(), 13);
  EXPECT_EQ(tapa::widthof<tapa::i<33>>(), 33);
  tapa::u<13> x = 5;
  EXPECT_EQ(tapa::widthof(x), 13);
}

TEST(IntTest, BoolContext) {
  tapa::u<8> a = 0;
  tapa::u<8> b = 3;
  EXPECT_FALSE(a);
  EXPECT_TRUE(b);
  if (b) SUCCEED();
}

TEST(IntTest, ComparisonWithBuiltins) {
  tapa::u<8> a = 100;
  EXPECT_TRUE(a > 99);
  EXPECT_TRUE(101 > a);
  EXPECT_TRUE(a == 100);
  tapa::i<8> b = -1;
  // `0u` is a 32-bit unsigned, so both sides convert to unsigned and the
  // -1 becomes huge -- the same answer plain C gives for `-1 < 0u`.
  EXPECT_FALSE(b < 0u);
  EXPECT_TRUE(0u < b);
}

}  // namespace
