#ifndef TAPA_FRONTEND_DISCOVER_H_
#define TAPA_FRONTEND_DISCOVER_H_

#include <map>
#include <string>
#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Expr.h"
#include "clang/AST/ExprCXX.h"
#include "llvm/ADT/ArrayRef.h"
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

// Whether a function is marked `[[tapa::target("ignore")]]`.
bool IsIgnored(const clang::FunctionDecl* func);

// The upper/lower level of a task function: upper iff its body holds a
// `tapa::task` object and it is not ignored.
TaskLevel LevelOf(const clang::FunctionDecl* func);

// The synthesis target for a task: its `[[tapa::target(...)]]` override, else
// the tool-wide default.
SynthTarget ResolveTarget(const clang::FunctionDecl* func,
                          SynthTarget default_target);

// Discover every TAPA task reachable from `top` (breadth-first over `invoke`
// edges), keyed by task name, with the graph fields populated: def, invoker,
// is_template_spec, name (mangled for specializations), readable_name, level,
// target. Ports and instances/streams are filled by later passes. Reports
// top-not-found / top-ignored / task-redefinition through `ctx` diagnostics and
// returns an empty map on a fatal error.
std::map<std::string, TaskModel> DiscoverTasks(
    clang::ASTContext& ctx, llvm::StringRef top_name,
    SynthTarget default_target,
    llvm::ArrayRef<const clang::FunctionDecl*> file_funcs);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_DISCOVER_H_
