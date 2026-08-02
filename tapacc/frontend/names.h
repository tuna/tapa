#ifndef TAPA_FRONTEND_NAMES_H_
#define TAPA_FRONTEND_NAMES_H_

#include <memory>
#include <string>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "clang/AST/Mangle.h"

namespace tapa::cc {

// The shared Itanium mangler used to mangle task names; one per ASTContext.
std::unique_ptr<clang::MangleContext> CreateMangleContext(
    clang::ASTContext& ctx);

// The mangled name for a task, prefixed "tapa_mangled" so the emitted symbol
// never starts with '_' (which Vitis rejects). Used for template
// specializations, whose plain names would collide.
std::string MangledTaskName(clang::MangleContext& mangler,
                            const clang::FunctionDecl* func);

// The human-readable (templated) name, e.g. "Compute<float, 4>".
std::string ReadableTaskName(const clang::ASTContext& ctx,
                             const clang::FunctionDecl* func);

// The task-map key: the mangled name for a template specialization, else the
// plain function name. Both discover and the invoke parser key on this, so
// instance edges reference the same task entries.
std::string TaskName(clang::MangleContext& mangler,
                     const clang::FunctionDecl* func);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_NAMES_H_
