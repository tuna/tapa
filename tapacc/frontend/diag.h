// Copyright (c) 2026 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#ifndef TAPA_FRONTEND_DIAG_H_
#define TAPA_FRONTEND_DIAG_H_

#include "clang/AST/ASTContext.h"
#include "clang/Basic/Diagnostic.h"
#include "clang/Basic/SourceLocation.h"

namespace tapa::cc {

// clang's getCustomDiagID takes the format as a string literal (templated on
// its length), so the format is a template parameter here, not a StringRef.
// Returns the DiagnosticBuilder so callers can attach %0/%1 argument strings;
// the diagnostic is emitted when the builder goes out of scope.
template <unsigned N>
clang::DiagnosticBuilder ReportCustomDiag(
    const clang::ASTContext& ctx, clang::DiagnosticsEngine::Level level,
    clang::SourceLocation loc, const char (&fmt)[N]) {
  clang::DiagnosticsEngine& diags = ctx.getDiagnostics();
  return diags.Report(loc, diags.getCustomDiagID(level, fmt));
}

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_DIAG_H_
