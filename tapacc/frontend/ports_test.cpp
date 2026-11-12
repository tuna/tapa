#include "ports.h"

#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/Frontend/ASTUnit.h"

#include "clang/Basic/DiagnosticIDs.h"

#include "classify.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

// Diagnostics sink for error-path tests: the default TextDiagnosticPrinter
// asserts on diagnostics emitted after source-file processing, which is
// exactly when BuildPorts runs against a finished test AST.
class CountingDiags : public clang::DiagnosticConsumer {
 public:
  void HandleDiagnostic(clang::DiagnosticsEngine::Level level,
                        const clang::Diagnostic&) override {
    if (level >= clang::DiagnosticsEngine::Error) ++errors;
  }
  unsigned errors = 0;
};

// Replace the AST's diagnostic client with the counting sink and build ports.
std::vector<Port> BuildPortsExpectingError(ParsedProbe& b,
                                           CountingDiags& diags) {
  b.ast->getDiagnostics().setClient(&diags, /*ShouldOwn=*/false);
  return BuildPorts(b.ast->getASTContext(), b.fn);
}

TEST(Ports, MixedSignature) {
  auto b = ParseProbe(
      "void probe(tapa::mmap<const float> a, tapa::istream<float>& s, int n, "
      "tapa::mmaps<float, 2> m, tapa::hmap<int, 4, 8> h);");
  const std::vector<Port> ports = BuildPorts(b.ast->getASTContext(), b.fn);

  // mmaps expands to 2 channels, so 4 scalar/single params -> 6 ports.
  ASSERT_EQ(ports.size(), 6u);

  EXPECT_EQ(ports[0].name, "a");
  EXPECT_STREQ(TapaKindCat(ports[0].kind), "mmap");
  EXPECT_EQ(ports[0].ctype, "const float*");
  EXPECT_EQ(ports[0].width, 32u);

  EXPECT_EQ(ports[1].name, "s");
  EXPECT_STREQ(TapaKindCat(ports[1].kind), "istream");
  EXPECT_EQ(ports[1].ctype, "float");
  EXPECT_EQ(ports[1].width, 32u);

  EXPECT_EQ(ports[2].name, "n");
  EXPECT_STREQ(TapaKindCat(ports[2].kind), "scalar");
  EXPECT_EQ(ports[2].ctype, "int");
  EXPECT_EQ(ports[2].width, 32u);

  EXPECT_EQ(ports[3].name, "m[0]");
  EXPECT_STREQ(TapaKindCat(ports[3].kind), "mmap");
  EXPECT_EQ(ports[3].ctype, "float*");
  EXPECT_EQ(ports[4].name, "m[1]");
  EXPECT_STREQ(TapaKindCat(ports[4].kind), "mmap");

  EXPECT_EQ(ports[5].name, "h");
  EXPECT_STREQ(TapaKindCat(ports[5].kind), "mmap");
  EXPECT_EQ(ports[5].ctype, "int*");
  ASSERT_TRUE(ports[5].chan_count.has_value());
  EXPECT_EQ(*ports[5].chan_count, 4u);
  ASSERT_TRUE(ports[5].chan_size.has_value());
  EXPECT_EQ(*ports[5].chan_size, 8u);
}

TEST(Ports, StreamsCarryChanCountNotExpanded) {
  auto b = ParseProbe("void probe(tapa::istreams<float, 3>& s);");
  const std::vector<Port> ports = BuildPorts(b.ast->getASTContext(), b.fn);
  ASSERT_EQ(ports.size(), 1u);
  EXPECT_EQ(ports[0].name, "s");
  EXPECT_STREQ(TapaKindCat(ports[0].kind), "istreams");
  ASSERT_TRUE(ports[0].chan_count.has_value());
  EXPECT_EQ(*ports[0].chan_count, 3u);
  EXPECT_FALSE(ports[0].chan_size.has_value());
}

TEST(Ports, AsyncMmap) {
  auto b = ParseProbe("void probe(tapa::async_mmap<int>& a);");
  const std::vector<Port> ports = BuildPorts(b.ast->getASTContext(), b.fn);
  ASSERT_EQ(ports.size(), 1u);
  EXPECT_STREQ(TapaKindCat(ports[0].kind), "async_mmap");
  EXPECT_EQ(ports[0].ctype, "int*");
}

