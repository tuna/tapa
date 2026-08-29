#ifndef TAPA_FRONTEND_DISCOVER_H_
#define TAPA_FRONTEND_DISCOVER_H_

#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Expr.h"
#include "clang/AST/ExprCXX.h"
#include "llvm/ADT/StringRef.h"

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
