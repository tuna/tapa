#ifndef TAPA_CODEGEN_BACKEND_H_
#define TAPA_CODEGEN_BACKEND_H_

#include <string_view>

#include "clang/AST/Decl.h"
#include "clang/AST/Stmt.h"
#include "clang/Rewrite/Core/Rewriter.h"

#include "frontend/classify.h"
#include "frontend/program.h"
#include "code_sink.h"

namespace tapa::cc {

// Everything a per-port hook needs to branch on: the parameter, its TAPA kind,
// the task's level, and whether it is the top-level task. The level x category
// fan-out is data here, not a method-name explosion.
struct PortContext {
  const clang::ParmVarDecl* param;
  TapaKind kind;
  TaskLevel level;
  bool is_top;
};

// A synthesis backend (vendor). The base class owns non-virtual routers that
// dispatch by kind; a vendor overrides the small per-category hooks and the
// whole-task operations it needs. Vendors are grouped: one class per family
// (e.g. Xilinx HLS + Vitis via a flag). RTTI-free by construction.
class Backend {
 public:
  virtual ~Backend() = default;

  // Serialized target name ("hls", "ignore", ...).
  virtual std::string_view Name() const = 0;

  // Router: emit the interface pragmas + anti-DCE stubs for one port.
  void EmitPortPreamble(const PortContext& p, CodeSink& out) const {
    if (IsStreamInterface(p.kind)) {
      EmitStreamPort(p, out);
    } else if (IsAsyncMmap(p.kind)) {
      EmitAsyncMmapPort(p, out);
    } else if (IsMmapInterface(p.kind)) {
      EmitMmapPort(p, out);
    } else {
      EmitScalarPort(p, out);
    }
  }

  // Whole-function operations. Signature rewriting depends on the function's
  // own level (a task's TaskModel.level + whether it is the top), so it is
  // applied to every task in a file, not just the current one.
  //  - RewriteSignature: rewrite the parameter list (mmap -> uint64 offset,
  //    Vitis stream -> qdma_axis). Applied to every task func in the file.
  //  - RewriteTaskFunc: rewrite the body/shell of the *current* task (top-level
  //    extern-C shell, middle-level connection shell, lower-level preamble).
  //  - StripOtherTask: reduce a non-current task in this file to a signature.
  //  - RewriteHelperFunc: rewrite a non-task helper function (same in every
  //    file).
  //  - LowerPipeline/LowerUnroll: lower a loop attribute to backend pragmas.
  virtual void RewriteSignature(const TaskModel& task, bool is_top,
                                clang::Rewriter& rewriter) const = 0;
  virtual void RewriteTaskFunc(const TaskModel& task, bool is_top,
                               clang::Rewriter& rewriter) const = 0;
  virtual void StripOtherTask(const clang::FunctionDecl* func,
                              clang::Rewriter& rewriter) const = 0;
  virtual void RewriteHelperFunc(const clang::FunctionDecl* func,
                                 clang::Rewriter& rewriter) const = 0;
  virtual void LowerPipeline(int ii, const clang::Stmt* body,
                             clang::Rewriter& rewriter) const = 0;
  virtual void LowerUnroll(int factor, const clang::Stmt* body,
                           clang::Rewriter& rewriter) const = 0;

 protected:
  // Per-category port hooks: a vendor overrides only what it needs; the
  // defaults emit nothing.
  virtual void EmitStreamPort(const PortContext&, CodeSink&) const {}
  virtual void EmitMmapPort(const PortContext&, CodeSink&) const {}
  virtual void EmitAsyncMmapPort(const PortContext&, CodeSink&) const {}
  virtual void EmitScalarPort(const PortContext&, CodeSink&) const {}
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_BACKEND_H_
