#include "ports.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "clang/Basic/DiagnosticIDs.h"

#include "classify.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

class ProbeFinder : public clang::RecursiveASTVisitor<ProbeFinder> {
 public:
  const clang::FunctionDecl* found = nullptr;
  bool VisitFunctionDecl(clang::FunctionDecl* f) {
    if (f->getNameAsString() == "probe") found = f;
    return true;
  }
};

// Keep the ASTUnit alive: the FunctionDecl and its ASTContext live in it.
struct Built {
  std::unique_ptr<clang::ASTUnit> ast;
  const clang::FunctionDecl* fn = nullptr;
};

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
std::vector<Port> BuildPortsExpectingError(Built& b, CountingDiags& diags) {
  b.ast->getDiagnostics().setClient(&diags, /*ShouldOwn=*/false);
  return BuildPorts(b.ast->getASTContext(), b.fn);
}

Built BuildProbe(const std::string& signature) {
  const std::string code =
      std::string(kTapaStubDecls) + "\n" + signature + "\n";
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  ProbeFinder finder;
  finder.TraverseDecl(ast->getASTContext().getTranslationUnitDecl());
  EXPECT_NE(finder.found, nullptr);
  return Built{std::move(ast), finder.found};
}

TEST(Ports, MixedSignature) {
  auto b = BuildProbe(
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
  auto b = BuildProbe("void probe(tapa::istreams<float, 3>& s);");
  const std::vector<Port> ports = BuildPorts(b.ast->getASTContext(), b.fn);
  ASSERT_EQ(ports.size(), 1u);
  EXPECT_EQ(ports[0].name, "s");
  EXPECT_STREQ(TapaKindCat(ports[0].kind), "istreams");
  ASSERT_TRUE(ports[0].chan_count.has_value());
  EXPECT_EQ(*ports[0].chan_count, 3u);
  EXPECT_FALSE(ports[0].chan_size.has_value());
}

TEST(Ports, AsyncMmapAndOmmap) {
  auto b =
      BuildProbe("void probe(tapa::async_mmap<int>& a, tapa::ommap<float> o);");
  const std::vector<Port> ports = BuildPorts(b.ast->getASTContext(), b.fn);
  ASSERT_EQ(ports.size(), 2u);
  EXPECT_STREQ(TapaKindCat(ports[0].kind), "async_mmap");
  EXPECT_EQ(ports[0].ctype, "int*");
  EXPECT_STREQ(TapaKindCat(ports[1].kind), "ommap");
  EXPECT_EQ(ports[1].ctype, "float*");
}

TEST(Ports, StreamsAndAsyncMmapRequireReference) {
  const char* bad[] = {
      "void probe(tapa::istream<int> s);",
      "void probe(tapa::ostream<int> s);",
      "void probe(tapa::istreams<int, 2> s);",
      "void probe(tapa::ostreams<int, 2> s);",
      "void probe(tapa::async_mmap<int> m);",
  };
  for (const char* signature : bad) {
    auto b = BuildProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

TEST(Ports, MmapFamilyRequiresValue) {
  const char* bad[] = {
      "void probe(tapa::mmap<int>& m);",
      "void probe(tapa::mmaps<int, 2>& m);",
      "void probe(tapa::immap<int>& m);",
      "void probe(tapa::ommap<int>& m);",
      "void probe(tapa::hmap<int, 4, 8>& m);",
  };
  for (const char* signature : bad) {
    auto b = BuildProbe(signature);
    CountingDiags diags;
    BuildPortsExpectingError(b, diags);
    EXPECT_GT(diags.errors, 0u) << signature;
  }
}

}  // namespace
}  // namespace tapa::cc
