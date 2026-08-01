#ifndef TAPA_CODEGEN_WRAPPER_H_
#define TAPA_CODEGEN_WRAPPER_H_

#include "clang/AST/ASTContext.h"
#include "clang/Rewrite/Core/Rewriter.h"

#include "frontend/program.h"
#include "backend.h"

namespace tapa::cc {

// Generate the mangled wrapper for a template-specialization task: a concrete
// function `void <mangled>(<concrete params>) { <lower-level port preamble>
// <ReadableName>(<args>); }` that HLS can synthesize (a template can't be a
// top-level module).
std::string GenerateWrapper(const TaskModel& task, const Backend& backend,
                            clang::ASTContext& ctx);

// Insert the wrapper right after the task's invoker function.
void InsertWrapper(const TaskModel& task, const Backend& backend,
                   clang::ASTContext& ctx, clang::Rewriter& rewriter);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_WRAPPER_H_
