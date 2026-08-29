#ifndef TAPA_CODEGEN_REWRITE_H_
#define TAPA_CODEGEN_REWRITE_H_

#include <string>

#include "clang/AST/ASTContext.h"

#include "backend.h"
#include "frontend/program.h"

namespace tapa::cc {

// Emit the self-contained vendor C++ for one task: the task itself fully
// rewritten (signature + body/shell), every other task reduced to a signature,
// non-task helpers rewritten, and loop attributes lowered to backend pragmas.
// A single pass over the model drives one clang::Rewriter -- no per-task
// re-traversal of the AST. Returns the rewritten main-file text.
std::string EmitTaskCode(const Program& program, const TaskModel& task,
                         const Backend& backend, clang::ASTContext& ctx);

// The shared variant of the same text: no current task, so EVERY task is
// reduced to its rewritten signature while helpers keep their rewritten
// definitions. A task blob built from another translation unit carries that
// TU's helper definitions only as declarations, so each TU also emits this
// file to hand them to every other TU's HLS job. `file_name` labels the
// emitted text in diagnostics.
std::string EmitSharedCode(const Program& program, const Backend& backend,
                           clang::ASTContext& ctx,
                           const std::string& file_name);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_REWRITE_H_
