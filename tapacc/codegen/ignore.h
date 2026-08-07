#ifndef TAPA_CODEGEN_IGNORE_H_
#define TAPA_CODEGEN_IGNORE_H_

#include "backend.h"

namespace tapa::cc {

// The "ignore" backend: a task marked [[tapa::target("ignore")]] is a black box
// the user supplies RTL for, so tapacc emits only a shell whose ports survive
// dead-code elimination (dummy reads/writes) and whose body is otherwise empty.
class IgnoreBackend final : public Backend {
 public:
  // Ignore never rewrites signatures (no mmap -> offset).
  void RewriteSignature(const TaskModel&, bool,
                        clang::Rewriter&) const override {}
  void RewriteTaskFunc(const TaskModel& task, bool is_top,
                       clang::Rewriter& rewriter) const override;
  void StripOtherTask(const clang::FunctionDecl* func,
                      clang::Rewriter& rewriter) const override;
  void RewriteHelperFunc(const clang::FunctionDecl* func,
                         clang::Rewriter& rewriter) const override;
  // Loop attributes vanish with the replaced body; nothing to lower.
  void LowerPipeline(int, const clang::Stmt*, clang::Rewriter&) const override {
  }
  void LowerUnroll(int, const clang::Stmt*, clang::Rewriter&) const override {}

 protected:
  void EmitStreamPort(const PortContext&, CodeSink&) const override;
  void EmitMmapPort(const PortContext&, CodeSink&) const override;
  void EmitAsyncMmapPort(const PortContext&, CodeSink&) const override;
  void EmitScalarPort(const PortContext&, CodeSink&) const override;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_IGNORE_H_
