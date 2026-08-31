#ifndef TAPA_FRONTEND_BUILD_PROGRAM_H_
#define TAPA_FRONTEND_BUILD_PROGRAM_H_

#include <set>
#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"

namespace tapa::cc {

// True when `loc` belongs to the selected source files. A null set selects
// only the main file; a concrete set selects the rewritten-tree mirror
// closure by FileID.
bool IsInSourceFiles(const clang::ASTContext& ctx, clang::SourceLocation loc,
                     const std::set<clang::FileID>* files);

// Global function definitions in the selected files, in source order. Drives
// both task discovery and codegen's source assembly.
std::vector<const clang::FunctionDecl*> CollectFileFuncs(
    const clang::ASTContext& ctx,
    const std::set<clang::FileID>* files = nullptr);

// Selected-file definitions with internal linkage (`static`, anonymous
// namespace) and methods — everything `CollectFileFuncs` leaves out. Helpers
// for codegen's purposes, but kept out of task discovery.
std::vector<const clang::FunctionDecl*> CollectLocalFuncs(
    const clang::ASTContext& ctx,
    const std::set<clang::FileID>* files = nullptr);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_BUILD_PROGRAM_H_
