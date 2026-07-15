#include "conventions.h"

#include "gtest/gtest.h"

#include "code_sink.h"

namespace tapa::cc {
namespace {

TEST(Conventions, Names) {
  EXPECT_EQ(OffsetName("a"), "a_offset");
  EXPECT_EQ(ArrayElemOffset("m", 2), "m_2_offset");
  EXPECT_EQ(ArrayNameAt("q", 3), "q[3]");
  EXPECT_EQ(FifoVar("s"), "s._");
  EXPECT_EQ(PeekVar("s"), "s._peek");
  EXPECT_EQ(kMangledPrefix, "tapa_mangled");
}

TEST(CodeSink, Accumulates) {
  CodeSink out;
  EXPECT_TRUE(out.Empty());
  out.Line("void(x);");
  out.Pragma({"HLS", "interface", "ap_fifo", "port =", "q"});
  ASSERT_EQ(out.Lines().size(), 2u);
  EXPECT_EQ(out.Lines()[0], "void(x);");
  EXPECT_EQ(out.Str(), "void(x);\n#pragma HLS interface ap_fifo port = q");
}

}  // namespace
}  // namespace tapa::cc
