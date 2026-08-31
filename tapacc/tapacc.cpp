// tapacc: the TAPA C-to-HLS rewriter. Parses every input translation unit
// into one merged task graph and emits that single graph on stdout for the
// tapa-ir crate to consume:
// {top, target, tasks:{name:{srcs, include_dirs, defines, level, synth,
// readable_name, ports, tasks, fifos}}}. `tapa analyze` nests that payload
// under the "graph" key of the work dir's tapa.json.
//
// The inputs are the user's original translation units, any number of TUs in
// one run: tapacc mirrors the union of the TUs' non-system include closures
// under `-emit-dir`, rewrites declarations/helpers once per file, and wraps
// every task definition in its `TAPA_TASK_DEF_*` guard. Every task manifest
// then names the same mirrored TUs and selects one definition through its
// guard. A file sighted by several TUs is rendered once, from the first TU
// in input order, and every later TU's rendering of it must agree byte for
// byte.
//
// Each ClangTool action owns its ASTContext and AST nodes never outlive their
// TU, so the pipeline runs as TWO passes over the same file list around a
// pure-data merge (frontend/program_builder.{h,cpp}):
//
//   1. an index action per TU: collect definitions and invoke edges from the
//      TU's mirror closure;
//   2. the merge: one definition index keyed by function identity plus
//      canonical definition path/offset, BFS from `--top`, and the
//      merge-rule diagnostics;
//   3. a rewrite action per TU: apply one guarded per-file rewrite and
//      absorb its live include log.
//
// The single JSON prints only when both passes ran clean for every TU, so a
// program that fails a merge rule or a rewrite diagnostic exits non-zero
// with no graph on stdout.
//
// Two distinct notions of "target" live in that schema, and the tapa-ir
// crate parses both as closed enums with deny_unknown_fields:
//   - root "target": the vendor FLOW, kebab-case "xilinx-vitis"/"xilinx-hls".
//   - per-task "synth": the synthesis POLICY, "hls"/"ignore" only -- it answers
//     just "synthesize or skip". The internal three-valued SynthTarget still
//     carries the flow per task so the right backend gets picked, but that
//     distinction collapses on the wire.
//
// Changing what this file emits changes a contract with Rust code that cannot
// see it. `bazel test //tapa-core:tapacc_conformance_test` (Linux only) is the
// guard: it runs this binary on tests/apps/vadd/vadd.cpp and strict-parses the
// stdout below with tapa-ir's real types, so a field tapa-ir does not model --
// or a required one that stops being emitted -- fails there rather than in
// someone's build.

#include <iostream>
#include <memory>
#include <utility>
#include <vector>

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/CommonOptionsParser.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/CommandLine.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/raw_ostream.h"

#include "codegen/macro_splice.h"
#include "codegen/tree_writer.h"
#include "frontend/program_builder.h"
#include "frontend/vendor_scan.h"

namespace {

using namespace tapa::cc;

llvm::cl::OptionCategory g_category("tapacc-ng options");

llvm::cl::opt<std::string> g_top("top", llvm::cl::Required,
                                 llvm::cl::desc("Top-level task name"),
                                 llvm::cl::cat(g_category));

llvm::cl::opt<bool> g_no_vendor_scan(
    "no-vendor-scan", llvm::cl::init(false),
    llvm::cl::desc("Disable the vendor-usage soft warnings"),
    llvm::cl::cat(g_category));

// Where the mirrored guarded source tree is materialized. The caller
// creates the directory; tapacc refuses to run if it is missing.
llvm::cl::opt<std::string> g_emit_dir(
    "emit-dir", llvm::cl::Required,
    llvm::cl::desc("Directory to write rewritten sources to"),
    llvm::cl::value_desc("dir"), llvm::cl::cat(g_category));

enum class CliTarget { kHls, kVitis };
llvm::cl::opt<CliTarget> g_target(
    "target", llvm::cl::desc("Target flow (default xilinx-hls)"),
    llvm::cl::init(CliTarget::kHls), llvm::cl::cat(g_category),
    llvm::cl::values(
        clEnumValN(CliTarget::kHls, "xilinx-hls", "Xilinx HLS (default)"),
        clEnumValN(CliTarget::kVitis, "xilinx-vitis", "Xilinx Vitis")));

// One frontend action over one TU, feeding the shared builder. The index
// pass also carries the vendor scan (attached once per TU, preprocessor
// phase included, so its warnings are not duplicated by the rewrite pass).
class BuilderAction : public clang::ASTFrontendAction {
 public:
  BuilderAction(ProgramBuilder& builder, bool index_pass)
      : builder_(builder), index_pass_(index_pass) {}

