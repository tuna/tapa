#include "discover.h"

#include <map>
#include <memory>
#include <queue>
#include <string>

#include "clang/AST/Attr.h"
#include "clang/AST/Mangle.h"

#include "classify.h"
#include "diag.h"
#include "names.h"

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

// The callee task of an invoke: the function referenced by the first argument,
// or nullptr if it is not a plain function reference (never dereferences a
// failed cast — §8.3).
const clang::FunctionDecl* InvokeCallee(
    const clang::CXXMemberCallExpr* invoke) {
  if (invoke->getNumArgs() == 0) return nullptr;
  const clang::Expr* arg0 = invoke->getArg(0)->IgnoreImplicit();
  const auto* ref = llvm::dyn_cast<clang::DeclRefExpr>(arg0);
  if (ref == nullptr) return nullptr;
  return llvm::dyn_cast<clang::FunctionDecl>(ref->getDecl());
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

std::map<std::string, TaskModel> DiscoverTasks(
    clang::ASTContext& ctx, llvm::StringRef top_name,
    SynthTarget default_target,
    llvm::ArrayRef<const clang::FunctionDecl*> file_funcs) {
  // Index definitions by name so redefinitions can be detected and the top
  // task located.
  std::multimap<std::string, const clang::FunctionDecl*> defs;
  for (const clang::FunctionDecl* f : file_funcs) {
    if (f->isThisDeclarationADefinition()) {
      defs.emplace(f->getNameAsString(), f);
    }
  }

  const auto top_it = defs.find(top_name.str());
  if (top_it == defs.end()) {
    auto builder = ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, {},
                                    "top-level task '%0' not found");
    builder.AddString(top_name);
    return {};
  }
  const clang::FunctionDecl* top_func = top_it->second;
  if (IsIgnored(top_func)) {
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                     top_func->getLocation(),
                     "tapa top-level task function cannot be ignored");
    return {};
  }

  const auto mangler = CreateMangleContext(ctx);

  auto make_model = [&](const clang::FunctionDecl* func,
                        const clang::FunctionDecl* invoker,
                        bool is_spec) -> TaskModel {
    TaskModel m;
    m.def = func;
    m.invoker = invoker;
    m.is_template_spec = is_spec;
    if (is_spec) {
      m.name = MangledTaskName(*mangler, func);
      m.readable_name = ReadableTaskName(ctx, func);
    } else {
      m.name = func->getNameAsString();
      m.readable_name = m.name;
    }
    m.level = LevelOf(func);
    m.target = ResolveTarget(func, default_target);
    return m;
  };

  std::map<std::string, TaskModel> tasks;
  std::queue<const clang::FunctionDecl*> work;

  const TaskModel top_model = make_model(top_func, nullptr, /*is_spec=*/false);
  tasks.emplace(top_model.name, top_model);
  work.push(top_func);

  while (!work.empty()) {
    const clang::FunctionDecl* upper = work.front();
    work.pop();
    if (IsIgnored(upper)) continue;
    const clang::Expr* task_obj = GetTapaTaskObject(upper->getBody());
    if (task_obj == nullptr) continue;  // leaf task

    for (const clang::CXXMemberCallExpr* invoke : GetInvokes(task_obj)) {
      const clang::FunctionDecl* child = InvokeCallee(invoke);
      if (child == nullptr || !child->isThisDeclarationADefinition()) continue;
      const bool is_spec = child->isFunctionTemplateSpecialization();
      TaskModel m = make_model(child, is_spec ? upper : nullptr, is_spec);
      if (tasks.find(m.name) == tasks.end()) {
        tasks.emplace(m.name, std::move(m));
        work.push(child);
      }
    }
  }

  // A task function must have exactly one definition.
  for (const auto& [name, model] : tasks) {
    if (defs.count(model.def->getNameAsString()) > 1) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                       model.def->getLocation(), "task '%0' re-defined")
          .AddString(model.def->getNameAsString());
    }
  }

  return tasks;
}

}  // namespace tapa::cc
