#include "discover.h"

#include <map>
#include <memory>
#include <string>
#include <vector>

#include "gtest/gtest.h"

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Frontend/ASTUnit.h"
#include "clang/Tooling/Tooling.h"

#include "program.h"
#include "tapa_stub_decls.h"

namespace tapa::cc {
namespace {

// Collect global function definitions in the main file, mirroring the tool's
// first pass. RecursiveASTVisitor does not descend into implicit template
// instantiations by default, so a specialization is reached only via `invoke`.
class FuncCollector : public clang::RecursiveASTVisitor<FuncCollector> {
 public:
  explicit FuncCollector(const clang::ASTContext& ctx) : ctx_(ctx) {}
  std::vector<const clang::FunctionDecl*> funcs;
  bool VisitFunctionDecl(clang::FunctionDecl* f) {
    if (f->isGlobal() &&
        ctx_.getSourceManager().isWrittenInMainFile(f->getLocation()) &&
        f->hasBody()) {
      funcs.push_back(f);
    }
    return true;
  }

 private:
  const clang::ASTContext& ctx_;
};

constexpr char kProgram[] = R"cpp(
  void Leaf(tapa::istream<float>& in, tapa::ostream<float>& out) {}
  [[tapa::target("ignore")]] void LeafIgn(tapa::istream<float>& in) {}
  [[tapa::target("ignore")]] void Unreached(tapa::istream<float>& in) {}
  template <typename T>
  void TLeaf(tapa::istream<T>& in) {}
  void Mid(tapa::istream<float>& a, tapa::ostream<float>& b) {
    tapa::task().invoke(Leaf, a, b);
  }
  void Top(tapa::istream<float>& x, tapa::ostream<float>& y) {
    tapa::stream<float> q;
    tapa::task()
        .invoke(Mid, x, q)
        .invoke(Leaf, q, y)
        .invoke(LeafIgn, q)
        .invoke(TLeaf<float>, q);
  }
)cpp";

struct Discovered {
  std::unique_ptr<clang::ASTUnit> ast;
  std::map<std::string, TaskModel> tasks;
};

Discovered Discover(const std::string& top,
                    SynthTarget target = SynthTarget::kXilinxHls) {
  const std::string code = std::string(kTapaStubDecls) + "\n" + kProgram;
  auto ast = clang::tooling::buildASTFromCodeWithArgs(
      code, std::vector<std::string>{"-std=c++17"});
  EXPECT_NE(ast, nullptr);
  FuncCollector collector(ast->getASTContext());
  collector.TraverseDecl(ast->getASTContext().getTranslationUnitDecl());
  auto tasks =
      DiscoverTasks(ast->getASTContext(), top, target, collector.funcs);
  return Discovered{std::move(ast), std::move(tasks)};
}

TEST(Discover, ReachableSetAndLevels) {
  auto d = Discover("Top");
  // Top, Mid, Leaf, LeafIgn, TLeaf<float> -> 5. Unreached is excluded.
  EXPECT_EQ(d.tasks.size(), 5u);
  ASSERT_TRUE(d.tasks.count("Top"));
  ASSERT_TRUE(d.tasks.count("Mid"));
  ASSERT_TRUE(d.tasks.count("Leaf"));
  ASSERT_TRUE(d.tasks.count("LeafIgn"));
  EXPECT_FALSE(d.tasks.count("Unreached"));

  EXPECT_EQ(d.tasks.at("Top").level, TaskLevel::kUpper);
  EXPECT_EQ(d.tasks.at("Mid").level, TaskLevel::kUpper);
  EXPECT_EQ(d.tasks.at("Leaf").level, TaskLevel::kLower);
  EXPECT_EQ(d.tasks.at("LeafIgn").level, TaskLevel::kLower);
}

TEST(Discover, TargetResolution) {
  auto d = Discover("Top");
  EXPECT_EQ(d.tasks.at("Top").target, SynthTarget::kXilinxHls);
  EXPECT_EQ(d.tasks.at("Leaf").target, SynthTarget::kXilinxHls);
  EXPECT_EQ(d.tasks.at("LeafIgn").target, SynthTarget::kIgnore);
}

TEST(Discover, DefaultTargetIsHonored) {
  auto d = Discover("Top", SynthTarget::kXilinxVitis);
  EXPECT_EQ(d.tasks.at("Top").target, SynthTarget::kXilinxVitis);
  // The explicit [[tapa::target("ignore")]] still wins over the default.
  EXPECT_EQ(d.tasks.at("LeafIgn").target, SynthTarget::kIgnore);
}

TEST(Discover, TemplateSpecialization) {
  auto d = Discover("Top");
  const TaskModel* spec = nullptr;
  for (const auto& [name, model] : d.tasks) {
    if (model.is_template_spec) spec = &model;
  }
  ASSERT_NE(spec, nullptr);
  EXPECT_EQ(spec->readable_name, "TLeaf<float>");
  EXPECT_EQ(spec->name.rfind("tapa_mangled", 0), 0u);  // starts with prefix
  EXPECT_EQ(spec->level, TaskLevel::kLower);
  EXPECT_NE(spec->invoker, nullptr);  // wrapper is emitted after its invoker
}

TEST(Discover, TopNotFoundReturnsEmpty) {
  auto d = Discover("DoesNotExist");
  EXPECT_TRUE(d.tasks.empty());
}

}  // namespace
}  // namespace tapa::cc
