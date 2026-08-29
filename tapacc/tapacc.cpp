// tapacc: the TAPA C-to-HLS rewriter. Parses every input translation unit
// (flattened by `tapa analyze`) into one merged task graph and emits that
// single graph on stdout for the tapa-ir crate to consume:
// {top, target, tasks:{name:{srcs, include_dirs, defines, level, synth,
// readable_name, ports, tasks, fifos}}}. `tapa analyze` nests that payload
// under the "graph" key of the work dir's tapa.json.
//
// The per-task rewritten C++ text is NOT inline in that JSON: each task's
// `srcs` names a file under the required `-emit-dir` that tapacc writes
// the text to (`<emit-dir>/<task>.cpp`), so the work dir's rewritten/
// tree plus tapa.json together are the analyze artifact.
//
// Each ClangTool action owns its ASTContext and AST nodes never outlive
// their TU, so the pipeline runs as TWO passes over the same file list
// around a pure-data merge (frontend/program_builder.{h,cpp}):
//
//   1. an index action per TU: collect every function definition sighted
//      in the main file plus every `tapa::task::invoke` edge, the callee
//      resolved through the declaration visible in that TU;
//   2. the merge: one definition index keyed by FQN + canonical parameter
//      signature (mangled name for template specializations), BFS from
//      `--top` over the merged edges, and the merge-rule diagnostics;
//   3. a rewrite action per TU: a per-TU view of the merged task set, with
//      per-task emission for every merged task defined in that TU.
//
// The single JSON prints only when both passes ran clean for every TU, so a
// program that fails a merge rule or a per-task diagnostic exits non-zero
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

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/CommonOptionsParser.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/CommandLine.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/raw_ostream.h"

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

// Where each task's rewritten source is written (`<dir>/<task>.cpp`), the
// file the task's `srcs` manifest entry names. The caller creates the
// directory; tapacc refuses to run if it is missing or not writable.
llvm::cl::opt<std::string> g_emit_dir(
    "emit-dir", llvm::cl::Required,
    llvm::cl::desc("Directory to write each task's rewritten source to"),
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
    return true;
  }

  std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
      clang::CompilerInstance&, llvm::StringRef) override {
    class Consumer : public clang::ASTConsumer {
     public:
      Consumer(ProgramBuilder& builder, bool index_pass)
          : builder_(builder), index_pass_(index_pass) {}
      void HandleTranslationUnit(clang::ASTContext& ctx) override {
        if (index_pass_) {
          if (!g_no_vendor_scan) ScanVendorAsts(ctx);
          builder_.IndexTu(ctx);
        } else {
          builder_.RewriteTu(ctx);
        }
      }

     private:
      ProgramBuilder& builder_;
      bool index_pass_;
    };
    return std::make_unique<Consumer>(builder_, index_pass_);
  }

 private:
  ProgramBuilder& builder_;
  bool index_pass_;
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
  const bool is_vitis = g_target == CliTarget::kVitis;
  ProgramBuilder builder(g_top.getValue(), is_vitis ? SynthTarget::kXilinxVitis
                                                    : SynthTarget::kXilinxHls);

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
  if (!builder.WriteSources(g_emit_dir.getValue(), &error)) {
    llvm::errs() << "error: cannot write rewritten sources to -emit-dir "
                 << g_emit_dir.getValue() << ": " << error << "\n";
    return 1;
  }

  std::cout << builder.EmitJson();
  return 0;
}
