#include "type_args.h"

#include <memory>
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

TEST(TypeArgs, ElementTypeName) {
  auto b = BuildParam("void probe(tapa::mmap<const float> a);");
  const auto* arg = GetTemplateArg(b.param->getType(), 0);
  ASSERT_NE(arg, nullptr);
  EXPECT_EQ(TemplateArgName(*arg), "const float");
}

TEST(TypeArgs, IntegralArgs) {
  auto b = BuildParam("void probe(tapa::hmap<int, 2, 8> h);");
  // Channel counts / sizes must resolve through the canonical specialization;
  // the as-written path stores them as unevaluated expressions.
  EXPECT_EQ(IntTemplateArg(b.param->getType(), 1), 2);
  EXPECT_EQ(IntTemplateArg(b.param->getType(), 2), 8);
}

TEST(TypeArgs, IntArgMissingIsNullopt) {
  auto b = BuildParam("void probe(tapa::mmap<float> m);");
  // mmap has no integral parameter at index 1.
  EXPECT_EQ(IntTemplateArg(b.param->getType(), 1), std::nullopt);
}

TEST(TypeArgs, MissingArgIsNullNotAssert) {
  auto b = BuildParam("void probe(int n);");
  // A non-templated scalar has no template arguments: null, no crash.
  EXPECT_EQ(GetTemplateArg(b.param->getType(), 0), nullptr);
}

TEST(TypeArgs, ReferenceIsPeeled) {
  auto b = BuildParam("void probe(tapa::istream<float>& s);");
  const auto* arg = GetTemplateArg(b.param->getType(), 0);
  ASSERT_NE(arg, nullptr);
  EXPECT_EQ(TemplateArgName(*arg), "float");
}

TEST(TypeArgs, BitWidth) {
  auto b = BuildParam("void probe(unsigned long long n);");
  EXPECT_EQ(BitWidth(b.ast->getASTContext(), b.param->getType()), 64u);
}

}  // namespace
}  // namespace tapa::cc
