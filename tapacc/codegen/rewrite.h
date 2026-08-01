#ifndef TAPA_CODEGEN_REWRITE_H_
#define TAPA_CODEGEN_REWRITE_H_

#include <string>

#include "clang/AST/ASTContext.h"

#include "frontend/program.h"
#include "backend.h"

namespace tapa::cc {

// Emit the self-contained vendor C++ for one task: the task itself fully
// rewritten (signature + body/shell), every other task reduced to a signature,
// non-task helpers rewritten, and loop attributes lowered to backend pragmas.
// A single pass over the model drives one clang::Rewriter -- no per-task
// re-traversal of the AST. Returns the rewritten main-file text.
std::string EmitTaskCode(const Program& program, const TaskModel& task,
                         const Backend& backend, clang::ASTContext& ctx);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_REWRITE_H_
