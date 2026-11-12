#include "wrapper.h"

#include <string>

#include "clang/AST/Decl.h"
#include "clang/Basic/LangOptions.h"

#include "code_sink.h"
#include "conventions.h"
#include "frontend/classify.h"
#include "frontend/type_args.h"

namespace tapa::cc {

std::string GenerateWrapper(const TaskModel& task, const Backend& backend,
                            clang::ASTContext& ctx) {
  // Suppress the tag keyword ("class"/"struct") so the printed parameter types
  // match the header the vendor toolchain sees.
  clang::PrintingPolicy policy = ctx.getPrintingPolicy();
  policy.SuppressTagKeyword = true;

  const auto params = task.def->parameters();
  const bool upper = task.level == TaskLevel::kUpper;

  // An upper task's parameters take the signature every upper task's shell
  // has: a memory port is its base address.
  std::string code = "\n\nvoid " + task.name + "(";
  for (unsigned i = 0; i < params.size(); ++i) {
    if (i > 0) code += ", ";
    const std::string name = params[i]->getNameAsString();
    const TapaKind kind = ClassifyTapaType(params[i]);
    if (upper && (kind == TapaKind::kMmap || kind == TapaKind::kAsyncMmap)) {
      code += "uint64_t " + OffsetName(name);
    } else if (upper && IsMmapArray(kind)) {
      for (int64_t j = 0; j < ArraySize(params[i]); ++j) {
        if (j > 0) code += ", ";
        code += "uint64_t " + ArrayElemOffset(name, static_cast<int>(j));
      }
    } else {
      code += params[i]->getType().getAsString(policy) + " " + name;
    }
  }
  code += ") {\n";

  // The interface preamble for each concrete port at the task's level
  // (2-space indented, no blank separators).
  CodeSink sink;
  for (const clang::ParmVarDecl* param : params) {
    backend.EmitPortPreamble(
        PortContext{param, ClassifyTapaType(param), task.level, false}, sink);
  }
  for (const std::string& line : sink.Lines()) {
    code += "  " + line + "\n";
  }
  // An upper task's body is that shell alone.
  if (upper) return code + "}\n";

  // Call the actual templated function by its readable (templated) name.
  code += "  " + task.readable_name + "(";
  for (unsigned i = 0; i < params.size(); ++i) {
    if (i > 0) code += ", ";
    code += params[i]->getNameAsString();
  }
  code += ");\n}\n";
  return code;
}

}  // namespace tapa::cc