  bool BeginSourceFileAction(clang::CompilerInstance& ci) override {
    if (index_pass_ && !g_no_vendor_scan)
      AttachVendorScan(ci.getPreprocessor());
    ci.getPreprocessor().addPPCallbacks(std::make_unique<IncludeRecorder>(
        ci.getSourceManager(), &include_log_));
    // Selective macro expansion needs the expanded token stream; only the
    // rewrite pass edits, so only it records.
    if (!index_pass_) {
      recorder_ = std::make_unique<TokenRecorder>(ci.getPreprocessor());
    }
    return true;
  }

  std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
      clang::CompilerInstance&, llvm::StringRef) override {
    class Consumer : public clang::ASTConsumer {
     public:
      explicit Consumer(BuilderAction& owner)
          : builder_(owner.builder_),
            index_pass_(owner.index_pass_),
            include_log_(&owner.include_log_),
            owner_(owner) {}
      void HandleTranslationUnit(clang::ASTContext& ctx) override {
        // Consume before any rewriting: the collector is complete only once
        // the whole top-level loop has run.
        const clang::syntax::TokenBuffer* tokens =
            owner_.recorder_ == nullptr ? nullptr
                                        : &owner_.recorder_->Consume();
        if (index_pass_) {
          if (!g_no_vendor_scan) ScanVendorAsts(ctx);
          builder_.IndexTu(ctx, *include_log_);
        } else {
          builder_.RewriteTreeTu(ctx, std::move(*include_log_), tokens);
        }
      }

     private:
      ProgramBuilder& builder_;
      bool index_pass_;
      std::vector<IncludeDirective>* include_log_;
      BuilderAction& owner_;
    };
    return std::make_unique<Consumer>(*this);
  }

 private:
  ProgramBuilder& builder_;
  bool index_pass_;
  std::vector<IncludeDirective> include_log_;
  std::unique_ptr<TokenRecorder> recorder_;
};

class BuilderActionFactory : public clang::tooling::FrontendActionFactory {
 public:
  BuilderActionFactory(ProgramBuilder& builder, bool index_pass)
      : builder_(builder), index_pass_(index_pass) {}

  std::unique_ptr<clang::FrontendAction> create() override {
    return std::make_unique<BuilderAction>(builder_, index_pass_);
  }

 private:
  ProgramBuilder& builder_;
  bool index_pass_;
};

}  // namespace

int main(int argc, const char** argv) {
  auto parser =
      clang::tooling::CommonOptionsParser::create(argc, argv, g_category);
  if (!parser) {
    llvm::errs() << llvm::toString(parser.takeError()) << "\n";
    return 1;
  }
  if (!llvm::sys::fs::is_directory(g_emit_dir.getValue())) {
    llvm::errs() << "error: -emit-dir " << g_emit_dir.getValue()
                 << " is not a directory; create it first (tapa analyze "
                    "creates <work_dir>/rewritten)\n";
    return 1;
  }
  const std::vector<std::string>& sources = parser->getSourcePathList();

  std::vector<std::string> main_files;
  main_files.reserve(sources.size());
  for (const std::string& source : sources) {
    main_files.push_back(CanonicalPath(source));
  }
  const bool is_vitis = g_target == CliTarget::kVitis;
  ProgramBuilder builder(
      g_top.getValue(),
      is_vitis ? SynthTarget::kXilinxVitis : SynthTarget::kXilinxHls,
      TreeConfig{g_emit_dir.getValue(), std::move(main_files)});

  clang::tooling::ClangTool index_tool(parser->getCompilations(),
                                       parser->getSourcePathList());
  BuilderActionFactory index_factory(builder, /*index_pass=*/true);
  int rc = index_tool.run(&index_factory);
  if (rc == 0 && !builder.MergeAndDiscover()) {
    for (const std::string& error : builder.errors()) {
      llvm::errs() << "error: " << error << "\n";
    }
    return 1;
  }
  if (rc != 0) return rc;

  clang::tooling::ClangTool rewrite_tool(parser->getCompilations(),
                                         parser->getSourcePathList());
  BuilderActionFactory rewrite_factory(builder, /*index_pass=*/false);
  rc = rewrite_tool.run(&rewrite_factory);
  if (rc != 0) return rc;

  std::string error;
  if (!builder.WriteTree(&error)) {
    llvm::errs() << "error: cannot write the rewritten source tree to "
                    "-emit-dir "
                 << g_emit_dir.getValue() << ": " << error << "\n";
    return 1;
  }

  std::cout << builder.EmitJson();
  return 0;
}
