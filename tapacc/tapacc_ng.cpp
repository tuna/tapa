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

#include "codegen/ignore.h"
#include "codegen/rewrite.h"
#include "codegen/xilinx.h"
#include "frontend/build_program.h"
#include "frontend/classify.h"
#include "frontend/program.h"

namespace {

using tapa::cc::Arg;
using tapa::cc::Instance;
using tapa::cc::Port;
using tapa::cc::StreamDecl;
using tapa::cc::SynthTarget;
using tapa::cc::TapaKindCat;
using tapa::cc::TaskLevel;
using tapa::cc::TaskModel;

const char* LevelStr(TaskLevel level) {
  return level == TaskLevel::kUpper ? "upper" : "lower";
}

const char* TargetStr(SynthTarget target) {
  switch (target) {
    case SynthTarget::kXilinxVitis:
      return "xilinx_vitis";
    case SynthTarget::kIgnore:
      return "ignore";
    default:
      return "xilinx_hls";
  }
}

nlohmann::json PortJson(const Port& port) {
  nlohmann::json j{{"cat", TapaKindCat(port.kind)},
                   {"name", port.name},
                   {"type", port.ctype},
                   {"width", port.width}};
  if (port.chan_count) j["chan_count"] = *port.chan_count;
  if (port.chan_size) j["chan_size"] = *port.chan_size;
  return j;
}

nlohmann::json InstanceJson(const Instance& inst) {
  nlohmann::json j;
  j["step"] = inst.step;
  if (inst.name) j["name"] = *inst.name;
  j["args"] = nlohmann::json::object();
  for (const auto& [port, arg] : inst.args) {
    j["args"][port] = {{"arg", arg.arg}, {"cat", TapaKindCat(arg.cat)}};
  }
  return j;
}

nlohmann::json StreamJson(const StreamDecl& stream) {
  nlohmann::json j;
  j["depth"] = stream.depth;
  if (stream.produced_by) {
    j["produced_by"] = {stream.produced_by->task, stream.produced_by->index};
  }
  if (stream.consumed_by) {
    j["consumed_by"] = {stream.consumed_by->task, stream.consumed_by->index};
  }
  return j;
}

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
    const IgnoreBackend ignore;

    nlohmann::json out;
    out["top"] = program.top;
    out["tasks"] = nlohmann::json::object();
    for (const auto& [name, task] : program.tasks) {
      const Backend* backend = &hls;
      if (task.target == SynthTarget::kXilinxVitis) backend = &vitis;
      if (task.target == SynthTarget::kIgnore) backend = &ignore;

      nlohmann::json& t = out["tasks"][name];
      t["code"] = EmitTaskCode(program, task, *backend, ctx);
      t["level"] = LevelStr(task.level);
      t["target"] = TargetStr(task.target);
      t["readable_name"] = task.readable_name;
      t["ports"] = nlohmann::json::array();
      for (const Port& port : task.ports) t["ports"].push_back(PortJson(port));
      if (task.level == TaskLevel::kUpper) {
        t["tasks"] = nlohmann::json::object();
        for (const auto& [child, insts] : task.instances) {
          nlohmann::json arr = nlohmann::json::array();
          for (const Instance& inst : insts) arr.push_back(InstanceJson(inst));
          t["tasks"][child] = std::move(arr);
        }
        t["fifos"] = nlohmann::json::object();
        for (const auto& [fifo, stream] : task.streams) {
          t["fifos"][fifo] = StreamJson(stream);
        }
      }
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
