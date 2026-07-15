#include "rewrite.h"

#include "clang/AST/Attr.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/StmtCXX.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Rewrite/Core/Rewriter.h"

#include "emit.h"

namespace tapa::cc {

namespace {

// The body of a loop statement, or nullptr if it is not a loop.
const clang::Stmt* GetLoopBody(const clang::Stmt* stmt) {
  if (stmt == nullptr) return nullptr;
  if (const auto* s = llvm::dyn_cast<clang::DoStmt>(stmt)) return s->getBody();
  if (const auto* s = llvm::dyn_cast<clang::ForStmt>(stmt)) return s->getBody();
  if (const auto* s = llvm::dyn_cast<clang::WhileStmt>(stmt)) {
    return s->getBody();
  }
  if (const auto* s = llvm::dyn_cast<clang::CXXForRangeStmt>(stmt)) {
    return s->getBody();
  }
  return nullptr;
}

// Lower every [[tapa::pipeline]] / [[tapa::unroll]] attribute in a function
// body to backend pragmas and remove the attribute text. Walks only this body.
void LowerLoopAttrs(const clang::Stmt* stmt, const Backend& backend,
                    clang::Rewriter& rewriter) {
  if (stmt == nullptr) return;
  for (const clang::Stmt* child : stmt->children()) {
    LowerLoopAttrs(child, backend, rewriter);
  }
  const auto* attributed = llvm::dyn_cast<clang::AttributedStmt>(stmt);
  if (attributed == nullptr) return;
  const clang::Stmt* body = GetLoopBody(attributed->getSubStmt());
  for (const clang::Attr* attr : attributed->getAttrs()) {
    if (const auto* pipeline = llvm::dyn_cast<clang::TapaPipelineAttr>(attr)) {
      backend.LowerPipeline(pipeline->getII(), body, rewriter);
      RemoveLoweredAttr(rewriter, attr->getRange());
    } else if (const auto* unroll =
                   llvm::dyn_cast<clang::TapaUnrollAttr>(attr)) {
      backend.LowerUnroll(unroll->getFactor(), body, rewriter);
      RemoveLoweredAttr(rewriter, attr->getRange());
    }
  }
}

}  // namespace

std::string EmitTaskCode(const Program& program, const TaskModel& task,
                         const Backend& backend, clang::ASTContext& ctx) {
  clang::Rewriter rewriter(ctx.getSourceManager(), ctx.getLangOpts());

  // Task functions: every one gets its signature rewritten (per its own level);
  // the current task keeps a rewritten body, the rest become signatures.
  for (const auto& [name, model] : program.tasks) {
    if (model.is_template_spec) continue;  // handled via a wrapper (TODO)
    const bool is_top = name == program.top;
    backend.RewriteSignature(model, is_top, rewriter);
    if (name == task.name) {
      backend.RewriteTaskFunc(model, is_top, rewriter);
      LowerLoopAttrs(model.def->getBody(), backend, rewriter);
    } else {
      backend.StripOtherTask(model.def, rewriter);
    }
  }

  // Non-task helper functions: same rewrite in every file.
  for (const clang::FunctionDecl* func : program.file_funcs) {
    if (program.tasks.count(func->getNameAsString()) == 0) {
      backend.RewriteHelperFunc(func, rewriter);
      LowerLoopAttrs(func->getBody(), backend, rewriter);
    }
  }

  const clang::SourceManager& sm = ctx.getSourceManager();
  const clang::FileID main_file = sm.getMainFileID();
  const llvm::RewriteBuffer* buffer = rewriter.getRewriteBufferFor(main_file);
  if (buffer == nullptr) {
    return sm.getBufferData(main_file).str();  // no edits
  }
  return std::string(buffer->begin(), buffer->end());
}

}  // namespace tapa::cc
