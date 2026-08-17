#ifndef TAPA_FRONTEND_BUILD_PROGRAM_H_
#define TAPA_FRONTEND_BUILD_PROGRAM_H_

#include <vector>

#include "clang/AST/ASTContext.h"
#include "clang/AST/Decl.h"
#include "llvm/ADT/StringRef.h"

#include "program.h"

namespace tapa::cc {

// Global function definitions in the main file, in source order. Drives both
// task discovery and codegen's per-task file assembly.
std::vector<const clang::FunctionDecl*> CollectFileFuncs(
    const clang::ASTContext& ctx);

// Main-file definitions with internal linkage (`static`, anonymous
// namespace) and methods — everything `CollectFileFuncs` leaves out. Helpers
// for codegen's purposes, but kept out of task discovery.
std::vector<const clang::FunctionDecl*> CollectLocalFuncs(
    const clang::ASTContext& ctx);

// Build the whole typed Program from a parsed translation unit: discover the
// tasks reachable from `top`, extract each task's ports, and parse every upper
// task's instances and streams. The single frontend entry point (AST -> model,
// no source rewriting).
Program BuildProgram(clang::ASTContext& ctx, llvm::StringRef top,
                     SynthTarget default_target);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_BUILD_PROGRAM_H_