// The graph records an hmap's chan_size in 32 bits: the largest that fits
// passes through, and one past it is an error rather than a wrapped value.
TEST(Ports, HmapChanSizeFitsIn32Bits) {
  auto fits = ParseProbe("void probe(tapa::hmap<char, 2, 4294967295LL> h);");
  const std::vector<Port> ports =
      BuildPorts(fits.ast->getASTContext(), fits.fn);
  ASSERT_EQ(ports.size(), 1u);
  ASSERT_TRUE(ports[0].chan_size.has_value());
  EXPECT_EQ(*ports[0].chan_size, 4294967295u);

  for (const char* signature : {
           "void probe(tapa::hmap<char, 2, 4294967296LL> h);",
           "void probe(tapa::hmap<char, 2, -1> h);",
       }) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

// The other integral template arguments the graph records in 32 bits are
// range-checked like an hmap's chan_size: a negative count is an error
// rather than a wrapped value.
TEST(Ports, IntegralArgsFitIn32Bits) {
  for (const char* signature : {
           "void probe(tapa::hmap<char, -1, 8> h);",
           "void probe(tapa::istreams<float, -1> s);",
           "void probe(tapa::mmaps<float, -2> m);",
       }) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

// A port is known by its parameter's name: an unnamed parameter used to
// become a port named "", which no vendor RTL port matches.
TEST(Ports, EveryParameterIsNamed) {
  for (const char* signature : {
           "void probe(tapa::mmap<int> m, int);",
           "void probe(tapa::istream<int>&, int n);",
       }) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_EQ(diags.errors, 1u) << signature;
  }
}

// A scalar by reference used to pass on the top with the reference's
// 64-bit width, which sized its kernel.xml register.
TEST(Ports, ScalarsArePassedByValue) {
  for (const char* signature : {
           "void probe(const int& n);",
           "void probe(int& n);",
           "void probe(float&& x);",
       }) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_EQ(diags.errors, 1u) << signature;
  }
}

TEST(Ports, StreamsAndAsyncMmapRequireReference) {
  const char* bad[] = {
      "void probe(tapa::istream<int> s);",
      "void probe(tapa::istream<int>&& s);",
      "void probe(tapa::ostream<int> s);",
      "void probe(tapa::ostream<int>&& s);",
      "void probe(tapa::istreams<int, 2> s);",
      "void probe(tapa::istreams<int, 2>&& s);",
      "void probe(tapa::ostreams<int, 2> s);",
      "void probe(tapa::ostreams<int, 2>&& s);",
      "void probe(tapa::async_mmap<int> m);",
      "void probe(tapa::async_mmap<int>&& m);",
  };
  for (const char* signature : bad) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

// `tapa::stream` is a channel, not a port: it has no direction, so a task
// cannot declare one as a parameter; as a scalar it would bind to the
// s_axilite control bundle, which Vitis HLS only refuses much later.
TEST(Ports, StreamInstanceIsNotAPort) {
  const char* bad[] = {
      "void probe(tapa::stream<int>& s);",
      "void probe(tapa::stream<int>&& s);",
      "void probe(tapa::stream<int, 8>& s);",
      "void probe(tapa::streams<int, 2>& s);",
      "void probe(tapa::streams<int, 2>&& s);",
  };
  for (const char* signature : bad) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

TEST(Ports, RawPointerIsNotAMemoryPort) {
  const char* bad[] = {
      "void probe(int* mem);",
      "void probe(const float* mem);",
  };
  for (const char* signature : bad) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

TEST(Ports, MmapFamilyRequiresValue) {
  const char* bad[] = {
      "void probe(tapa::mmap<int>& m);",
      "void probe(tapa::mmap<int>&& m);",
      "void probe(tapa::mmaps<int, 2>& m);",
      "void probe(tapa::mmaps<int, 2>&& m);",
      "void probe(tapa::hmap<int, 4, 8>& m);",
      "void probe(tapa::hmap<int, 4, 8>&& m);",
  };
  for (const char* signature : bad) {
    auto b = ParseProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

}  // namespace
}  // namespace tapa::cc
