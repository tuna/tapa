#ifndef TAPA_CODEGEN_EMIT_H_
#define TAPA_CODEGEN_EMIT_H_

#include <string>

#include "clang/AST/Decl.h"
#include "clang/AST/Stmt.h"
#include "clang/Rewrite/Core/Rewriter.h"

#include "code_sink.h"
#include "frontend/classify.h"

namespace tapa::cc {

// Emit dummy reads/writes (and peeks) of a stream port so a vendor compiler
// does not optimize the port away. `qdma` selects the Vitis AXI-stream form
// (no peek; a qdma_axis write type).
void EmitDummyStreamRW(const clang::ParmVarDecl* param, TapaKind kind,
                       CodeSink& out, bool qdma);

// Emit a dummy volatile read of an mmap offset / scalar (or each channel of an
// mmap array) so the port survives dead-code elimination.
void EmitDummyMmapOrScalarRW(const clang::ParmVarDecl* param, TapaKind kind,
                             CodeSink& out);

// Insert an HLS pragma at the start of a loop body (after the opening brace of
// a compound statement, or via `_Pragma` before a single statement).
void AddPragmaToBody(clang::Rewriter& rewriter, const clang::Stmt* body,
                     const std::string& pragma);

// Insert an HLS pragma line just after a statement (e.g. a stream declaration).
void AddPragmaAfterStmt(clang::Rewriter& rewriter, const clang::Stmt* stmt,
                        const std::string& pragma);

// Vitis HLS rejects `inline` on task functions; strips the leading keyword
// from this declaration's spelling (call for every redeclaration).
void RemoveInline(const clang::FunctionDecl* func, clang::Rewriter& rewriter);

// Remove a lowered `[[tapa::...]]` attribute's source text, swallowing the
// surrounding whitespace, a trailing/leading comma, or an enclosing `[[ ]]` so
// the result is clean. Takes the attribute's own source range.
void RemoveLoweredAttr(clang::Rewriter& rewriter,
                       clang::SourceRange attr_range);

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_EMIT_H_
