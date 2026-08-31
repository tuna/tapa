#ifndef TAPA_CODEGEN_WRAPPER_H_
#define TAPA_CODEGEN_WRAPPER_H_

#include "clang/AST/ASTContext.h"

#include "backend.h"
#include "frontend/program.h"

namespace tapa::cc {

// Generate the mangled wrapper for a template-specialization task: a concrete
// function `void <mangled>(<concrete params>) { <lower-level port preamble>
// <ReadableName>(<args>); }` that HLS can synthesize (a template can't be a
// top-level module).
std::string GenerateWrapper(const TaskModel& task, const Backend& backend,
                            clang::ASTContext& ctx);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_WRAPPER_H_
