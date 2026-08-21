// Semantics contract for tapa::fixed/ufixed: the portable stand-ins for the
// vendor fixed-point types. Every expectation is hand-computed from the
// scaling rule (value == V * 2^-(W - I)) and the documented quantization and
// overflow modes, so it says what the type is supposed to do rather than
// what it happens to do. tapa/vendor_parity_test.cpp is the other half:
// it asks ap_fixed the same questions when the vendor headers are installed.

#include "tapa/host/fixed.h"

#include <sstream>
#include <string>

#include "gtest/gtest.h"

namespace tapa {
namespace {

TEST(FixedTest, RawBitsAreTheScaledValue) {
  ufixed<8, 4> a = 1.5;
  EXPECT_EQ(a.V, u<8>(24));  // 1.5 * 2^4
  EXPECT_DOUBLE_EQ(a.to_double(), 1.5);

  fixed<8, 4> b = -1.5;
  EXPECT_EQ(b.V, i<8>(-24));
  EXPECT_DOUBLE_EQ(b.to_double(), -1.5);

  // A whole type has no fractional bits, a pure fraction no integer ones.
  fixed<8, 8> whole = -3;
  EXPECT_EQ(whole.V, i<8>(-3));
  ufixed<8, 0> frac = 0.5;
  EXPECT_EQ(frac.V, u<8>(128));
  EXPECT_DOUBLE_EQ(frac.to_double(), 0.5);
}

TEST(FixedTest, ShapeConstants) {
  EXPECT_EQ((ufixed<32, 8>::width), 32);
  EXPECT_EQ((ufixed<32, 8>::iwidth), 8);
  EXPECT_EQ((ufixed<32, 8>::fwidth), 24);
  EXPECT_FALSE((ufixed<32, 8>::is_signed));
  EXPECT_TRUE((fixed<32, 8>::is_signed));
  // The binary point may sit outside the word entirely.
  EXPECT_EQ((fixed<8, 12>::fwidth), -4);
  EXPECT_EQ((fixed<8, -4>::fwidth), 12);
}

// Quantization discards fractional bits the target cannot hold. Each mode is
// checked on the same two values: +0.5 ulp and -0.5 ulp of the target, which
// is where the modes differ.
TEST(FixedTest, QuantizationModes) {
  // Source: 1/32 and -1/32 exactly. Target ulp is 1/16, so both are ties.
  const double up = 0.03125;
  const double down = -0.03125;

  EXPECT_EQ((fixed<8, 4, q_mode::trn>(up).V), i<8>(0));
  EXPECT_EQ((fixed<8, 4, q_mode::trn>(down).V), i<8>(-1));  // toward -inf

  EXPECT_EQ((fixed<8, 4, q_mode::trn_zero>(up).V), i<8>(0));
  EXPECT_EQ((fixed<8, 4, q_mode::trn_zero>(down).V), i<8>(0));

  EXPECT_EQ((fixed<8, 4, q_mode::rnd>(up).V), i<8>(1));  // ties toward +inf
  EXPECT_EQ((fixed<8, 4, q_mode::rnd>(down).V), i<8>(0));

  EXPECT_EQ((fixed<8, 4, q_mode::rnd_zero>(up).V), i<8>(0));
  EXPECT_EQ((fixed<8, 4, q_mode::rnd_zero>(down).V), i<8>(0));

  EXPECT_EQ((fixed<8, 4, q_mode::rnd_min_inf>(up).V), i<8>(0));
  EXPECT_EQ((fixed<8, 4, q_mode::rnd_min_inf>(down).V), i<8>(-1));

  EXPECT_EQ((fixed<8, 4, q_mode::rnd_inf>(up).V), i<8>(1));
  EXPECT_EQ((fixed<8, 4, q_mode::rnd_inf>(down).V), i<8>(-1));

  // Ties to even: 0.5 ulp above 0 goes to 0, above 1 goes to 2.
  EXPECT_EQ((fixed<8, 4, q_mode::rnd_conv>(up).V), i<8>(0));
  EXPECT_EQ((fixed<8, 4, q_mode::rnd_conv>(0.09375).V), i<8>(2));  // 1.5 ulp

  // Away from a tie every rounding mode agrees.
  EXPECT_EQ((fixed<8, 4, q_mode::rnd>(0.05).V), i<8>(1));
  EXPECT_EQ((fixed<8, 4, q_mode::rnd_conv>(0.05).V), i<8>(1));
  EXPECT_EQ((fixed<8, 4, q_mode::trn>(0.05).V), i<8>(0));
}

TEST(FixedTest, OverflowModes) {
  // A 4-bit whole type holds -8..7 signed, 0..15 unsigned.
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::sat>(100).V), i<4>(7));
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::sat>(-100).V), i<4>(-8));
  // Symmetric saturation gives up the extra negative value.
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::sat_sym>(-100).V), i<4>(-7));
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::sat_zero>(100).V), i<4>(0));
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::sat_zero>(-100).V), i<4>(0));
  // Wrapping keeps the low bits: 100 mod 16 == 4.
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::wrap>(100).V), i<4>(4));
  EXPECT_EQ((ufixed<4, 4, q_mode::trn, o_mode::sat>(100).V), u<4>(15));
  EXPECT_EQ((ufixed<4, 4, q_mode::trn, o_mode::sat>(-1).V), u<4>(0));
  EXPECT_EQ((ufixed<4, 4, q_mode::trn, o_mode::wrap>(100).V), u<4>(4));
  // In range, the mode does not matter.
  EXPECT_EQ((fixed<4, 4, q_mode::trn, o_mode::sat>(5).V), i<4>(5));
}

