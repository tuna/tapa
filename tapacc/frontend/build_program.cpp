#include "build_program.h"

#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Basic/SourceManager.h"

namespace tapa::cc {

namespace {

class FileFuncCollector : public clang::RecursiveASTVisitor<FileFuncCollector> {
 public:
  explicit FileFuncCollector(const clang::ASTContext& ctx) : ctx_(ctx) {}
  std::vector<const clang::FunctionDecl*> funcs;

  bool VisitFunctionDecl(clang::FunctionDecl* func) {
    if (func->isGlobal() &&
        ctx_.getSourceManager().isWrittenInMainFile(func->getLocation()) &&
        func->hasBody()) {
      funcs.push_back(func);
    }
    return true;
  }

 private:
  const clang::ASTContext& ctx_;
};

// Definitions `isGlobal()` excludes: `static` functions, functions in an
// anonymous namespace, and non-static methods. They are helpers like any
// other and need the inline policy, but they are collected separately
// because task discovery indexes by bare name — a method sharing a name
// with a task would read as a redefinition.
class LocalFuncCollector
    : public clang::RecursiveASTVisitor<LocalFuncCollector> {
 public:
  explicit LocalFuncCollector(const clang::ASTContext& ctx) : ctx_(ctx) {}
  std::vector<const clang::FunctionDecl*> funcs;

  bool VisitFunctionDecl(clang::FunctionDecl* func) {
    if (!func->isGlobal() && func->isThisDeclarationADefinition() &&
        ctx_.getSourceManager().isWrittenInMainFile(func->getLocation()) &&
        // An implicit or compiler-supplied body has no source to rewrite.
        !func->isImplicit() && !func->isDefaulted() && !func->isDeleted()) {
      funcs.push_back(func);
    }
    return true;
  }

 private:
  const clang::ASTContext& ctx_;
};

}  // namespace

std::vector<const clang::FunctionDecl*> CollectFileFuncs(
    const clang::ASTContext& ctx) {
  FileFuncCollector collector(ctx);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(
      const_cast<clang::ASTContext&>(ctx).getTranslationUnitDecl());
  return collector.funcs;
}

std::vector<const clang::FunctionDecl*> CollectLocalFuncs(
    const clang::ASTContext& ctx) {
  LocalFuncCollector collector(ctx);
  collector.TraverseDecl(
      const_cast<clang::ASTContext&>(ctx).getTranslationUnitDecl());
  return collector.funcs;
}

}  // namespace tapa::cc
