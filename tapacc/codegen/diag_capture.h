#ifndef TAPA_CODEGEN_DIAG_CAPTURE_H_
#define TAPA_CODEGEN_DIAG_CAPTURE_H_

#include <string>
#include <vector>

#include "clang/Basic/Diagnostic.h"
#include "llvm/ADT/SmallVector.h"

namespace tapa::cc {

// Collects every error-level diagnostic as plain text, for tests that run a
// rewrite pipeline and assert on its refusals. The consumer must outlive the
// CompilerInstance whose diagnostics point at it.
class CollectingDiagConsumer : public clang::DiagnosticConsumer {
 public:
  std::vector<std::string> errors;

  void HandleDiagnostic(clang::DiagnosticsEngine::Level level,
                        const clang::Diagnostic& info) override {
    if (level < clang::DiagnosticsEngine::Error) return;
    llvm::SmallVector<char, 128> msg;
    info.FormatDiagnostic(msg);
    errors.emplace_back(msg.data(), msg.size());
  }
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_DIAG_CAPTURE_H_
