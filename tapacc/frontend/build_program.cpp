#include "build_program.h"

#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Basic/SourceManager.h"

namespace tapa::cc {

namespace {

class FileFuncCollector : public clang::RecursiveASTVisitor<FileFuncCollector> {
 public:
  FileFuncCollector(const clang::ASTContext& ctx,
                    const std::set<clang::FileID>* files)
      : ctx_(ctx), files_(files) {}
  std::vector<const clang::FunctionDecl*> funcs;

  bool VisitFunctionDecl(clang::FunctionDecl* func) {
    if (func->isGlobal() &&
        IsInSourceFiles(ctx_, func->getLocation(), files_) && func->hasBody()) {
      funcs.push_back(func);
    }
    return true;
  }

 private:
  const clang::ASTContext& ctx_;
  const std::set<clang::FileID>* files_;
};

// Definitions `isGlobal()` excludes: `static` functions, functions in an
// anonymous namespace, and non-static methods. They are helpers like any
// other and need the inline policy, but they are collected separately
// because task discovery indexes by bare name — a method sharing a name
// with a task would read as a redefinition.
class LocalFuncCollector
    : public clang::RecursiveASTVisitor<LocalFuncCollector> {
 public:
  LocalFuncCollector(const clang::ASTContext& ctx,
                     const std::set<clang::FileID>* files)
      : ctx_(ctx), files_(files) {}
  std::vector<const clang::FunctionDecl*> funcs;

  bool VisitFunctionDecl(clang::FunctionDecl* func) {
    if (!func->isGlobal() && func->isThisDeclarationADefinition() &&
        IsInSourceFiles(ctx_, func->getLocation(), files_) &&
        // An implicit or compiler-supplied body has no source to rewrite.
        !func->isImplicit() && !func->isDefaulted() && !func->isDeleted()) {
      funcs.push_back(func);
    }
    return true;
  }

 private:
  const clang::ASTContext& ctx_;
  const std::set<clang::FileID>* files_;
};

}  // namespace

bool IsInSourceFiles(const clang::ASTContext& ctx, clang::SourceLocation loc,
                     const std::set<clang::FileID>* files) {
  const clang::SourceManager& sm = ctx.getSourceManager();
  if (files == nullptr) return sm.isWrittenInMainFile(loc);
  if (loc.isInvalid()) return false;
  return files->count(sm.getFileID(sm.getExpansionLoc(loc))) > 0;
}

std::vector<const clang::FunctionDecl*> CollectFileFuncs(
    const clang::ASTContext& ctx, const std::set<clang::FileID>* files) {
  FileFuncCollector collector(ctx, files);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(
      const_cast<clang::ASTContext&>(ctx).getTranslationUnitDecl());
  return collector.funcs;
}

std::vector<const clang::FunctionDecl*> CollectLocalFuncs(
    const clang::ASTContext& ctx, const std::set<clang::FileID>* files) {
  LocalFuncCollector collector(ctx, files);
  collector.TraverseDecl(
      const_cast<clang::ASTContext&>(ctx).getTranslationUnitDecl());
  return collector.funcs;
}

}  // namespace tapa::cc
