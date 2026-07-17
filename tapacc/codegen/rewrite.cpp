#include "rewrite.h"

#include <set>
#include <string>

#include "clang/AST/Attr.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/StmtCXX.h"
#include "clang/Basic/SourceManager.h"
#include "clang/Rewrite/Core/Rewriter.h"

#include "emit.h"
#include "wrapper.h"

#include "../frontend/discover.h"

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

// The primary-template pattern of a template-specialization task's definition
// (the decl that actually appears in the source), or nullptr.
const clang::FunctionDecl* SpecPrimary(const TaskModel& model) {
  if (!model.is_template_spec || model.def == nullptr) return nullptr;
  const clang::FunctionDecl* pattern =
      model.def->getTemplateInstantiationPattern();
  return pattern != nullptr ? pattern->getCanonicalDecl() : nullptr;
}

std::string EmitTaskCode(const Program& program, const TaskModel& task,
                         const Backend& backend, clang::ASTContext& ctx) {
  clang::Rewriter rewriter(ctx.getSourceManager(), ctx.getLangOpts());

  // The source decls that are template patterns for some task specialization,
  // and the pattern for the current task (if it is a specialization). These
  // primaries appear as functions in the file but are keyed in program.tasks by
  // mangled name, so they are handled here rather than as helpers.
  std::set<const clang::FunctionDecl*> spec_primaries;
  for (const auto& [name, model] : program.tasks) {
    if (const clang::FunctionDecl* primary = SpecPrimary(model)) {
      spec_primaries.insert(primary);
    }
  }
  const clang::FunctionDecl* current_primary = SpecPrimary(task);

  // Non-template task functions: signature rewritten per level; current task
  // keeps a rewritten body, the rest become signatures.
  for (const auto& [name, model] : program.tasks) {
    if (model.is_template_spec) continue;  // reached via its primary below
    const bool is_top = name == program.top;
    backend.RewriteSignature(model, is_top, rewriter);
    if (name == task.name) {
      backend.RewriteTaskFunc(model, is_top, rewriter);
      LowerLoopAttrs(model.def->getBody(), backend, rewriter);
    } else {
      backend.StripOtherTask(model.def, rewriter);
    }
  }

  // Remaining source functions: template-task primaries and plain helpers.
  for (const clang::FunctionDecl* func : program.file_funcs) {
    if (program.tasks.count(func->getNameAsString()) != 0 &&
        !program.tasks.at(func->getNameAsString()).is_template_spec) {
      continue;  // a non-template task, already handled above
    }
    const clang::FunctionDecl* canonical = func->getCanonicalDecl();
    if (spec_primaries.count(canonical) != 0) {
      // A template primary whose specialization(s) are tasks.
      if (current_primary != nullptr && canonical == current_primary) {
        // Rewrite the primary template using its OWN parameters: a bare
        // template-parameter type (e.g. `tapa_mmap_type mmap`) is classified as
        // a scalar here (no interface pragma) -- only the concrete wrapper
        // does.
        TaskModel primary = task;
        primary.def = func;
        backend.RewriteTaskFunc(primary, /*is_top=*/false, rewriter);
        LowerLoopAttrs(func->getBody(), backend, rewriter);
      } else {
        backend.StripOtherTask(func, rewriter);
      }
    } else if (GetTapaTaskObject(func->getBody()) != nullptr) {
      // Unreachable upper-level task: discovery is reachability-based, so it
      // is not in program.tasks, but its body still invokes sub-tasks and
      // would no longer type-check against their rewritten signatures. Strip
      // it like any non-current task.
      TaskModel model;
      model.def = func;
      model.level = TaskLevel::kUpper;
      backend.RewriteSignature(model, /*is_top=*/false, rewriter);
      backend.StripOtherTask(func, rewriter);
    } else {
      backend.RewriteHelperFunc(func, rewriter);
      LowerLoopAttrs(func->getBody(), backend, rewriter);
    }
  }

  // Emit the mangled wrapper for the current specialization after its invoker.
  if (task.is_template_spec) {
    InsertWrapper(task, backend, ctx, rewriter);
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
