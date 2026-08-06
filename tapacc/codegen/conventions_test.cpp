#include "conventions.h"

#include <cstdlib>
#include <fstream>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "code_sink.h"

namespace tapa::cc {
namespace {

// The conventions are one half of a cross-language contract: this test
// checks the C++ spellings against the shared fixture, and
// `tapa-core/tapa-ir/src/port.rs` checks the RTL-identifier projection
// of the same file. A production either side does not recognize fails
// that side, so the fixture only changes in lockstep with both.
std::string FixturePath() {
  const char* srcdir = std::getenv("TEST_SRCDIR");
  const char* rlocation = std::getenv("TAPA_NAMING_FIXTURE");
  if (srcdir != nullptr && rlocation != nullptr)
    return std::string(srcdir) + "/" + rlocation;
  // Direct (non-bazel) invocation from the repository root.
  return "tapa-core/tapa-ir/testdata/naming_conventions.tsv";
}

std::vector<std::string> SplitTabs(const std::string& line) {
  std::vector<std::string> fields;
  size_t begin = 0;
  while (true) {
    size_t end = line.find('\t', begin);
    fields.push_back(line.substr(begin, end - begin));
    if (end == std::string::npos) return fields;
    begin = end + 1;
  }
}

TEST(Conventions, MatchesCrossLanguageFixture) {
  std::ifstream fixture(FixturePath());
  ASSERT_TRUE(fixture.is_open()) << "cannot open fixture: " << FixturePath();
  int productions = 0;
  std::string line;
  while (std::getline(fixture, line)) {
    if (line.empty() || line[0] == '#') continue;
    std::vector<std::string> f = SplitTabs(line);
    ++productions;
    if (f[0] == "offset_name") {
      ASSERT_EQ(f.size(), 3u) << line;
      EXPECT_EQ(OffsetName(f[1]), f[2]) << line;
    } else if (f[0] == "array_channel") {
      // f[4] is the RTL identifier; its projection belongs to tapa-ir.
      ASSERT_EQ(f.size(), 6u) << line;
      int index = std::stoi(f[2]);
      EXPECT_EQ(ArrayNameAt(f[1], index), f[3]) << line;
      EXPECT_EQ(ArrayElemOffset(f[1], index), f[5]) << line;
    } else if (f[0] == "fifo_var") {
      ASSERT_EQ(f.size(), 3u) << line;
      EXPECT_EQ(FifoVar(f[1]), f[2]) << line;
    } else if (f[0] == "peek_var") {
      ASSERT_EQ(f.size(), 3u) << line;
      EXPECT_EQ(PeekVar(f[1]), f[2]) << line;
    } else if (f[0] == "mangled_prefix") {
      ASSERT_EQ(f.size(), 2u) << line;
      EXPECT_EQ(kMangledPrefix, f[1]) << line;
    } else if (f[0] == "direct_offset_port") {
      // Vitis HLS names this RTL port; the probe order is owned by the
      // Rust consumers (tapa-codegen child pinning, frt-cosim
      // testbenches), whose tests read the same fixture.
    } else {
      ADD_FAILURE() << "unknown production in fixture: " << line;
    }
  }
  EXPECT_GE(productions, 8) << "fixture lost productions";
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
