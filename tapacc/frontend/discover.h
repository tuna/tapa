#ifndef TAPA_FRONTEND_DISCOVER_H_
#define TAPA_FRONTEND_DISCOVER_H_

#include <utility>
#include <vector>

#include "clang/AST/Attr.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Expr.h"
#include "clang/AST/ExprCXX.h"

#include "program.h"

namespace tapa::cc {

// The `tapa::task` builder object in a function body, or nullptr if the body
// contains none (i.e. the function is a leaf, not an upper-level task).
// Structural: the child expression's type is classified as `kTask`, not
// compared against the canonical string "struct tapa::task".
const clang::Expr* GetTapaTaskObject(const clang::Stmt* body);

// Every `tapa::task::invoke(...)` call under a statement, in source (DFS)
// order.
std::vector<const clang::CXXMemberCallExpr*> GetInvokes(
    const clang::Stmt* stmt);

// The task function an invoke references, or nullptr if its first argument is
// not a plain function reference. A cross-TU invoke yields the declaration
// visible in the invoking TU, not the definition.
const clang::FunctionDecl* InvokeCallee(const clang::CXXMemberCallExpr* invoke);

// Whether an invoke names its instance: a `const char (&)[N]` parameter right
// after the callable, never a port.
bool InvokeNamesInstance(const clang::CXXMemberCallExpr* invoke);

// The port arguments of an invoke, each with the index of the callee
// parameter it binds: every argument after the callable, the instance name
// and a `tapa::executable`.
std::vector<std::pair<unsigned, const clang::Expr*>> InvokePortArgs(
    const clang::CXXMemberCallExpr* invoke);

// The body of a loop statement, or nullptr if it is not a loop.
const clang::Stmt* LoopBody(const clang::Stmt* stmt);

// The `[[tapa::target]]` written on any declaration of a function, the
// definition's first: the attribute does not inherit across redeclarations,
// and a header declaring a task is where it is usually written.
const clang::TapaTargetAttr* TargetAttr(const clang::FunctionDecl* func);

// Attribute `A` written on parameter `index` of any declaration of `func`,
// the definition's first.
template <typename A>
const A* ParamAttr(const clang::FunctionDecl* func, unsigned index) {
  const clang::FunctionDecl* def = func->getDefinition();
  if (def != nullptr && index < def->getNumParams()) {
    if (const A* attr = def->getParamDecl(index)->getAttr<A>()) return attr;
  }
  for (const clang::FunctionDecl* decl : func->redecls()) {
    if (index >= decl->getNumParams()) continue;
    if (const A* attr = decl->getParamDecl(index)->getAttr<A>()) return attr;
  }
  return nullptr;
}

// Whether a function is marked `[[tapa::target("ignore")]]`.
bool IsIgnored(const clang::FunctionDecl* func);

// The upper/lower level of a task function: upper iff its body holds a
// `tapa::task` object and it is not ignored.
TaskLevel LevelOf(const clang::FunctionDecl* func);

// The synthesis target for a task: its `[[tapa::target(...)]]` override, else
// the tool-wide default.
SynthTarget ResolveTarget(const clang::FunctionDecl* func,
                          SynthTarget default_target);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_DISCOVER_H_
