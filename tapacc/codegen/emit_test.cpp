#include "emit.h"

#include <memory>
#include <string>

#include "gtest/gtest.h"

#include "clang/AST/Decl.h"
#include "clang/Frontend/ASTUnit.h"

#include "code_sink.h"
#include "frontend/classify.h"
#include "frontend/tapa_stub_decls.h"

namespace tapa::cc {
namespace {

// Keep the ASTUnit alive: the ParmVarDecl and its ASTContext live in it.
struct Built {
  std::unique_ptr<clang::ASTUnit> ast;
  const clang::ParmVarDecl* param = nullptr;
};

Built BuildParam(const std::string& signature) {
  ParsedProbe parsed = ParseProbe(signature);
  Built b{std::move(parsed.ast), FirstParamOf(parsed.fn)};
  EXPECT_NE(b.param, nullptr);
  return b;
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

TEST(Emit, OStreamQdmaWrite) {
  auto b = BuildParam("void probe(tapa::ostream<float>& out);");
  CodeSink out;
  EmitDummyStreamRW(b.param, TapaKind::kOStream, out, /*qdma=*/true);
  ASSERT_EQ(out.Lines().size(), 1u);
  EXPECT_EQ(out.Lines()[0], "out.write(qdma_axis<32, 0, 0, 0>());");
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
