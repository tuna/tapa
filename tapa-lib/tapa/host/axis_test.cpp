#include "tapa/host/axis.h"

#include <cstddef>
#include <sstream>
#include <string>

#include "gtest/gtest.h"

#include "tapa.h"

namespace tapa {
namespace {

// The vendor layout, recorded from ap_axiu on Vitis 2025.2. Object size and
// member offsets are observable: they set the mmap element stride and the
// stream element size, so a packet that is one byte off silently corrupts
// every transfer between host and kernel.
//
// tapa/vendor_parity_test.cpp derives the same numbers from the vendor
// headers directly whenever they are installed. These constants are what
// keeps the check running when they are not; regenerate them from that test
// if a future Vitis reshapes the packet.
using P32 = axis<u<32>>;
using P64 = axis<u<64>>;
using P512 = axis<u<512>>;
using P8 = axis<u<8>>;
using P12 = axis<u<12>>;
using PAll = axis<u<32>, 4, 3, 2>;

TEST(Axis, LayoutMatchesTheVendorPacket) {
  EXPECT_EQ(sizeof(P32), 12u);
  EXPECT_EQ(alignof(P32), 4u);
  EXPECT_EQ(offsetof(P32, data), 0u);
  EXPECT_EQ(offsetof(P32, keep), 4u);
  EXPECT_EQ(offsetof(P32, strb), 5u);
  EXPECT_EQ(offsetof(P32, user), 6u);
  EXPECT_EQ(offsetof(P32, last), 7u);
  EXPECT_EQ(offsetof(P32, id), 8u);
  EXPECT_EQ(offsetof(P32, dest), 9u);

  EXPECT_EQ(sizeof(P64), 16u);
  EXPECT_EQ(alignof(P64), 8u);
  EXPECT_EQ(offsetof(P64, keep), 8u);
  EXPECT_EQ(offsetof(P64, last), 11u);

  // A 512-bit payload takes a 64-bit TKEEP and TSTRB -- one bit per byte.
  EXPECT_EQ(sizeof(P512), 128u);
  EXPECT_EQ(alignof(P512), 64u);
  EXPECT_EQ(offsetof(P512, keep), 64u);
  EXPECT_EQ(offsetof(P512, strb), 72u);
  EXPECT_EQ(offsetof(P512, last), 81u);

  EXPECT_EQ(sizeof(P8), 7u);
  EXPECT_EQ(alignof(P8), 1u);
  EXPECT_EQ(sizeof(P12), 8u);
  EXPECT_EQ(alignof(P12), 2u);

  // A disabled signal still occupies its slot, so enabling one does not move
  // the others.
  EXPECT_EQ(sizeof(PAll), sizeof(P32));
  EXPECT_EQ(offsetof(PAll, user), offsetof(P32, user));
  EXPECT_EQ(offsetof(PAll, id), offsetof(P32, id));
  EXPECT_EQ(offsetof(PAll, dest), offsetof(P32, dest));
}

TEST(Axis, SignalWidthsFollowThePayload) {
  EXPECT_EQ(P32::width_data, 32);
  EXPECT_EQ(P32::width_keep, 4);
  EXPECT_EQ(P32::width_strb, 4);
  EXPECT_EQ(P512::width_keep, 64);
  // Not a whole number of bytes: TKEEP rounds up.
  EXPECT_EQ(P12::width_keep, 2);
  EXPECT_EQ(axis<float>::width_data, 32);
  EXPECT_EQ(axis<float>::width_keep, 4);

  EXPECT_FALSE(P32::has_user);
  EXPECT_FALSE(P32::has_id);
  EXPECT_FALSE(P32::has_dest);
  EXPECT_TRUE(PAll::has_user);
  EXPECT_TRUE(PAll::has_id);
  EXPECT_TRUE(PAll::has_dest);
  EXPECT_EQ(PAll::width_user, 4);
  EXPECT_EQ(PAll::width_id, 3);
  EXPECT_EQ(PAll::width_dest, 2);
}

TEST(Axis, DefaultPacketMarksEveryByteValid) {
  P32 p;
  EXPECT_EQ(p.keep, u<4>(0xf));
  EXPECT_EQ(p.strb, u<4>(0xf));
  EXPECT_EQ(p.last, u<1>(0));

  P512 wide;
  EXPECT_EQ(wide.keep, ~u<64>(0));

  PAll all;
  EXPECT_EQ(all.get_user(), u<4>(0));
  EXPECT_EQ(all.get_id(), u<3>(0));
  EXPECT_EQ(all.get_dest(), u<2>(0));
}

TEST(Axis, KeepAllRestoresEveryByte) {
  P32 p;
  p.keep = 0;
  EXPECT_EQ(p.keep, u<4>(0));
  p.keep_all();
  EXPECT_EQ(p.keep, u<4>(0xf));
}

TEST(Axis, AccessorsRoundTrip) {
  PAll p;
  p.set_data(0xdeadbeef);
  p.set_keep(0x5);
  p.set_strb(0xa);
  p.set_last(1);
  p.set_user(0xc);
  p.set_id(0x5);
  p.set_dest(0x2);

  EXPECT_EQ(p.get_data(), u<32>(0xdeadbeef));
  EXPECT_EQ(p.get_keep(), u<4>(0x5));
  EXPECT_EQ(p.get_strb(), u<4>(0xa));
  EXPECT_EQ(p.get_last(), u<1>(1));
  EXPECT_EQ(p.get_user(), u<4>(0xc));
  EXPECT_EQ(p.get_id(), u<3>(0x5));
  EXPECT_EQ(p.get_dest(), u<2>(0x2));
}

TEST(Axis, WritingADisabledSignalIsInert) {
  // The vendor throws here in simulation and asserts under synthesis; a
  // packet that never carries the signal simply has nowhere to put it.
  P32 p;
  p.set_user(0xf);
  EXPECT_EQ(p.get_user(), u<1>(0));
  EXPECT_EQ(sizeof(P32), 12u);
}

TEST(Axis, FullConstructorSetsEverySignal) {
  const PAll p(0x12345678, 0x3, 0xc, 0x9, 1, 0x6, 0x1);
  EXPECT_EQ(p.data, u<32>(0x12345678));
  EXPECT_EQ(p.keep, u<4>(0x3));
  EXPECT_EQ(p.strb, u<4>(0xc));
  EXPECT_EQ(p.get_user(), u<4>(0x9));
  EXPECT_EQ(p.last, u<1>(1));
  EXPECT_EQ(p.get_id(), u<3>(0x6));
  EXPECT_EQ(p.get_dest(), u<2>(0x1));
}

TEST(Axis, EqualityComparesEveryEnabledSignal) {
  PAll a;
  PAll b;
  EXPECT_EQ(a, b);
  b.set_dest(0x3);
  EXPECT_NE(a, b);
  b.set_dest(0);
  EXPECT_EQ(a, b);

  // A disabled signal cannot make two packets differ.
  P32 c;
  P32 d;
  d.set_id(0x7);
  EXPECT_EQ(c, d);
}

TEST(Axis, PrintsOnlyTheSignalsItCarries) {
  P32 p;
  p.data = 5;
  std::ostringstream os;
  os << p;
  const std::string text = os.str();
  EXPECT_NE(text.find("data: 5"), std::string::npos);
  EXPECT_NE(text.find("last: 0"), std::string::npos);
  EXPECT_EQ(text.find("user:"), std::string::npos);

  PAll all;
  all.set_user(3);
  std::ostringstream os_all;
  os_all << all;
  EXPECT_NE(os_all.str().find("user: 3"), std::string::npos);
}

TEST(Axis, TravelsThroughAStream) {
  stream<P32, 4> q("packets");
  for (int i = 0; i < 4; ++i) {
    P32 p;
    p.data = i * 7;
    p.last = i == 3;
    q.write(p);
  }
  for (int i = 0; i < 4; ++i) {
    const P32 p = q.read();
    EXPECT_EQ(p.data, u<32>(i * 7));
    EXPECT_EQ(p.last, u<1>(i == 3));
    EXPECT_EQ(p.keep, u<4>(0xf));
  }
}

}  // namespace
}  // namespace tapa
