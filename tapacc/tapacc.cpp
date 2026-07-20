// tapacc: the TAPA C-to-HLS rewriter. Parses a (flattened) TAPA C++ translation
// unit into a typed program model (frontend/), generates per-task vendor HLS
// via a backend (codegen/), and emits on stdout the task graph the tapa-ir
// crate consumes: {top, target, tasks:{name:{code, level, synth,
// readable_name, ports, tasks, fifos}}}. `tapa analyze` nests that payload
// under the "graph" key of the work dir's tapa.json.
//
// Two distinct notions of "target" live in that schema, and the tapa-ir crate
// parses both as closed enums with deny_unknown_fields:
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
#include "llvm/Support/raw_ostream.h"

#include "nlohmann/json.hpp"

#include "codegen/ignore.h"
#include "codegen/rewrite.h"
#include "codegen/schema_fields.h"
#include "codegen/xilinx.h"
#include "frontend/build_program.h"
#include "frontend/classify.h"
#include "frontend/program.h"

namespace {

using namespace tapa::cc;

const char* LevelStr(TaskLevel level) {
  return level == TaskLevel::kUpper ? "upper" : "lower";
}

// Maps the per-task synthesis policy to its wire string. tapa-ir's SynthTarget
// is closed over {"hls", "ignore"}, so both HLS and Vitis tasks collapse to
// "hls": the per-task field only says whether to synthesize. The flow itself is
// emitted once at the graph root (see FlowStr).
const char* SynthStr(SynthTarget target) {
  switch (target) {
    case SynthTarget::kIgnore:
      return "ignore";
    case SynthTarget::kXilinxHls:
    case SynthTarget::kXilinxVitis:
      return "hls";
  }
  // Unreachable for a valid enumerator; keeps -Wswitch (not -Wswitch-default)
  // free to flag a future enumerator that needs a policy decision here.
  return "hls";
}

// Wire string for the root-level vendor flow. Kebab-case, unlike the per-task
// policy above.
const char* FlowStr(bool is_vitis) {
  return is_vitis ? "xilinx-vitis" : "xilinx-hls";
}

nlohmann::json PortJson(const Port& port) {
  nlohmann::json j{{kFieldCat, TapaKindCat(port.kind)},
                   {kFieldName, port.name},
                   {kFieldType, port.ctype},
                   {kFieldWidth, port.width}};
  if (port.chan_count) j[kFieldChanCount] = *port.chan_count;
  if (port.chan_size) j[kFieldChanSize] = *port.chan_size;
  return j;
}

nlohmann::json InstanceJson(const Instance& inst) {
  nlohmann::json j;
  j[kFieldStep] = inst.step;
  if (inst.name) j[kFieldName] = *inst.name;
  j[kFieldArgs] = nlohmann::json::object();
  for (const auto& [port, arg] : inst.args) {
    j[kFieldArgs][port] = {{kFieldArg, arg.arg},
                           {kFieldCat, TapaKindCat(arg.cat)}};
  }
  return j;
}

nlohmann::json StreamJson(const StreamDecl& stream) {
  nlohmann::json j;
  // External top-level stream ports have no depth; the key is omitted so
  // tapa-ir parses the entry as an external (kernel-boundary) FIFO.
  if (stream.depth) j[kFieldDepth] = *stream.depth;
  if (stream.produced_by) {
    j[kFieldProducedBy] = {stream.produced_by->task, stream.produced_by->index};
  }
  if (stream.consumed_by) {
    j[kFieldConsumedBy] = {stream.consumed_by->task, stream.consumed_by->index};
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
    out[kFieldTop] = program.top;
    out[kFieldTarget] = FlowStr(is_vitis);
    out[kFieldTasks] = nlohmann::json::object();
    for (const auto& [name, task] : program.tasks) {
      const Backend* backend = &hls;
      if (task.target == SynthTarget::kXilinxVitis) backend = &vitis;
      if (task.target == SynthTarget::kIgnore) backend = &ignore;

      nlohmann::json& t = out[kFieldTasks][name];
      t[kFieldCode] = EmitTaskCode(program, task, *backend, ctx);
      t[kFieldLevel] = LevelStr(task.level);
      t[kFieldSynth] = SynthStr(task.target);
      t[kFieldReadableName] = task.readable_name;
      t[kFieldPorts] = nlohmann::json::array();
      for (const Port& port : task.ports)
        t[kFieldPorts].push_back(PortJson(port));
      if (task.level == TaskLevel::kUpper) {
        t[kFieldTasks] = nlohmann::json::object();
        for (const auto& [child, insts] : task.instances) {
          nlohmann::json arr = nlohmann::json::array();
          for (const Instance& inst : insts) arr.push_back(InstanceJson(inst));
          t[kFieldTasks][child] = std::move(arr);
        }
        t[kFieldFifos] = nlohmann::json::object();
        for (const auto& [fifo, stream] : task.streams) {
          t[kFieldFifos][fifo] = StreamJson(stream);
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
