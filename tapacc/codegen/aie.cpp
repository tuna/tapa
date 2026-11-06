#include "aie.h"

#include <string>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Attr.h"
#include "clang/AST/Decl.h"
#include "clang/AST/ParentMapContext.h"
#include "clang/AST/Stmt.h"
#include "clang/AST/StmtCXX.h"
#include "clang/AST/TypeLoc.h"

#include "frontend/diag.h"
#include "frontend/discover.h"

namespace tapa::cc {

void AieBackend::RewriteTaskFunc(const TaskModel& task, bool /*is_top*/,
                                 EditSink& edits) const {
  if (!task.def->hasBody()) return;
  // chess keeps a large body out of line, and the kernel wrapper then hands
  // it references to its port objects, so every stream or frame access
  // reloads their state from memory. Inlined, that state stays in registers.
  // With nothing spelled before the return type, the task's guard opens at
  // the same offset; appending keeps the marker inside the guard.
  edits.InsertTextAfter(task.def->getTypeSpecStartLoc(), "TAPA_AIE_INLINE ");
  // The signature stub was captured before this runs, so the frame type
  // lands in the task's own variant only; every other variant keeps the
  // portable `tapa::istream<T>&` spelling.
  for (unsigned index = 0; index < task.def->getNumParams(); ++index) {
    const clang::ParmVarDecl* param = task.def->getParamDecl(index);
    const clang::TapaFrameAttr* frame = nullptr;
    for (const clang::FunctionDecl* decl : task.def->redecls()) {
      if (index < decl->getNumParams()) {
        if (const auto* attr =
                decl->getParamDecl(index)->getAttr<clang::TapaFrameAttr>()) {
          frame = attr;
        }
      }
    }
    const clang::TypeSourceInfo* info = param->getTypeSourceInfo();
    if (frame == nullptr || info == nullptr) continue;
    const auto ref = info->getTypeLoc().getAs<clang::ReferenceTypeLoc>();
    if (ref.isNull()) continue;
    const clang::SourceRange stream = ref.getPointeeLoc().getSourceRange();
    edits.InsertTextBefore(stream.getBegin(), "tapa::aie::frame_t<");
    edits.InsertTextAfterToken(stream.getEnd(),
                               ", " + std::to_string(frame->getSize()) + ">");
  }
}

namespace {

bool HasUnrollAttr(const clang::Stmt* stmt) {
  const auto* attributed = clang::dyn_cast<clang::AttributedStmt>(stmt);
  if (attributed == nullptr) return false;
  for (const clang::Attr* attr : attributed->getAttrs()) {
    if (attr->getKind() == clang::attr::TapaUnroll) return true;
  }
  return false;
}

void UnrollNestedLoops(const clang::Stmt* stmt, EditSink& edits) {
  for (const clang::Stmt* child : stmt->children()) {
    if (child == nullptr) continue;
    if (HasUnrollAttr(child)) {
      const clang::Stmt* body =
          LoopBody(clang::cast<clang::AttributedStmt>(child)->getSubStmt());
      if (body != nullptr) UnrollNestedLoops(body, edits);
      continue;
    }
    if (const clang::Stmt* body = LoopBody(child)) {
      edits.InsertTextBefore(body->getBeginLoc(), "chess_flatten_loop ");
    }
    UnrollNestedLoops(child, edits);
  }
}

}  // namespace

void AieBackend::LowerPipeline(int /*ii*/, const std::string& /*style*/,
                               const clang::Stmt* body, EditSink& edits) const {
  // Match HLS nested-loop pipelining: the outer loop is pipelined and every
  // inner loop without an unroll attribute is fully unrolled.
  if (body != nullptr) {
    UnrollNestedLoops(body, edits);
    edits.InsertTextBefore(body->getBeginLoc(),
                           "chess_prepare_for_pipelining ");
  }
}

void AieBackend::LowerUnroll(int factor, const clang::Stmt* body,
                             EditSink& edits) const {
  if (body == nullptr) return;
  edits.InsertTextBefore(
      body->getBeginLoc(),
      factor == 0 ? std::string("chess_flatten_loop ")
                  : "chess_unroll_loop(" + std::to_string(factor) + ") ");
}

namespace {

void ReportUnsupportedAttr(const clang::Attr& attr, EditSink& edits) {
  ReportCustomDiag(edits.getASTContext(), clang::DiagnosticsEngine::Remark,
                   attr.getRange().getBegin(),
                   "[[tapa::%0]] is ignored for the AI Engine")
      << attr.getSpelling();
}

bool IsLoopBody(const clang::Stmt* body, clang::ASTContext& ctx) {
  for (const clang::DynTypedNode& parent : ctx.getParents(*body)) {
    const auto* loop = parent.get<clang::Stmt>();
    if (loop != nullptr && LoopBody(loop) == body) return true;
  }
  return false;
}

}  // namespace

void AieBackend::LowerFuncPipeline(int, const std::string&,
                                   const clang::Stmt* body,
                                   EditSink& edits) const {
  if (body != nullptr) {
    ReportCustomDiag(edits.getASTContext(), clang::DiagnosticsEngine::Remark,
                     body->getBeginLoc(),
                     "function-level [[tapa::pipeline]] is ignored for the AI "
                     "Engine; put it on the loop to pipeline the loop there");
  }
}

void AieBackend::LowerStmtAttr(const clang::Attr& attr, const clang::Stmt* body,
                               EditSink& edits) const {
  // HLS only reports a trip-count range; chess optimizes on it, so on the AI
  // Engine a loop that runs outside it is undefined.
  if (const auto* tripcount = llvm::dyn_cast<clang::TapaTripcountAttr>(&attr);
      tripcount != nullptr && body != nullptr &&
      IsLoopBody(body, edits.getASTContext())) {
    edits.InsertTextBefore(body->getBeginLoc(),
                           "chess_loop_range(" +
                               std::to_string(tripcount->getMin()) + ", " +
                               std::to_string(tripcount->getMax()) + ") ");
    return;
  }
  // chess has no loop-nest merge, and its automatic step is the unroll that
  // LowerPipeline adds, which `unroll(k)` controls.
  if (attr.getKind() == clang::attr::TapaFlatten) {
    ReportCustomDiag(edits.getASTContext(), clang::DiagnosticsEngine::Remark,
                     attr.getRange().getBegin(),
                     "[[tapa::flatten]] is ignored for the AI Engine; a loop "
                     "nested in a pipelined loop is fully unrolled there "
                     "unless it carries [[tapa::unroll(k)]]");
    return;
  }
  ReportUnsupportedAttr(attr, edits);
}

void AieBackend::LowerDeclAttr(const clang::Attr& attr, const clang::VarDecl&,
                               const clang::DeclStmt&, EditSink& edits) const {
  if (attr.getKind() == clang::attr::TapaFrame) return;
  ReportUnsupportedAttr(attr, edits);
}

void AieBackend::LowerParamAttr(const clang::Attr& attr,
                                const clang::ParmVarDecl&, const clang::Stmt*,
                                EditSink& edits) const {
  if (attr.getKind() == clang::attr::TapaFrame) return;
  ReportUnsupportedAttr(attr, edits);
}
}  // namespace tapa::cc
