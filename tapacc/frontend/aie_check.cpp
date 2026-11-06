#include "aie_check.h"

#include <algorithm>
#include <cstdint>
#include <functional>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <utility>
#include <vector>

#include "clang/AST/Attr.h"
#include "clang/AST/Expr.h"
#include "clang/AST/ExprCXX.h"
#include "clang/AST/RecursiveASTVisitor.h"

#include "classify.h"
#include "diag.h"

namespace tapa::cc {

namespace {

// The task parameter a stream expression names, seeing through implicit
// casts, `operator[]`, and local stream aliases.
std::optional<std::string> PortOf(
    const clang::Expr* expr,
    const std::map<std::string, std::string>& aliases) {
  expr = expr->IgnoreImplicit();
  if (const auto* call = llvm::dyn_cast<clang::CXXOperatorCallExpr>(expr)) {
    if (call->getOperator() == clang::OO_Subscript && call->getNumArgs() > 0) {
      return PortOf(call->getArg(0), aliases);
    }
  }
  if (const auto* ref = llvm::dyn_cast<clang::DeclRefExpr>(expr)) {
    if (const auto* param =
            llvm::dyn_cast<clang::ParmVarDecl>(ref->getDecl())) {
      return param->getNameAsString();
    }
    if (const auto* var = llvm::dyn_cast<clang::VarDecl>(ref->getDecl())) {
      if (const auto it = aliases.find(var->getNameAsString());
          it != aliases.end()) {
        return it->second;
      }
    }
  }
  return std::nullopt;
}

// Closed stream parameters of a task and its ordinary C++ helpers. A helper
// summary is mapped back through each call's actual arguments, so analysis
// stays on the TAPA stream that reaches the boundary rather than rejecting
// every unrelated close in the translation unit.
class CloseAnalysis {
 public:
  std::set<std::string> Closed(const clang::FunctionDecl* def) {
    if (def == nullptr) return {};
    def = def->getDefinition();
    if (def == nullptr || !def->hasBody()) return {};
    const clang::FunctionDecl* key = def->getCanonicalDecl();
    if (const auto it = cache_.find(key); it != cache_.end()) return it->second;
    if (!active_.insert(key).second) return {};

    std::map<std::string, std::string> aliases;
    std::set<std::string> closed;
    struct Visitor : public clang::RecursiveASTVisitor<Visitor> {
      Visitor(CloseAnalysis& self, std::map<std::string, std::string>& aliases,
              std::set<std::string>& closed)
          : self(self), aliases(aliases), closed(closed) {}

      CloseAnalysis& self;
      std::map<std::string, std::string>& aliases;
      std::set<std::string>& closed;

      bool VisitVarDecl(clang::VarDecl* var) {
        if (const clang::Expr* init = var->getInit()) {
          if (auto name = PortOf(init, aliases)) {
            aliases[var->getNameAsString()] = *name;
          }
        }
        return true;
      }

      bool VisitCXXMemberCallExpr(clang::CXXMemberCallExpr* call) {
        const clang::CXXMethodDecl* method = call->getMethodDecl();
        if (method != nullptr && method->getDeclName().isIdentifier()) {
          const llvm::StringRef op = method->getName();
          if (op == "close" || op == "try_close") {
            if (auto name =
                    PortOf(call->getImplicitObjectArgument(), aliases)) {
              closed.insert(*name);
            }
          }
        }
        return true;
      }

      bool VisitCallExpr(clang::CallExpr* call) {
        const clang::FunctionDecl* callee = call->getDirectCallee();
        if (callee == nullptr) return true;
        const auto helper = self.Closed(callee);
        for (unsigned index = 0; index < callee->getNumParams(); ++index) {
          const clang::ParmVarDecl* formal = callee->getParamDecl(index);
          if (helper.count(formal->getNameAsString()) == 0 ||
              index >= call->getNumArgs()) {
            continue;
          }
          if (auto name = PortOf(call->getArg(index), aliases)) {
            closed.insert(*name);
          }
        }
        return true;
      }
    } visitor{*this, aliases, closed};
    visitor.TraverseStmt(def->getBody());
    active_.erase(key);
    return cache_[key] = std::move(closed);
  }

 private:
  std::map<const clang::FunctionDecl*, std::set<std::string>> cache_;
  std::set<const clang::FunctionDecl*> active_;
};

// Why a stream operation has no AI Engine form, or null when it has one.
const char* AieStreamOpProblem(llvm::StringRef op, unsigned num_args) {
  // `read(bool&)` and `read(nullptr)`; the bulk `read(dst, n)` blocks.
  if (op == "read" && num_args == 1) {
    return "AI Engine stream reads are blocking; use read() without "
           "arguments";
  }
  for (const char* eot :
       {"close", "try_close", "open", "try_open", "eot", "try_eot"}) {
    if (op == eot) {
      return "AI Engine streams carry no end-of-transmission token; loop "
             "over an element count instead";
    }
  }
  for (const char* nonblocking : {"peek", "try_peek", "empty", "full"}) {
    if (op == nonblocking) {
      return "AI Engine streams are blocking and expose no occupancy; use "
             "the blocking read() and write()";
    }
  }
  return nullptr;
}

// Every member call on a TAPA stream in a body.
class StreamCalls : public clang::RecursiveASTVisitor<StreamCalls> {
 public:
  template <typename F>
  explicit StreamCalls(F&& visit) : visit_(std::forward<F>(visit)) {}

  bool VisitCXXMemberCallExpr(clang::CXXMemberCallExpr* call) {
    const clang::Expr* object = call->getImplicitObjectArgument();
    const clang::CXXMethodDecl* method = call->getMethodDecl();
    if (object != nullptr && method != nullptr &&
        method->getDeclName().isIdentifier()) {
      const TapaKind kind = ClassifyTapaType(object->getType());
      if (IsStreamInterface(kind) || IsStreamInstance(kind)) visit_(*call);
    }
    return true;
  }

