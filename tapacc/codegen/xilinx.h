#ifndef TAPA_CODEGEN_XILINX_H_
#define TAPA_CODEGEN_XILINX_H_

#include <string_view>

#include "backend.h"

namespace tapa::cc {

// The Xilinx backend, covering both Vitis HLS (`is_vitis = false`) and Vitis
// (`is_vitis = true`) via one grouped class.
class XilinxBackend final : public Backend {
 public:
  explicit XilinxBackend(bool is_vitis) : is_vitis_(is_vitis) {}

  std::string_view Name() const override { return "hls"; }

  void RewriteSignature(const TaskModel& task, bool is_top,
                        clang::Rewriter& rewriter) const override;
  void RewriteTaskFunc(const TaskModel& task, bool is_top,
                       clang::Rewriter& rewriter) const override;
  void StripOtherTask(const clang::FunctionDecl* func,
                      clang::Rewriter& rewriter) const override;
  void RewriteHelperFunc(const clang::FunctionDecl* func,
                         clang::Rewriter& rewriter) const override;
  void LowerPipeline(int ii, const clang::Stmt* body,
                     clang::Rewriter& rewriter) const override;
  void LowerUnroll(int factor, const clang::Stmt* body,
                   clang::Rewriter& rewriter) const override;

 protected:
  void EmitStreamPort(const PortContext&, CodeSink&) const override;
  void EmitMmapPort(const PortContext&, CodeSink&) const override;
  void EmitAsyncMmapPort(const PortContext&, CodeSink&) const override;
  void EmitScalarPort(const PortContext&, CodeSink&) const override;

 private:
  bool is_vitis_;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_XILINX_H_
