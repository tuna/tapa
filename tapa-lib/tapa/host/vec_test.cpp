#include "tapa/host/vec.h"

#include <cmath>

#include <sstream>

#include <gtest/gtest.h>

namespace tapa {
namespace {

using Vec4 = vec_t<int, 4>;

Vec4 MakeVec4(int a, int b, int c, int d) {
  Vec4 v;
  v.set(0, a);
  v.set(1, b);
  v.set(2, c);
  v.set(3, d);
  return v;
}

::testing::AssertionResult Equals(const Vec4& actual, int a, int b, int c,
                                  int d) {
  const int expected[] = {a, b, c, d};
  for (int i = 0; i < 4; ++i) {
    if (actual[i] != expected[i]) {
      std::ostringstream os;
      os << "at [" << i << "]: expected " << expected[i] << ", got " << actual;
      return ::testing::AssertionFailure() << os.str();
    }
  }
  return ::testing::AssertionSuccess();
}

TEST(VecTest, LengthAndWidthDescribeTheLayout) {
  EXPECT_EQ(Vec4::length, 4);
  EXPECT_EQ(Vec4::width, widthof<int>() * 4);
  // `width` is the total bit width, so nesting multiplies rather than nests.
  EXPECT_EQ((vec_t<Vec4, 2>::width), widthof<int>() * 8);
}

TEST(VecTest, ElementsAreReadAndWrittenByPosition) {
  Vec4 v = MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(Equals(v, 1, 2, 3, 4));
  EXPECT_EQ(v.get(2), 3);

  v[2] = 30;
  EXPECT_EQ(v.get(2), 30);
  v.set(2, 300);
  EXPECT_EQ(v[2], 300);
}

TEST(VecTest, GetIsAvailableOnAConstVector) {
  const Vec4 v = MakeVec4(1, 2, 3, 4);
  EXPECT_EQ(v.get(0), 1);
  EXPECT_EQ(v[3], 4);
}

TEST(VecTest, AScalarBroadcastsToEveryElement) {
  Vec4 assigned;
  assigned = 7;
  EXPECT_TRUE(Equals(assigned, 7, 7, 7, 7));

  Vec4 via_set;
  via_set.set(7);
  EXPECT_TRUE(Equals(via_set, 7, 7, 7, 7));

  EXPECT_TRUE(Equals(make_vec<4>(7), 7, 7, 7, 7));
}

TEST(VecTest, ConversionCastsEveryElement) {
  vec_t<double, 4> src;
  src.set(0, 1.9);
  src.set(1, -2.9);
  src.set(2, 3.0);
  src.set(3, 4.5);

  // static_cast<int> truncates toward zero, element by element.
  EXPECT_TRUE(Equals(static_cast<Vec4>(src), 1, -2, 3, 4));
}

TEST(VecTest, CompoundAssignmentAgainstAnotherVector) {
  Vec4 v = MakeVec4(10, 20, 30, 40);
  v += MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(Equals(v, 11, 22, 33, 44));
  v -= MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(Equals(v, 10, 20, 30, 40));
  v *= MakeVec4(2, 2, 2, 2);
  EXPECT_TRUE(Equals(v, 20, 40, 60, 80));
  v /= MakeVec4(2, 4, 5, 8);
  EXPECT_TRUE(Equals(v, 10, 10, 12, 10));
  v %= MakeVec4(3, 4, 5, 6);
  EXPECT_TRUE(Equals(v, 1, 2, 2, 4));
}

TEST(VecTest, CompoundAssignmentAgainstAScalar) {
  Vec4 v = MakeVec4(0b1100, 0b1010, 0b0110, 0b0011);
  v &= 0b1001;
  EXPECT_TRUE(Equals(v, 0b1000, 0b1000, 0b0000, 0b0001));
  v |= 0b0110;
  EXPECT_TRUE(Equals(v, 0b1110, 0b1110, 0b0110, 0b0111));
  v ^= 0b1111;
  EXPECT_TRUE(Equals(v, 0b0001, 0b0001, 0b1001, 0b1000));
  v <<= 2;
  EXPECT_TRUE(Equals(v, 0b100, 0b100, 0b100100, 0b100000));
  v >>= 2;
  EXPECT_TRUE(Equals(v, 0b1, 0b1, 0b1001, 0b1000));
}

TEST(VecTest, UnaryOperatorsApplyElementwise) {
  EXPECT_TRUE(Equals(-MakeVec4(1, -2, 3, -4), -1, 2, -3, 4));
  EXPECT_TRUE(Equals(+MakeVec4(1, -2, 3, -4), 1, -2, 3, -4));
  EXPECT_TRUE(Equals(~MakeVec4(0, 1, -1, 5), ~0, ~1, ~(-1), ~5));
}

// `-v` used to negate `v` itself and hand back a copy of the result, so the
// operand of every unary operator was silently destroyed.
TEST(VecTest, UnaryOperatorsLeaveTheirOperandAlone) {
  Vec4 v = MakeVec4(1, -2, 3, -4);
  EXPECT_TRUE(Equals(-v, -1, 2, -3, 4));
  EXPECT_TRUE(Equals(v, 1, -2, 3, -4));
  EXPECT_TRUE(Equals(~v, ~1, ~(-2), ~3, ~(-4)));
  EXPECT_TRUE(Equals(v, 1, -2, 3, -4));
}

// Reading a vector never modifies it, so every operator that only reads is
// callable on a `const` one.
TEST(VecTest, ConstVectorsSupportTheReadOnlyOperators) {
  const Vec4 v = MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(Equals(-v, -1, -2, -3, -4));
  EXPECT_TRUE(Equals(v + v, 2, 4, 6, 8));
  EXPECT_TRUE(Equals(v * 3, 3, 6, 9, 12));
}

TEST(VecTest, BinaryOperatorsAgainstAnotherVector) {
  const Vec4 lhs = MakeVec4(10, 20, 30, 40);
  const Vec4 rhs = MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(Equals(lhs + rhs, 11, 22, 33, 44));
  EXPECT_TRUE(Equals(lhs - rhs, 9, 18, 27, 36));
  EXPECT_TRUE(Equals(lhs * rhs, 10, 40, 90, 160));
  EXPECT_TRUE(Equals(lhs / rhs, 10, 10, 10, 10));
  EXPECT_TRUE(Equals(lhs % rhs, 0, 0, 0, 0));
  // Neither operand is touched.
  EXPECT_TRUE(Equals(lhs, 10, 20, 30, 40));
  EXPECT_TRUE(Equals(rhs, 1, 2, 3, 4));
}

TEST(VecTest, BinaryOperatorsAgainstAScalar) {
  Vec4 v = MakeVec4(10, 20, 30, 40);
  EXPECT_TRUE(Equals(v + 5, 15, 25, 35, 45));
  EXPECT_TRUE(Equals(v - 5, 5, 15, 25, 35));
  EXPECT_TRUE(Equals(v * 2, 20, 40, 60, 80));
  EXPECT_TRUE(Equals(v / 10, 1, 2, 3, 4));
  EXPECT_TRUE(Equals(v % 7, 3, 6, 2, 5));
  // The vector is the left operand, so it is left unchanged throughout.
  EXPECT_TRUE(Equals(v, 10, 20, 30, 40));
}

TEST(VecTest, AScalarOnTheLeftKeepsOperandOrder) {
  const Vec4 v = MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(Equals(100 - v, 99, 98, 97, 96));
  EXPECT_TRUE(Equals(24 / v, 24, 12, 8, 6));
  EXPECT_TRUE(Equals(1 << v, 2, 4, 8, 16));
}

TEST(VecTest, ShiftDropsTheFirstElementAndAppends) {
  Vec4 v = MakeVec4(1, 2, 3, 4);
  v.shift(5);
  EXPECT_TRUE(Equals(v, 2, 3, 4, 5));
}

TEST(VecTest, HasFindsAnElementAndWorksOnAConstVector) {
  const Vec4 v = MakeVec4(1, 2, 3, 4);
  EXPECT_TRUE(v.has(1));
  EXPECT_TRUE(v.has(4));
  EXPECT_FALSE(v.has(5));

  // Every element is scanned, so a match in the last slot is still found.
  EXPECT_TRUE(MakeVec4(0, 0, 0, 9).has(9));
}

TEST(VecTest, TruncatedSelectsACompileTimeRange) {
  const Vec4 v = MakeVec4(1, 2, 3, 4);

  const vec_t<int, 2> middle = truncated<1, 3>(v);
  EXPECT_EQ(middle[0], 2);
  EXPECT_EQ(middle[1], 3);

  const vec_t<int, 2> prefix = truncated<2>(v);
  EXPECT_EQ(prefix[0], 1);
  EXPECT_EQ(prefix[1], 2);
}

TEST(VecTest, TruncatedSelectsARuntimeOffset) {
  const Vec4 v = MakeVec4(1, 2, 3, 4);
  const vec_t<int, 2> tail = truncated<2>(v, 2);
  EXPECT_EQ(tail[0], 3);
  EXPECT_EQ(tail[1], 4);
}

TEST(VecTest, CatJoinsVectorsAndScalars) {
  const vec_t<int, 2> pair = truncated<2>(MakeVec4(1, 2, 3, 4));

  const vec_t<int, 3> appended = cat(pair, 3);
  EXPECT_EQ(appended[0], 1);
  EXPECT_EQ(appended[1], 2);
  EXPECT_EQ(appended[2], 3);

  const vec_t<int, 3> prepended = cat(0, pair);
  EXPECT_EQ(prepended[0], 0);
  EXPECT_EQ(prepended[1], 1);
  EXPECT_EQ(prepended[2], 2);

  EXPECT_TRUE(Equals(cat(pair, pair), 1, 2, 1, 2));
}

TEST(VecTest, CatIsVariadic) {
  const vec_t<int, 2> pair = truncated<2>(MakeVec4(3, 4, 0, 0));
  // The recursion folds from the right (`cat(a, cat(b, rest))`), so only the
  // trailing argument may be a vector.
  EXPECT_TRUE(Equals(cat(1, 2, pair), 1, 2, 3, 4));
}

TEST(VecTest, SumAndProductReduceTheWholeVector) {
  EXPECT_EQ(sum(MakeVec4(1, 2, 3, 4)), 10);
  EXPECT_EQ(product(MakeVec4(1, 2, 3, 4)), 24);

  // The reduction recurses by halves, so a single element is the base case.
  vec_t<int, 1> one;
  one.set(0, 42);
  EXPECT_EQ(sum(one), 42);
  EXPECT_EQ(product(one), 42);
}

TEST(VecTest, MaxAndMinCompareElementwise) {
  const Vec4 lhs = MakeVec4(1, 5, 3, 7);
  const Vec4 rhs = MakeVec4(4, 2, 6, 0);
  EXPECT_TRUE(Equals(max(lhs, rhs), 4, 5, 6, 7));
  EXPECT_TRUE(Equals(min(lhs, rhs), 1, 2, 3, 0));

  // A scalar operand is broadcast, on either side.
  EXPECT_TRUE(Equals(max(3, lhs), 3, 5, 3, 7));
  EXPECT_TRUE(Equals(min(lhs, 3), 1, 3, 3, 3));
}

TEST(VecTest, MathFunctionsApplyElementwise) {
  vec_t<double, 2> v;
  v.set(0, 0.0);
  v.set(1, 1.0);

  EXPECT_DOUBLE_EQ(exp(v)[0], 1.0);
  EXPECT_DOUBLE_EQ(exp(v)[1], std::exp(1.0));
  EXPECT_DOUBLE_EQ(exp2(v)[1], 2.0);
  EXPECT_DOUBLE_EQ(expm1(v)[0], 0.0);
  EXPECT_DOUBLE_EQ(log(v)[1], 0.0);
  EXPECT_DOUBLE_EQ(log10(v)[1], 0.0);
  EXPECT_DOUBLE_EQ(log1p(v)[0], 0.0);
  EXPECT_DOUBLE_EQ(log2(v)[1], 0.0);
}

TEST(VecTest, StreamingPrintsIndexedElements) {
  std::ostringstream os;
  os << MakeVec4(1, 2, 3, 4);
  EXPECT_EQ(os.str(), "{[0]: 1, [1]: 2, [2]: 3, [3]: 4}");
}

}  // namespace
}  // namespace tapa
