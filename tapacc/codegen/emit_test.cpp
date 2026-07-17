#include "emit.h"

#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "../frontend/classify.h"
#include "../frontend/tapa_stub_decls.h"
#include "code_sink.h"

namespace tapa::cc {
namespace {

class FirstParamFinder : public clang::RecursiveASTVisitor<FirstParamFinder> {
 public:
  const clang::ParmVarDecl* found = nullptr;
  bool VisitFunctionDecl(clang::FunctionDecl* f) {
    if (f->getNameAsString() == "probe" && f->getNumParams() > 0) {
      found = f->getParamDecl(0);
    }
    return true;
  }
};

struct Built {
  std::unique_ptr<clang::ASTUnit> ast;
  const clang::ParmVarDecl* param = nullptr;
};

Built BuildParam(const std::string& signature) {
  const std::string code =
      std::string(kTapaStubDecls) + "\n" + signature + "\n";
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  FirstParamFinder finder;
  finder.TraverseDecl(ast->getASTContext().getTranslationUnitDecl());
  EXPECT_NE(finder.found, nullptr);
  return Built{std::move(ast), finder.found};
}

TEST(Emit, IStreamReadAndPeek) {
  auto b = BuildParam("void probe(tapa::istream<float>& in);");
  CodeSink out;
  EmitDummyStreamRW(b.param, TapaKind::kIStream, out, /*qdma=*/false);
  ASSERT_EQ(out.Lines().size(), 2u);
  EXPECT_EQ(out.Lines()[0], "{ auto val = in.read(); }");
  EXPECT_EQ(out.Lines()[1], "{ auto val = in.peek(nullptr); }");
}

TEST(Emit, OStreamWrite) {
  auto b = BuildParam("void probe(tapa::ostream<float>& out);");
  CodeSink out;
  EmitDummyStreamRW(b.param, TapaKind::kOStream, out, /*qdma=*/false);
  ASSERT_EQ(out.Lines().size(), 1u);
  EXPECT_EQ(out.Lines()[0], "out.write(float());");
}

TEST(Emit, OStreamQdmaWriteUsesElementType) {
  // qdma/axis top-level ports keep their `tapa::ostream<T>` spelling (the port
  // is not rewritten to `hls::stream<qdma_axis<...>>`), so the anti-DCE dummy
  // write must use the stream's element type, not qdma_axis.
  auto b = BuildParam("void probe(tapa::ostream<float>& out);");
  CodeSink out;
  EmitDummyStreamRW(b.param, TapaKind::kOStream, out, /*qdma=*/true);
  ASSERT_EQ(out.Lines().size(), 1u);
  EXPECT_EQ(out.Lines()[0], "out.write(float());");
}

TEST(Emit, MmapOffset) {
  auto b = BuildParam("void probe(tapa::mmap<float> m);");
  CodeSink out;
  EmitDummyMmapOrScalarRW(b.param, TapaKind::kMmap, out);
  ASSERT_EQ(out.Lines().size(), 1u);
  EXPECT_EQ(out.Lines()[0],
            "{ auto val = reinterpret_cast<volatile uint8_t&>(m_offset); }");
}

TEST(Emit, Scalar) {
  auto b = BuildParam("void probe(int n);");
  CodeSink out;
  EmitDummyMmapOrScalarRW(b.param, TapaKind::kNotTapa, out);
  ASSERT_EQ(out.Lines().size(), 1u);
  EXPECT_EQ(out.Lines()[0],
            "{ auto val = reinterpret_cast<volatile uint8_t&>(n); }");
}

TEST(Emit, ConstScalarKeepsConst) {
  auto b = BuildParam("void probe(const int n);");
  CodeSink out;
  EmitDummyMmapOrScalarRW(b.param, TapaKind::kNotTapa, out);
  ASSERT_EQ(out.Lines().size(), 1u);
  EXPECT_EQ(out.Lines()[0],
            "{ auto val = reinterpret_cast<volatile const uint8_t&>(n); }");
}

TEST(Emit, MmapsArrayChannels) {
  auto b = BuildParam("void probe(tapa::mmaps<float, 2> m);");
  CodeSink out;
  EmitDummyMmapOrScalarRW(b.param, TapaKind::kMmaps, out);
  ASSERT_EQ(out.Lines().size(), 2u);
  EXPECT_EQ(out.Lines()[0],
            "{ auto val = reinterpret_cast<volatile uint8_t&>(m_0_offset); }");
  EXPECT_EQ(out.Lines()[1],
            "{ auto val = reinterpret_cast<volatile uint8_t&>(m_1_offset); }");
}

}  // namespace
}  // namespace tapa::cc
