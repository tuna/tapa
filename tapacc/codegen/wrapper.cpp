#include "wrapper.h"

#include <string>

#include "clang/AST/Decl.h"
#include "clang/Basic/LangOptions.h"

#include "frontend/classify.h"
#include "code_sink.h"

namespace tapa::cc {

std::string GenerateWrapper(const TaskModel& task, const Backend& backend,
                            clang::ASTContext& ctx) {
  // Suppress the tag keyword ("class"/"struct") so the printed parameter types
  // match the header the vendor toolchain sees.
  clang::PrintingPolicy policy = ctx.getPrintingPolicy();
  policy.SuppressTagKeyword = true;

  const auto params = task.def->parameters();

  std::string code = "\n\nvoid " + task.name + "(";
  for (unsigned i = 0; i < params.size(); ++i) {
    if (i > 0) code += ", ";
    code += params[i]->getType().getAsString(policy) + " " +
            params[i]->getNameAsString();
  }
  code += ") {\n";

  // Lower-level interface preamble for each concrete port (2-space indented,
  // no blank separators -- matching the old wrapper layout).
  CodeSink sink;
  for (const clang::ParmVarDecl* param : params) {
    backend.EmitPortPreamble(
        PortContext{param, ClassifyTapaType(param), TaskLevel::kLower, false},
        sink);
  }
  for (const std::string& line : sink.Lines()) {
    code += "  " + line + "\n";
  }

  // Call the actual templated function by its readable (templated) name.
  code += "  " + task.readable_name + "(";
  for (unsigned i = 0; i < params.size(); ++i) {
    if (i > 0) code += ", ";
    code += params[i]->getNameAsString();
  }
  code += ");\n}\n";
  return code;
}

void InsertWrapper(const TaskModel& task, const Backend& backend,
                   clang::ASTContext& ctx, clang::Rewriter& rewriter) {
  if (task.invoker == nullptr) return;
  rewriter.InsertTextAfterToken(task.invoker->getEndLoc(),
                                GenerateWrapper(task, backend, ctx));
}

}  // namespace tapa::cc