TEST(FixedTest, ArithmeticWidensSoResultsAreExact) {
  ufixed<8, 4> a = 1.5;
  ufixed<8, 4> b = 2.25;

  // The product is exact: W1 + W2 bits, I1 + I2 above the point.
  const auto product = a * b;
  EXPECT_EQ(decltype(product)::width, 16);
  EXPECT_EQ(decltype(product)::iwidth, 8);
  EXPECT_DOUBLE_EQ(product.to_double(), 3.375);

  // A sum takes one more integer bit than the wider operand.
  const auto sum = a + b;
  EXPECT_EQ(decltype(sum)::width, 9);
  EXPECT_EQ(decltype(sum)::iwidth, 5);
  EXPECT_DOUBLE_EQ(sum.to_double(), 3.75);

  // Subtraction is always signed, even between unsigned operands.
  const auto difference = a - b;
  EXPECT_TRUE(decltype(difference)::is_signed);
  EXPECT_DOUBLE_EQ(difference.to_double(), -0.75);

  EXPECT_DOUBLE_EQ((-a).to_double(), -1.5);
}

TEST(FixedTest, MixedWidthsAlignTheBinaryPoint) {
  ufixed<8, 4> coarse = 1.5;    // ulp 1/16
  ufixed<12, 4> fine = 1.0625;  // ulp 1/256
  const auto sum = coarse + fine;
  EXPECT_EQ(decltype(sum)::fwidth, 8);  // the finer of the two
  EXPECT_DOUBLE_EQ(sum.to_double(), 2.5625);
}

TEST(FixedTest, DivisionTruncatesTowardZero) {
  fixed<16, 8> a = 7.5;
  fixed<16, 8> b = 2.0;
  EXPECT_DOUBLE_EQ((a / b).to_double(), 3.75);

  fixed<8, 8> odd = -7;
  fixed<8, 8> two = 2;
  // -7 / 2 is -3.5, and the result type here has no fractional bits.
  EXPECT_DOUBLE_EQ((odd / two).to_double(), -3.0);
}

TEST(FixedTest, ComparisonCrossesTypes) {
  ufixed<8, 4> a = 1.5;
  ufixed<12, 6> b = 1.5;
  EXPECT_TRUE(a == b);
  EXPECT_FALSE(a < b);
  EXPECT_TRUE(a <= b);

  fixed<8, 4> c = -1.5;
  fixed<8, 4> d = 1.5;
  EXPECT_TRUE(c < d);
  EXPECT_TRUE(d > c);
  EXPECT_TRUE(c != d);
}

TEST(FixedTest, BitAndSliceAccessReachTheRawPattern) {
  ufixed<32, 8> a = 1.5;
  EXPECT_EQ(a.V, u<32>(3u << 23));  // 1.5 * 2^24
  EXPECT_EQ(a(31, 24), u<32>(1));   // the integer part
  EXPECT_TRUE(a.get_bit(23));
  EXPECT_FALSE(a.get_bit(0));

  a.V = 0;
  a(31, 24) = 3;
  EXPECT_DOUBLE_EQ(a.to_double(), 3.0);
}

