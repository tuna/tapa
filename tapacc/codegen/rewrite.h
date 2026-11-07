#ifndef TAPA_CODEGEN_REWRITE_H_
#define TAPA_CODEGEN_REWRITE_H_

#include <set>
#include <string>
#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/Basic/SourceLocation.h"
#include "llvm/ADT/StringRef.h"

#include "backend.h"
#include "frontend/program.h"

namespace tapa::cc {

class EditSink;
class TreeSession;

// One backend per synthesis target.
struct Backends {
  const Backend& hls;
  const Backend& vitis;
  const Backend& ignore;
  const Backend& aie;
};

// Apply the per-decl rewrite rules once across the mirrored files of one
// TU: each task definition is wrapped in its sanitized guard with an exact
// rewritten-signature stub in the else branch, helpers and unreachable tasks
// are rewritten unconditionally, and loop attributes are lowered to backend
// pragmas.
void RewriteTreeFiles(const Program& program, SynthTarget default_target,
                      const Backends& backends, clang::ASTContext& ctx,
                      TreeSession& session);

// Removes each `[[tapa::...]]` from code the preprocessor skipped, with a
// remark that it takes no effect there. tapacc never parses that code, so it
// cannot lower it, and a vendor compiling with other macros (chess defines
// TAPA_TARGET_AIE_, which tapacc does not) would otherwise get it verbatim.
// Only the `mirrored` files are edited.
void StripSkippedAttrs(const std::vector<clang::SourceRange>& skipped,
                       const std::set<clang::FileID>& mirrored,
                       clang::ASTContext& ctx, EditSink& edits);

// Reports any lowered attribute text left in one final emitted file. The
// same file renders in every TU that sights it, so the same leak reaches
// this scan once per TU; `reported` dedupes per file and spelling across
// the run (the caller owns the set, one per pipeline run).
void ReportLeakedAttrs(llvm::StringRef code, llvm::StringRef file,
                       clang::ASTContext& ctx, std::set<std::string>& reported);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_REWRITE_H_
