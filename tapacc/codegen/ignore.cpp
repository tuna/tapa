#include "ignore.h"

#include <string>

#include "clang/AST/Decl.h"

#include "code_sink.h"
#include "emit.h"
#include "frontend/classify.h"

namespace tapa::cc {

void IgnoreBackend::EmitStreamPort(const PortContext& p, CodeSink& out) const {
  EmitDummyStreamRW(p.param, p.kind, out, /*qdma=*/false);
}

void IgnoreBackend::EmitMmapPort(const PortContext& p, CodeSink& out) const {
  EmitDummyMmapOrScalarRW(p.param, p.kind, out);
}

void IgnoreBackend::EmitAsyncMmapPort(const PortContext& p,
                                      CodeSink& out) const {
  EmitDummyMmapOrScalarRW(p.param, p.kind, out);
}

void IgnoreBackend::EmitScalarPort(const PortContext& p, CodeSink& out) const {
  EmitDummyMmapOrScalarRW(p.param, p.kind, out);
}

void IgnoreBackend::RewriteTaskFunc(const TaskModel& task, bool /*is_top*/,
                                    clang::Rewriter& rewriter) const {
  const clang::FunctionDecl* func = task.def;
  if (!func->hasBody()) return;
  // Replace the body with a shell of dummy port reads/writes only.
  CodeSink sink;
  for (const clang::ParmVarDecl* param : func->parameters()) {
    EmitPortPreamble(
        PortContext{param, ClassifyTapaType(param), TaskLevel::kLower, false},
        sink);
    sink.Line("");  // blank line between parameters (and a trailing newline)
  }
  rewriter.ReplaceText(func->getBody()->getSourceRange(),
                       "{\n" + sink.Str() + "}\n");
}

void IgnoreBackend::StripOtherTask(const clang::FunctionDecl* /*func*/,
                                   clang::Rewriter& /*rewriter*/) const {
  // The ignore shell leaves other task functions untouched (unlike the Xilinx
  // backend); only the ignored task becomes a shell and helpers are cleared.
}

void IgnoreBackend::RewriteHelperFunc(const clang::FunctionDecl* func,
                                      clang::Rewriter& rewriter) const {
  // Clear non-task helper bodies in the ignore shell.
  if (func->hasBody()) {
    rewriter.ReplaceText(func->getBody()->getSourceRange(), "{}\n");
  }
}

}  // namespace tapa::cc
