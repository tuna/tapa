#include "build_program.h"

#include "clang/AST/RecursiveASTVisitor.h"
#include "clang/Basic/SourceManager.h"

#include "discover.h"
#include "invoke_parser.h"
#include "ports.h"

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

}  // namespace

std::vector<const clang::FunctionDecl*> CollectFileFuncs(
    const clang::ASTContext& ctx) {
  FileFuncCollector collector(ctx);
  // TraverseDecl mutates nothing but is non-const in the API.
  collector.TraverseDecl(
      const_cast<clang::ASTContext&>(ctx).getTranslationUnitDecl());
  return collector.funcs;
}

Program BuildProgram(clang::ASTContext& ctx, llvm::StringRef top,
                     SynthTarget default_target) {
  Program program;
  program.top = top.str();
  program.file_funcs = CollectFileFuncs(ctx);
  program.tasks = DiscoverTasks(ctx, top, default_target, program.file_funcs);
  for (auto& [name, task] : program.tasks) {
    task.ports = BuildPorts(ctx, task.def);
    if (task.level == TaskLevel::kUpper) {
      ParseUpperTask(ctx, task);
    }
  }
  return program;
}

}  // namespace tapa::cc
