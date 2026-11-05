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
//   - root "target": the vendor FLOW, kebab-case
//     "xilinx-vitis"/"xilinx-slash"/"xilinx-hls".
//   - per-task "synth": the synthesis POLICY, "hls"/"ignore"/"aie" -- it
//     answers how the task is built: HLS, skipped, or on the AI Engine. The
//     internal SynthTarget still carries the flow per task so the right
//     backend gets picked, but the HLS/Vitis distinction collapses on the
//     wire.
//
// Changing what this file emits changes a contract with Rust code that cannot
// see it. `bazel test //tapa-core:tapacc_conformance_test` (Linux only) is the
// guard: it runs this binary on tests/apps/vadd/vadd.cpp and strict-parses the
// stdout below with tapa-ir's real types, so a field tapa-ir does not model --
// or a required one that stops being emitted -- fails there rather than in
// someone's build.

#include <iostream>
#include <map>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "clang/Tooling/CommonOptionsParser.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/CommandLine.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/raw_ostream.h"

#include "codegen/builder_action.h"
#include "codegen/tree_writer.h"
#include "frontend/program_builder.h"

namespace {

using namespace tapa::cc;

llvm::cl::OptionCategory g_category("tapacc options");

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

llvm::cl::opt<SynthTarget> g_target(
    "target", llvm::cl::desc("Target flow (default xilinx-hls)"),
    llvm::cl::init(SynthTarget::kXilinxHls), llvm::cl::cat(g_category),
    llvm::cl::values(
        clEnumValN(SynthTarget::kXilinxHls, "xilinx-hls",
                   "Xilinx HLS (default)"),
        clEnumValN(SynthTarget::kXilinxVitis, "xilinx-vitis", "Xilinx Vitis"),
        clEnumValN(SynthTarget::kXilinxSlash, "xilinx-slash", "Xilinx SLASH")));

// Per-task target mappings from `tapa analyze --target TASK:T`; they win
// over the source attribute so one source builds both PL-only and PL+AIE.
llvm::cl::list<std::string> g_task_targets(
    "task-target",
    llvm::cl::desc("Map one task to a target, TASK:TARGET with TARGET one of "
                   "aie, hls, ignore; overrides [[tapa::target]]"),
    llvm::cl::value_desc("task:target"), llvm::cl::cat(g_category));

class BuilderActionFactory : public clang::tooling::FrontendActionFactory {
 public:
  BuilderActionFactory(ProgramBuilder& builder, bool index_pass)
      : builder_(builder), index_pass_(index_pass) {}

  std::unique_ptr<clang::FrontendAction> create() override {
    return std::make_unique<BuilderAction>(builder_, index_pass_,
                                           /*vendor_scan=*/!g_no_vendor_scan);
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
  const SynthTarget flow = g_target.getValue();
  std::map<std::string, SynthTarget> task_targets;
  for (const std::string& mapping : g_task_targets) {
    const size_t colon = mapping.rfind(':');
    const std::string task = mapping.substr(0, colon);
    const std::string target =
        colon == std::string::npos ? "" : mapping.substr(colon + 1);
    if (task.empty() ||
        (target != "aie" && target != "hls" && target != "ignore")) {
      llvm::errs() << "error: -task-target " << mapping
                   << " is not TASK:TARGET with TARGET one of aie, hls, "
                      "ignore\n";
      return 1;
    }
    task_targets[task] = target == "aie"      ? SynthTarget::kAie
                         : target == "ignore" ? SynthTarget::kIgnore
                                              : flow;
  }
  ProgramBuilder builder(
      g_top.getValue(), flow,
      TreeConfig{g_emit_dir.getValue(), std::move(main_files)});
  builder.SetTaskTargets(std::move(task_targets));

  const auto fail_with_errors = [&builder] {
    for (const std::string& error : builder.errors()) {
      llvm::errs() << "error: " << error << "\n";
    }
    return 1;
  };

  clang::tooling::ClangTool index_tool(parser->getCompilations(),
                                       parser->getSourcePathList());
  BuilderActionFactory index_factory(builder, /*index_pass=*/true);
  int rc = index_tool.run(&index_factory);
  if (rc == 0 && !builder.MergeAndDiscover()) return fail_with_errors();
  if (rc != 0) return rc;

  clang::tooling::ClangTool rewrite_tool(parser->getCompilations(),
                                         parser->getSourcePathList());
  BuilderActionFactory rewrite_factory(builder, /*index_pass=*/false);
  rc = rewrite_tool.run(&rewrite_factory);
  if (rc != 0) return rc;
  if (!builder.CheckAieEot()) return fail_with_errors();

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
