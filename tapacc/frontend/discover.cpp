#include "discover.h"

#include "clang/AST/Attr.h"

#include "classify.h"

namespace tapa::cc {

namespace {

// Whether a member call is `tapa::task::invoke`.
bool IsTaskInvoke(const clang::CXXMemberCallExpr* call) {
  const clang::CXXRecordDecl* record = call->getRecordDecl();
  const clang::CXXMethodDecl* method = call->getMethodDecl();
  return record != nullptr && method != nullptr &&
         record->getQualifiedNameAsString() == "tapa::task" &&
         method->getName() == "invoke";
}

void GetInvokesInto(const clang::Stmt* stmt,
                    std::vector<const clang::CXXMemberCallExpr*>& out) {
  if (stmt == nullptr) return;
  for (const clang::Stmt* child : stmt->children()) {
    GetInvokesInto(child, out);
  }
  if (const auto* call = llvm::dyn_cast<clang::CXXMemberCallExpr>(stmt)) {
    if (IsTaskInvoke(call)) out.push_back(call);
  }
}

}  // namespace

const clang::Expr* GetTapaTaskObject(const clang::Stmt* body) {
  if (body == nullptr) return nullptr;
  for (const clang::Stmt* child : body->children()) {
    if (const auto* expr = llvm::dyn_cast_or_null<clang::Expr>(child)) {
      if (ClassifyTapaType(expr->getType()) == TapaKind::kTask) {
        return expr;
      }
    }
  }
  return nullptr;
}

std::vector<const clang::CXXMemberCallExpr*> GetInvokes(
    const clang::Stmt* stmt) {
  std::vector<const clang::CXXMemberCallExpr*> out;
  GetInvokesInto(stmt, out);
  return out;
}

const clang::FunctionDecl* InvokeCallee(
    const clang::CXXMemberCallExpr* invoke) {
  if (invoke->getNumArgs() == 0) return nullptr;
  const clang::Expr* arg0 = invoke->getArg(0)->IgnoreImplicit();
  const auto* ref = llvm::dyn_cast<clang::DeclRefExpr>(arg0);
  if (ref == nullptr) return nullptr;
  return llvm::dyn_cast<clang::FunctionDecl>(ref->getDecl());
}

bool IsIgnored(const clang::FunctionDecl* func) {
  if (const auto* attr = func->getAttr<clang::TapaTargetAttr>()) {
    return attr->getTarget() == clang::TapaTargetAttr::TargetType::Ignore;
  }
  return false;
}

TaskLevel LevelOf(const clang::FunctionDecl* func) {
  if (!IsIgnored(func) && GetTapaTaskObject(func->getBody()) != nullptr) {
    return TaskLevel::kUpper;
  }
  return TaskLevel::kLower;
}

SynthTarget ResolveTarget(const clang::FunctionDecl* func,
                          SynthTarget default_target) {
  const auto* attr = func->getAttr<clang::TapaTargetAttr>();
  if (attr == nullptr) return default_target;
  using TT = clang::TapaTargetAttr::TargetType;
  switch (attr->getTarget()) {
    case TT::XilinxHLS:
      return SynthTarget::kXilinxHls;
    case TT::XilinxVitis:
      return SynthTarget::kXilinxVitis;
    case TT::Ignore:
      return SynthTarget::kIgnore;
    default:
      return default_target;
  }
}

}  // namespace tapa::cc
