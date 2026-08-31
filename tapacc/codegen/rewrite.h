#ifndef TAPA_CODEGEN_REWRITE_H_
#define TAPA_CODEGEN_REWRITE_H_

#include <string>

#include "clang/AST/ASTContext.h"
#include "llvm/ADT/StringRef.h"

#include "backend.h"
#include "frontend/program.h"

namespace tapa::cc {

class TreeSession;

// Apply the per-decl rewrite rules once across the mirrored files of one
// TU: each task definition is wrapped in its sanitized guard with an exact
// rewritten-signature stub in the else branch, helpers and unreachable tasks
// are rewritten unconditionally, and loop attributes are lowered to backend
// pragmas.
void RewriteTreeFiles(const Program& program, SynthTarget default_target,
                      const Backend& hls, const Backend& vitis,
                      const Backend& ignore, clang::ASTContext& ctx,
                      TreeSession& session);

// Reports any lowered attribute text left in one final emitted file.
void ReportLeakedAttrs(llvm::StringRef code, llvm::StringRef label,
                       clang::ASTContext& ctx);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_REWRITE_H_
