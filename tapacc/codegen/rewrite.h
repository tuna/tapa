#ifndef TAPA_CODEGEN_REWRITE_H_
#define TAPA_CODEGEN_REWRITE_H_

#include <string>

#include "clang/AST/ASTContext.h"
#include "llvm/ADT/StringRef.h"

#include "backend.h"
#include "frontend/program.h"

namespace tapa::cc {

class TreeSession;

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

// Tree-mode assembly: applies the same per-decl rules as EmitTaskCode once
// across the mirrored files, wrapping each task definition in its sanitized
// guard and emitting exact rewritten-signature stubs in the else branches.
void RewriteTreeFiles(const Program& program, SynthTarget default_target,
                      const Backend& hls, const Backend& vitis,
                      const Backend& ignore, clang::ASTContext& ctx,
                      TreeSession& session);

// Reports any lowered attribute text left in one final emitted file.
void ReportLeakedAttrs(llvm::StringRef code, llvm::StringRef label,
                       clang::ASTContext& ctx);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_REWRITE_H_