TEST(FixedTest, RoundTripsThroughNative) {
  EXPECT_DOUBLE_EQ((fixed<32, 16>(-2.5)).to_double(), -2.5);
  EXPECT_DOUBLE_EQ((fixed<32, 16>(0.0)).to_double(), 0.0);
  EXPECT_DOUBLE_EQ((ufixed<32, 16>(1024.25)).to_double(), 1024.25);
  EXPECT_EQ((fixed<16, 16>(-1234)).to_int64(), -1234);
  // The fractional part is discarded toward ZERO, whatever the type's own
  // quantization mode says: that mode governs conversions between
  // fixed-point types, not the cast to an integer.
  EXPECT_EQ((fixed<16, 8>(-2.5)).to_int64(), -2);
  EXPECT_EQ((fixed<16, 8>(2.5)).to_int64(), 2);
  EXPECT_EQ((fixed<16, 8, q_mode::rnd>(-2.75)).to_int64(), -2);
}

TEST(FixedTest, StreamsAsItsValue) {
  std::ostringstream os;
  os << ufixed<8, 4>(1.5);
  EXPECT_EQ(os.str(), "1.5");
}

// `x = 0` has to pick the literal over a conversion to the fixed type, or
// it is ambiguous against the implicit copy assignment. Designs reset
// buffers this way.
TEST(FixedTest, AssignsFromNativeValues) {
  ufixed<16, 8> a = 1.5;
  a = 0;
  EXPECT_TRUE(a.is_zero());
  a = 2;
  EXPECT_DOUBLE_EQ(a.to_double(), 2.0);
  a = 0.25;
  EXPECT_DOUBLE_EQ(a.to_double(), 0.25);
  a = 3.5f;
  EXPECT_DOUBLE_EQ(a.to_double(), 3.5);

  ufixed<16, 8> b;
  b = a;  // still the ordinary copy
  EXPECT_DOUBLE_EQ(b.to_double(), 3.5);
  fixed<24, 12> c;
  c = a;  // and a conversion between shapes
  EXPECT_DOUBLE_EQ(c.to_double(), 3.5);
}

// Mixing a plain number in: the vendor widens as if the integer were a
// fixed-point value of its C type's width, so `a * 2` costs 32 bits of
// result. Arithmetic with a `double` is deliberately absent, here and in
// the vendor.
TEST(FixedTest, MixesWithPlainNumbers) {
  ufixed<32, 8> a = 1.5;
  const auto scaled = a * 2;
  EXPECT_EQ(decltype(scaled)::width, 64);   // 32 + sizeof(int) * 8
  EXPECT_EQ(decltype(scaled)::iwidth, 40);  // 8 + 32
  EXPECT_DOUBLE_EQ(scaled.to_double(), 3.0);
  EXPECT_DOUBLE_EQ((2 * a).to_double(), 3.0);
  // An arbitrary-width integer keeps its own width instead.
  const auto by_u4 = a * u<4>(3);
  EXPECT_EQ(decltype(by_u4)::width, 36);
  EXPECT_DOUBLE_EQ(by_u4.to_double(), 4.5);

  a += 1;
  EXPECT_DOUBLE_EQ(a.to_double(), 2.5);
  a *= 2;
  EXPECT_DOUBLE_EQ(a.to_double(), 5.0);

  EXPECT_TRUE(a > 4);
  EXPECT_TRUE(a == 5.0);
  EXPECT_FALSE(a < 5);
}

TEST(FixedTest, DefaultIsZero) {
  const ufixed<16, 8> a;
  EXPECT_TRUE(a.is_zero());
  EXPECT_DOUBLE_EQ(a.to_double(), 0.0);
}

// Object size follows the raw integer, which is the vendor's rule too.
static_assert(sizeof(ufixed<32, 8>) == 4);
static_assert(sizeof(fixed<18, 4>) == 4);
static_assert(sizeof(fixed<96, 32>) == 16);
static_assert(alignof(fixed<96, 32>) == 16);

}  // namespace
}  // namespace tapa
