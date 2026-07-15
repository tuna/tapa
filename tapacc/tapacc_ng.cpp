// tapacc_ng: the rewritten tapacc driver. For now a dev entry point that emits
// per-task generated code as JSON ({top, tasks:{name:{code}}}) so the output
// can be diffed against the old tapacc. The graph-metadata serialization is
// held until the tapa-ir schema lands.

#include <iostream>
#include <memory>

#include "clang/AST/ASTConsumer.h"
#include "clang/AST/ASTContext.h"
#include "clang/Frontend/CompilerInstance.h"
#include "clang/Frontend/FrontendActions.h"
#include "clang/Tooling/CommonOptionsParser.h"
#include "clang/Tooling/Tooling.h"
#include "llvm/Support/CommandLine.h"
#include "llvm/Support/raw_ostream.h"

#include "nlohmann/json.hpp"

#include "codegen/rewrite.h"
#include "codegen/xilinx.h"
#include "frontend/build_program.h"

namespace {

llvm::cl::OptionCategory g_category("tapacc-ng options");

llvm::cl::opt<std::string> g_top("top", llvm::cl::Required,
                                 llvm::cl::desc("Top-level task name"),
                                 llvm::cl::cat(g_category));

enum class CliTarget { kHls, kVitis };
llvm::cl::opt<CliTarget> g_target(
    "target", llvm::cl::desc("Target flow (default xilinx-hls)"),
    llvm::cl::init(CliTarget::kHls), llvm::cl::cat(g_category),
    llvm::cl::values(
        clEnumValN(CliTarget::kHls, "xilinx-hls", "Xilinx HLS (default)"),
        clEnumValN(CliTarget::kVitis, "xilinx-vitis", "Xilinx Vitis")));

class NgConsumer : public clang::ASTConsumer {
 public:
  void HandleTranslationUnit(clang::ASTContext& ctx) override {
    using namespace tapa::cc;
    const bool is_vitis = g_target == CliTarget::kVitis;
    const SynthTarget default_target =
        is_vitis ? SynthTarget::kXilinxVitis : SynthTarget::kXilinxHls;

    Program program = BuildProgram(ctx, g_top, default_target);
    const XilinxBackend hls(/*is_vitis=*/false);
    const XilinxBackend vitis(/*is_vitis=*/true);

    nlohmann::json out;
    out["top"] = program.top;
    out["tasks"] = nlohmann::json::object();
    for (const auto& [name, task] : program.tasks) {
      if (task.is_template_spec) continue;  // wrapper emission: TODO
      const Backend* backend =
          task.target == SynthTarget::kXilinxVitis ? &vitis : &hls;
      out["tasks"][name]["code"] = EmitTaskCode(program, task, *backend, ctx);
    }
    std::cout << out;
  }
};

class NgAction : public clang::ASTFrontendAction {
 public:
  std::unique_ptr<clang::ASTConsumer> CreateASTConsumer(
      clang::CompilerInstance&, llvm::StringRef) override {
    return std::make_unique<NgConsumer>();
  }
};

}  // namespace

int main(int argc, const char** argv) {
  auto parser =
      clang::tooling::CommonOptionsParser::create(argc, argv, g_category);
  if (!parser) {
    llvm::errs() << llvm::toString(parser.takeError()) << "\n";
    return 1;
  }
  clang::tooling::ClangTool tool(parser->getCompilations(),
                                 parser->getSourcePathList());
  return tool.run(clang::tooling::newFrontendActionFactory<NgAction>().get());
}