 private:
  std::function<void(const clang::CXXMemberCallExpr&)> visit_;
};

class LocalVars : public clang::RecursiveASTVisitor<LocalVars> {
 public:
  explicit LocalVars(const clang::ASTContext& ctx) : ctx_(ctx) {}

  bool VisitVarDecl(clang::VarDecl* var) {
    const clang::QualType type = var->getType();
    if (var->isLocalVarDecl() && !var->isStaticLocal() &&
        !type->isDependentType() && !type->isIncompleteType() &&
        type->isConstantSizeType()) {
      bytes_ +=
          static_cast<uint64_t>(ctx_.getTypeSizeInChars(type).getQuantity());
    }
    return true;
  }

  uint64_t bytes() const { return bytes_; }

 private:
  const clang::ASTContext& ctx_;
  uint64_t bytes_ = 0;
};

}  // namespace

void CheckAieTask(const clang::ASTContext& ctx, const TaskModel& task) {
  for (const clang::ParmVarDecl* param : task.def->parameters()) {
    const TapaKind kind = ClassifyTapaType(param);
    if (IsMmapInterface(kind) && !IsAsyncMmap(kind)) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                       param->getLocation(),
                       "AI Engine task '%0' takes the memory port '%1'; an AI "
                       "Engine task reaches memory through a "
                       "'tapa::async_mmap<T>&' port, which TAPA serves from "
                       "the PL")
          << task.name << param->getNameAsString();
      continue;
    }
    const Port* port = FindPort(task, param->getNameAsString());
    if (IsAsyncMmap(kind)) {
      if (port != nullptr && port->width != 32 && port->width != 64 &&
          port->width != 128) {
        ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                         param->getLocation(),
                         "async_mmap '%0' of AI Engine task '%1' has %2-bit "
                         "elements; an AI Engine memory element is 32, 64 or "
                         "128 bits, the widths that cross a PLIO and line up "
                         "with a 128-bit request")
            << param->getNameAsString() << task.name << port->width;
      }
      continue;
    }
    if (!IsStreamInterface(kind)) continue;
    if (port != nullptr && (port->width == 0 || port->width % 32 != 0)) {
      ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error,
                       param->getLocation(),
                       "stream '%0' of AI Engine task '%1' carries %2-bit "
                       "elements; an AI Engine stream element must be a "
                       "multiple of 32 bits, so pack narrower data into a "
                       "tapa::vec_t or a wider integer")
          << param->getNameAsString() << task.name << port->width;
    }
  }
  if (task.level != TaskLevel::kLower || !task.def->hasBody()) return;
  StreamCalls calls([&](const clang::CXXMemberCallExpr& call) {
    const llvm::StringRef op = call.getMethodDecl()->getName();
    const char* problem = AieStreamOpProblem(op, call.getNumArgs());
    if (problem == nullptr) return;
    ReportCustomDiag(ctx, clang::DiagnosticsEngine::Error, call.getExprLoc(),
                     "'%0' in AI Engine task '%1' has no AI Engine form: %2")
        << op << task.name << problem;
  });
  calls.TraverseStmt(task.def->getBody());
}

namespace {

// How a body names one parameter: through which members, and whether it
// is ever named otherwise.
class MemoryUseVisitor : public clang::RecursiveASTVisitor<MemoryUseVisitor> {
 public:
  explicit MemoryUseVisitor(const clang::ParmVarDecl* param) : param_(param) {}

  bool VisitMemberExpr(clang::MemberExpr* member) {
    const auto* ref =
        llvm::dyn_cast<clang::DeclRefExpr>(member->getBase()->IgnoreImplicit());
    if (ref == nullptr || ref->getDecl() != param_) return true;
    through_member_.insert(ref);
    const std::string name = member->getMemberDecl()->getNameAsString();
    if (name == "read_addr" || name == "read_data") {
      use_.reads = true;
    } else if (name == "write_addr" || name == "write_data") {
      use_.writes = true;
    } else if (name == "write_resp") {
      use_.writes = true;
      use_.responses = true;
    } else {
      use_ = {true, true, true};
    }
    return true;
  }

  bool VisitDeclRefExpr(clang::DeclRefExpr* ref) {
    if (ref->getDecl() == param_) refs_.push_back(ref);
    return true;
  }

  MemoryUse Result() const {
    for (const clang::DeclRefExpr* ref : refs_) {
      if (through_member_.count(ref) == 0) return {true, true, true};
    }
    return use_;
  }

 private:
  const clang::ParmVarDecl* param_;
  MemoryUse use_;
  std::vector<const clang::DeclRefExpr*> refs_;
  std::set<const clang::DeclRefExpr*> through_member_;
};

}  // namespace

MemoryUse UsedDirections(const clang::FunctionDecl* def,
                         const clang::ParmVarDecl* param) {
  if (def == nullptr || !def->hasBody()) return {true, true, true};
  MemoryUseVisitor visitor(param);
  visitor.TraverseStmt(def->getBody());
  return visitor.Result();
}

uint32_t LocalBytes(const clang::ASTContext& ctx,
                    const clang::FunctionDecl* def) {
  LocalVars locals(ctx);
  locals.TraverseStmt(def->getBody());
  return static_cast<uint32_t>(std::min<uint64_t>(locals.bytes(), UINT32_MAX));
}

std::set<std::string> ClosedPorts(const clang::FunctionDecl* def) {
  CloseAnalysis analysis;
  return analysis.Closed(def);
}

}  // namespace tapa::cc
