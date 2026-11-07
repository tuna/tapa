#ifndef TAPA_CODEGEN_BUILDER_ACTION_H_
#define TAPA_CODEGEN_BUILDER_ACTION_H_

#include <memory>
#include <utility>
#include <vector>

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Basic/Diagnostic.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/Syntax/Tokens.h"

#include "frontend/program_builder.h"
#include "frontend/vendor_scan.h"
#include "macro_splice.h"
#include "tree_writer.h"

namespace tapa::cc {

// One frontend action over one TU, feeding the shared builder. The include
// log is always recorded; only the rewrite pass records the expanded token
// stream (selective macro expansion needs it, and only the rewrite pass
// edits). The index pass carries the vendor scan when `vendor_scan` is set
// (attached once per TU, preprocessor phase included, so its warnings are
// not duplicated by the rewrite pass). `diags`, when given, replaces the
// compiler's diagnostic client so an in-process harness collects diagnostics
// instead of printing them.
class BuilderAction : public clang::ASTFrontendAction {
 public:
  BuilderAction(ProgramBuilder& builder, bool index_pass, bool vendor_scan,
                clang::DiagnosticConsumer* diags = nullptr)
      : builder_(builder),
        index_pass_(index_pass),
        vendor_scan_(vendor_scan),
        diags_(diags) {}

  bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
    if (diags_ != nullptr) {
      ci.getDiagnostics().setClient(diags_, /*OwnsClient=*/false);
    }
    if (index_pass_ && vendor_scan_) AttachVendorScan(ci.getPreprocessor());
    ci.getPreprocessor().addPPCallbacks(std::make_unique<IncludeRecorder>(
        ci.getSourceManager(), &include_log_));
    if (!index_pass_) {
      recorder_ = std::make_unique<TokenRecorder>(ci.getPreprocessor());
      ci.getPreprocessor().addPPCallbacks(
          std::make_unique<SkippedRangeRecorder>(&skipped_));
    }
    return true;
  }

  std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
      clang::CompilerInstance&, llvm::StringRef) override {
    class Consumer : public clang::ASTConsumer {
     public:
      explicit Consumer(BuilderAction& owner) : owner_(owner) {}
      void HandleTranslationUnit(clang::ASTContext& ctx) override {
        // Consume before any rewriting: the collector is complete only once
        // the whole top-level loop has run.
        const clang::syntax::TokenBuffer* tokens =
            owner_.recorder_ == nullptr ? nullptr
                                        : &owner_.recorder_->Consume();
        if (owner_.index_pass_) {
          if (owner_.vendor_scan_) ScanVendorAsts(ctx);
          owner_.builder_.IndexTu(ctx, owner_.include_log_);
        } else {
          owner_.builder_.RewriteTreeTu(ctx, std::move(owner_.include_log_),
                                        tokens, owner_.skipped_);
        }
      }

     private:
      BuilderAction& owner_;
    };
    return std::make_unique<Consumer>(*this);
  }

 private:
  ProgramBuilder& builder_;
  bool index_pass_;
  bool vendor_scan_;
  clang::DiagnosticConsumer* diags_;
  std::vector<IncludeDirective> include_log_;
  std::unique_ptr<TokenRecorder> recorder_;
  std::vector<clang::SourceRange> skipped_;  // the rewrite pass only
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_BUILDER_ACTION_H_
