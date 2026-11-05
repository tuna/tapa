#include "classify.h"

#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

// Grab the first parameter of a function named `probe`.
class ProbeParamFinder : public clang::RecursiveASTVisitor<ProbeParamFinder> {
 public:
  const clang::ParmVarDecl* found = nullptr;
  bool VisitFunctionDecl(clang::FunctionDecl* f) {
    if (f->getNameAsString() == "probe" && f->getNumParams() > 0) {
      found = f->getParamDecl(0);
    }
    return true;
  }
};

TapaKind ClassifyFirstParam(const std::string& decl) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + decl + "\n";
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  ProbeParamFinder finder;
  finder.TraverseDecl(ast->getASTContext().getTranslationUnitDecl());
  EXPECT_NE(finder.found, nullptr);
  return ClassifyTapaType(finder.found);
}

TEST(Classify, StreamInterfaces) {
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::istream<float>& s);"),
            TapaKind::kIStream);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::ostream<float>& s);"),
            TapaKind::kOStream);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::istreams<float, 4>& s);"),
            TapaKind::kIStreams);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::ostreams<float, 4>& s);"),
            TapaKind::kOStreams);
}

TEST(Classify, MmapInterfaces) {
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::mmap<float> m);"),
            TapaKind::kMmap);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::mmaps<float, 2> m);"),
            TapaKind::kMmaps);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::async_mmap<float> m);"),
            TapaKind::kAsyncMmap);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::hmap<float, 2, 8> m);"),
            TapaKind::kHmap);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::immap<float> m);"),
            TapaKind::kImmap);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::ommap<float> m);"),
            TapaKind::kOmmap);
}

TEST(Classify, ScalarAndReferencePeeling) {
  EXPECT_EQ(ClassifyFirstParam("void probe(int n);"), TapaKind::kNotTapa);
  EXPECT_EQ(ClassifyFirstParam("void probe(unsigned long long n);"),
            TapaKind::kNotTapa);
  // const-ref mmap still classifies as mmap (references/const are peeled).
  EXPECT_EQ(ClassifyFirstParam("void probe(const tapa::mmap<const float>& m);"),
            TapaKind::kMmap);
  EXPECT_EQ(
      ClassifyFirstParam("void probe(const tapa::mmap<const float>&& m);"),
      TapaKind::kMmap);
}

TEST(Classify, Markers) {
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::seq s);"), TapaKind::kSeq);
  EXPECT_EQ(ClassifyFirstParam("void probe(tapa::executable e);"),
            TapaKind::kExecutable);
}

TEST(Classify, Predicates) {
  EXPECT_TRUE(IsStreamInterface(TapaKind::kIStream));
  EXPECT_TRUE(IsInputStream(TapaKind::kIStreams));
  EXPECT_FALSE(IsStreamInterface(TapaKind::kMmap));
  EXPECT_TRUE(IsMmapInterface(TapaKind::kHmap));
  EXPECT_TRUE(IsAsyncMmap(TapaKind::kAsyncMmap));
}

TEST(Classify, Cat) {
  EXPECT_STREQ(TapaKindCat(TapaKind::kIStream), "istream");
  EXPECT_STREQ(TapaKindCat(TapaKind::kMmaps), "mmap");
  EXPECT_STREQ(TapaKindCat(TapaKind::kAsyncMmap), "async_mmap");
  EXPECT_STREQ(TapaKindCat(TapaKind::kHmap), "mmap");
  EXPECT_STREQ(TapaKindCat(TapaKind::kNotTapa), "scalar");
}

}  // namespace
}  // namespace tapa::cc
