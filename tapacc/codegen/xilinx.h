#ifndef TAPA_CODEGEN_XILINX_H_
#define TAPA_CODEGEN_XILINX_H_

#include "backend.h"

namespace tapa::cc {

// The Xilinx backend, covering both Vitis HLS (`is_vitis = false`) and Vitis
// (`is_vitis = true`) via one grouped class.
class XilinxBackend final : public Backend {
 public:
  explicit XilinxBackend(bool is_vitis) : is_vitis_(is_vitis) {}

  void RewriteSignature(const TaskModel& task, bool is_top,
                        EditSink& edits) const override;
  void RewriteTaskFunc(const TaskModel& task, bool is_top,
                       EditSink& edits) const override;
  void StripOtherTask(const clang::FunctionDecl* func,
                      EditSink& edits) const override;
  void RewriteHelperFunc(const clang::FunctionDecl* func,
                         EditSink& edits) const override;
  void LowerPipeline(int ii, const std::string& style, const clang::Stmt* body,
                     EditSink& edits) const override;
  void LowerUnroll(int factor, const clang::Stmt* body,
                   EditSink& edits) const override;
  void LowerStmtAttr(const clang::Attr& attr, const clang::Stmt* body,
                     EditSink& edits) const override;
  void LowerDeclAttr(const clang::Attr& attr, const clang::VarDecl& var,
                     const clang::DeclStmt& decl,
                     EditSink& edits) const override;
  void LowerParamAttr(const clang::Attr& attr, const clang::ParmVarDecl& param,
                      const clang::Stmt* body, EditSink& edits) const override;

 protected:
  void EmitStreamPort(const PortContext&, CodeSink&) const override;
  void EmitMmapPort(const PortContext&, CodeSink&) const override;
  void EmitAsyncMmapPort(const PortContext&, CodeSink&) const override;
  void EmitScalarPort(const PortContext&, CodeSink&) const override;

 private:
  // One non-defining declaration of the Vitis top, wrapped in extern "C"
  // after its trailing semicolon: the declaration-level half of the
  // top-level rewrite RewriteTaskFunc applies to every redeclaration.
  void WrapTaskDeclExternC(const clang::FunctionDecl* decl,
                           EditSink& edits) const;

  bool is_vitis_;
};

}  // namespace tapa::cc

#endif  // TAPA_CODEGEN_XILINX_H_
